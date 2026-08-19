//! Model-aware MCP middleware binary.
//!
//! Fronts a real MCP stdio server and enforces the price-awareness policy by
//! requiring a `caller_model` argument on every `tools/call`:
//!
//! ```text
//! mcp-toolmon -- <real-server-command> [server args...]
//! ```
//!
//! On `tools/list` it injects a required `caller_model` argument into every
//! advertised tool schema. On `tools/call` it reads `arguments.caller_model`,
//! rejects the call if absent, uses graded cost model with optional grant_id
//! to decide allow/delegate, and strips both caller_model and grant_id before
//! forwarding. All other traffic passes through untouched.
//!
//! Fail-open: if the price table cannot be loaded the proxy is a transparent
//! passthrough (no schema injection, no enforcement).
//!
//! Environment:
//! * `COST_GATE_TABLE` — path to `model_prices.json` (required for enforcement).
//! * `COST_GATE_TOOL_METRICS` — path to tool metrics rollup JSON (optional).
//! * `COST_GATE_GRANTS_DIR` — directory with grant JSON files (optional).
//! * `COST_GATE_SCALE_MAX` — budget scale max (default 100).
//! * `COST_GATE_BUDGET_ZERO_PRICE` — price at which budget=0 (default 60.0).
//! * `COST_GATE_TELEMETRY_LOG` — path to append per-call `CallTelemetry` JSONL
//!   records (ticket 9d527ad1; optional, no telemetry emitted when unset).

use std::sync::{
    Arc,
    Mutex,
};

use mcp_toolmon::{
    proxy::{
        ClientAction,
        PendingCalls,
        PendingList,
        handle_client_message,
        handle_server_message,
    },
    shadow,
    supervisor::Supervisor,
    watcher,
};
use serde_json::Value;
use tokio::io::{
    AsyncBufReadExt,
    AsyncWriteExt,
};

fn log(msg: &str) {
    eprintln!("[mcp-toolmon] {msg}");
}

/// Split argv into the real server command (everything after `--`).
fn server_command(argv: &[String]) -> Vec<String> {
    if let Some(pos) = argv.iter().position(|a| a == "--") {
        argv[pos + 1..].to_vec()
    } else {
        argv[1..].to_vec()
    }
}

fn main() {
    // A dedicated multi-thread runtime: tokio::io::stdin() reads via the
    // blocking-task pool, which runs concurrently with the reader task
    // regardless of flavor, but multi-thread keeps the two pumps (and any
    // future supervisor/watcher tasks from T4/T6) on real OS threads instead
    // of cooperating on a single one.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(run());
}

async fn run() {
    let argv: Vec<String> = std::env::args().collect();

    // Check for verdict subcommand before proxy logic.
    if argv.len() > 1 && argv[1] == "verdict" {
        toolmon_costgate::verdict::run_verdict(&argv);
        return;
    }

    let command = server_command(&argv);
    if command.is_empty() {
        log(
            "no server command provided; usage: mcp-toolmon -- <server> [args...]",
        );
        std::process::exit(2);
    }

    let policy = toolmon_costgate::config::build_policy_from_env();
    let telemetry_log =
        toolmon_costgate::config::telemetry_log_path_from_env().map(Arc::new);

    let supervisor = match Supervisor::spawn(&command) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            log(&format!("failed to launch server {command:?}: {e}"));
            std::process::exit(2);
        },
    };

    // Shared client-stdout writer (both the reader task and this loop may
    // write to it), the set of in-flight tools/list request ids, and the
    // in-flight tools/call requests awaiting a response for telemetry
    // correlation. The child handle itself is not baked into either task's
    // closure: both go through `Supervisor`, so a future supervisor (T4) can
    // swap the underlying child without restructuring these pumps.
    let client_out = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let pending = Arc::new(Mutex::new(PendingList::default()));
    let pending_calls = Arc::new(Mutex::new(PendingCalls::default()));

    // Binary watcher (T6): polls the canonical child binary path and
    // triggers a swap on a confirmed change. Its output (synthesized
    // reload-interruption errors plus the post-swap `tools/list_changed`
    // notification) arrives on this channel and is drained straight to the
    // client's stdout by a dedicated task, same as the reader task below.
    let (watcher_tx, mut watcher_rx) =
        tokio::sync::mpsc::unbounded_channel::<String>();
    let watcher_drain_out = Arc::clone(&client_out);
    let watcher_drain_task = tokio::spawn(async move {
        while let Some(line) = watcher_rx.recv().await {
            write_client_line(&watcher_drain_out, &line).await;
        }
    });
    let watcher_config = watcher::WatcherConfig::from_env();
    let watcher_handle = if watcher_config.enabled {
        match shadow::resolve_canonical(&command[0]) {
            Ok(canonical) => watcher::spawn(
                Arc::clone(&supervisor),
                canonical,
                supervisor.shadow_root_path().to_path_buf(),
                watcher_tx,
                watcher_config,
            ),
            Err(e) => {
                log(&format!(
                    "binary watcher disabled: could not resolve canonical path for {:?}: {e}",
                    command[0]
                ));
                None
            },
        }
    } else {
        log("binary watcher disabled via TOOLMON_RELOAD");
        None
    };

    // Server -> client: pass through, injecting tool schemas on list responses
    // and emitting telemetry for correlated tools/call responses.
    let reader_sup = Arc::clone(&supervisor);
    let reader_out = Arc::clone(&client_out);
    let reader_policy = policy.clone();
    let reader_pending = Arc::clone(&pending);
    let reader_pending_calls = Arc::clone(&pending_calls);
    let reader_telemetry_log = telemetry_log.clone();
    let reader = tokio::spawn(async move {
        loop {
            let Some(line) = reader_sup.read_line().await else {
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            let out_line = match serde_json::from_str::<Value>(&line) {
                Ok(msg) => {
                    // A message carrying a non-null `id` is a response to a
                    // request we forwarded; clear it from the in-flight
                    // table before it can become a synthesized-error
                    // candidate on a later swap (R6).
                    if let Some(id) = msg.get("id") {
                        if !id.is_null() {
                            reader_sup.resolve_pending(id).await;
                        }
                    }
                    let (rewritten, telemetry) = handle_server_message(
                        msg,
                        reader_policy.as_deref(),
                        &mut reader_pending.lock().unwrap(),
                        &mut reader_pending_calls.lock().unwrap(),
                    );
                    if let Some(telemetry) = &telemetry {
                        toolmon_costgate::config::emit_telemetry_jsonl(
                            reader_telemetry_log.as_deref(),
                            telemetry,
                        );
                    }
                    serde_json::to_string(&rewritten).unwrap_or(line)
                },
                Err(_) => line,
            };
            write_client_line(&reader_out, &out_line).await;
        }
    });

    // Client -> server: gate tools/call, record tools/list, forward the rest.
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(msg) => {
                let (action, telemetry) = handle_client_message(
                    msg,
                    policy.as_deref(),
                    &mut pending.lock().unwrap(),
                    &mut pending_calls.lock().unwrap(),
                );
                if let Some(telemetry) = &telemetry {
                    toolmon_costgate::config::emit_telemetry_jsonl(
                        telemetry_log.as_deref(),
                        telemetry,
                    );
                }
                match action {
                    ClientAction::Forward(v) => {
                        // Record the id (if any) as in-flight BEFORE writing,
                        // so a swap racing this write always sees it (R6).
                        let id =
                            v.get("id").cloned().filter(|id| !id.is_null());
                        if let Some(id) = &id {
                            supervisor.record_pending(id).await;
                        }
                        let s = serde_json::to_string(&v).unwrap_or(line);
                        if !supervisor.write_line(&s).await {
                            // No healthy child right now (mid-outage, R7):
                            // answer immediately with a synthesized
                            // reload-interruption error instead of hanging
                            // the client or exiting the proxy (R6).
                            if let Some(id) = id {
                                let err =
                                    supervisor.synthesize_and_clear(&id).await;
                                let s = serde_json::to_string(&err)
                                    .unwrap_or_default();
                                write_client_line(&client_out, &s).await;
                            }
                        }
                    },
                    ClientAction::Respond(v) => {
                        let s = serde_json::to_string(&v).unwrap_or_default();
                        write_client_line(&client_out, &s).await;
                    },
                }
            },
            Err(_) => {
                // Not JSON we understand; forward verbatim (no id to track).
                let _ = supervisor.write_line(&line).await;
            },
        }
    }

    // Client stdin reached EOF: this is the ONLY condition that still shuts
    // the proxy down (R7 changes what does NOT cause exit — a dying or
    // failed-to-respawn child no longer does). Close the current child's
    // stdin, wait for it, best-effort clean up this instance's shadow
    // directories, then exit 0 unconditionally; the child's own exit code is
    // no longer propagated since a swapped-in child's exit code has no
    // meaningful relationship to the client session.
    let _ = supervisor.shutdown().await;
    let _ = reader.await;
    if let Some(h) = watcher_handle {
        h.abort();
    }
    watcher_drain_task.abort();
}

async fn write_client_line(
    out: &tokio::sync::Mutex<tokio::io::Stdout>,
    line: &str,
) {
    let mut out = out.lock().await;
    let _ = out.write_all(line.as_bytes()).await;
    let _ = out.write_all(b"\n").await;
    let _ = out.flush().await;
}
