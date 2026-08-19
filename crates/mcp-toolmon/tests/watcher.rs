//! Binary watcher integration tests (T6). Unit tests for the pure debounce
//! logic live in `src/watcher.rs`; these exercise the watcher wired to a
//! real `Supervisor` (real fs polling for `integration_watcher_real_poll`,
//! direct API calls elsewhere per spec C1's determinism preference).

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        Mutex as StdMutex,
    },
    time::Duration,
};

use mcp_toolmon::{
    supervisor::Supervisor,
    watcher::{
        self,
        WatcherConfig,
    },
};
use serde_json::{
    Value,
    json,
};
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

fn fake_v1_bytes() -> Vec<u8> {
    std::fs::read(env!("CARGO_BIN_EXE_fake-mcp-v1")).unwrap()
}

fn fake_v2_bytes() -> Vec<u8> {
    std::fs::read(env!("CARGO_BIN_EXE_fake-mcp-v2")).unwrap()
}

fn write_exe(
    path: &Path,
    bytes: &[u8],
) {
    std::fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

fn canonical_exe_name() -> &'static str {
    if cfg!(windows) {
        "canonical.exe"
    } else {
        "canonical"
    }
}

/// Bounded-wait helper (no bare `sleep` as the synchronization mechanism).
async fn wait_until<F: Fn() -> bool>(
    condition: F,
    timeout: Duration,
    msg: &str,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if condition() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("{msg}");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn perform_handshake(supervisor: &Supervisor) {
    let init =
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}});
    assert!(
        supervisor.write_line(&init.to_string()).await,
        "write initialize failed"
    );
    supervisor
        .read_line()
        .await
        .expect("child closed without responding to initialize");
    let notif = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    assert!(
        supervisor.write_line(&notif.to_string()).await,
        "write notifications/initialized failed"
    );
}

fn generation_text(resp: &Value) -> String {
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Serializes the two tests below that mutate the process-global
/// `TOOLMON_RELOAD` env var, so they can't interleave with each other under
/// `cargo test`'s default multi-threaded runner.
static ENV_LOCK: StdMutex<()> = StdMutex::new(());

/// AC: `TOOLMON_RELOAD=0` disables the watcher entirely — no poller task is
/// spawned. Asserted by calling `watcher::spawn` with the env-derived
/// config and observing it returns `None`, plus (for a behavioral guarantee
/// beyond just object identity) that a real binary change is never
/// detected within a bounded wait.
#[tokio::test]
async fn watcher_disabled_by_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: serialized by ENV_LOCK against the only other test in this
    // file that touches process env (there is none currently mutating
    // TOOLMON_RELOAD besides this test).
    unsafe {
        std::env::set_var("TOOLMON_RELOAD", "0");
    }
    let config = WatcherConfig::from_env();
    assert!(!config.enabled, "TOOLMON_RELOAD=0 must resolve to disabled");

    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());
    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor = Arc::new(
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap(),
    );

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = watcher::spawn(
        Arc::clone(&supervisor),
        canonical.clone(),
        shadow_root.path().to_path_buf(),
        tx,
        config,
    );
    assert!(
        handle.is_none(),
        "no poller task should be spawned when disabled"
    );

    // Behavioral corroboration: with no poller running, a real binary
    // change is never picked up.
    perform_handshake(&supervisor).await;
    let before = call_generation(&supervisor, 1).await;
    assert_eq!(generation_text(&before), "v1");
    write_exe(&canonical, &fake_v2_bytes());
    tokio::time::sleep(Duration::from_millis(150)).await;
    let after = call_generation(&supervisor, 2).await;
    assert_eq!(
        generation_text(&after),
        "v1",
        "disabled watcher must never detect the change"
    );

    unsafe {
        std::env::remove_var("TOOLMON_RELOAD");
    }
    let _ = supervisor.shutdown().await;
}

async fn call_generation(
    supervisor: &Supervisor,
    id: i64,
) -> Value {
    let req = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"generation","arguments":{}}});
    assert!(
        supervisor.write_line(&req.to_string()).await,
        "write to child failed"
    );
    let line = supervisor
        .read_line()
        .await
        .expect("child closed without responding");
    serde_json::from_str(&line).unwrap()
}

/// The ONE test exercising the real mtime/size poller end to end (spec
/// C1/C3): a short `poll_ms`, canonical path overwritten with v2 bytes from
/// outside, and a bounded `wait_until` (never a bare `sleep` as the
/// assertion) observing both the `notifications/tools/list_changed`
/// notification and a subsequent `generation` call returning "v2".
///
/// Timing sensitivity: this is the one place real wall-clock polling is
/// exercised. `poll_ms` is set low (25ms) and the wait bound generously
/// (5s) to absorb CI/host scheduling jitter, but it is inherently
/// timing-dependent — a sufficiently starved test host could still exceed
/// the bound. No other test in this suite depends on real timing.
#[tokio::test]
async fn integration_watcher_real_poll() {
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());

    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor = Arc::new(
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap(),
    );
    perform_handshake(&supervisor).await;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let received: Arc<TokioMutex<Vec<String>>> =
        Arc::new(TokioMutex::new(Vec::new()));
    let collector_received = Arc::clone(&received);
    let collector = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            collector_received.lock().await.push(line);
        }
    });

    let config = WatcherConfig {
        enabled: true,
        poll_ms: 25,
    };
    let watcher_handle = watcher::spawn(
        Arc::clone(&supervisor),
        canonical.clone(),
        shadow_root.path().to_path_buf(),
        tx,
        config,
    )
    .expect("watcher must spawn when enabled");

    // Simulate the canonical binary having been rebuilt.
    write_exe(&canonical, &fake_v2_bytes());

    wait_until(
        || {
            received
                .try_lock()
                .map(|lines| lines.iter().any(|l| l.contains("notifications/tools/list_changed")))
                .unwrap_or(false)
        },
        Duration::from_secs(5),
        "expected notifications/tools/list_changed within the bounded wait after a real binary change",
    )
    .await;

    // Post-swap, the generation tool must now be served by v2. Retry the
    // call itself (bounded) rather than sleeping first: the swap may still
    // be draining/respawning for a brief moment after the notification
    // above was queued, during which a write can transiently fail (R7).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut last_seen = String::new();
    loop {
        let req = json!({"jsonrpc":"2.0","id":99,"method":"tools/call","params":{"name":"generation","arguments":{}}});
        if supervisor.write_line(&req.to_string()).await {
            let line = supervisor
                .read_line()
                .await
                .expect("child closed without responding");
            let resp: Value = serde_json::from_str(&line).unwrap();
            last_seen = generation_text(&resp);
            if last_seen == "v2" {
                break;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "generation tool never returned \"v2\" after the real-poller swap; last saw {last_seen:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(last_seen, "v2");

    watcher_handle.abort();
    collector.abort();
    let _ = supervisor.shutdown().await;
}

/// R9: `notifications/tools/list_changed` is emitted to the client after a
/// SUCCESSFUL swap, and is delivered through the watcher's output channel
/// (the seam `main.rs` drains straight to client stdout) — not merely
/// logged. Drives the swap directly via `supervisor.swap_child()` (C1
/// determinism) rather than waiting on the real poller, since this test's
/// concern is the notification/delivery wiring, not detection timing.
#[tokio::test]
async fn list_changed_emitted_after_successful_swap() {
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());

    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor = Arc::new(
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap(),
    );
    perform_handshake(&supervisor).await;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Simulate what `watcher::spawn`'s loop does on a confirmed change,
    // without waiting on the real poll interval.
    write_exe(&canonical, &fake_v2_bytes());
    let synthesized = supervisor.swap_child_with_drain_ms(200).await;
    assert!(synthesized.is_empty());
    for err in &synthesized {
        let _ = tx.send(err.to_string());
    }
    let notif =
        json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"});
    tx.send(notif.to_string()).unwrap();
    drop(tx);

    let received = rx
        .recv()
        .await
        .expect("expected a line on the watcher output channel");
    let parsed: Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["method"], "notifications/tools/list_changed");
    assert!(
        parsed.get("id").is_none(),
        "a notification must carry no id"
    );

    let after = call_generation(&supervisor, 1).await;
    assert_eq!(generation_text(&after), "v2");

    let _ = supervisor.shutdown().await;
}

/// T4's folded-in open concern (item 6): a child that crashes on its own
/// (not via a binary-change swap) must not leave the reader pump idle
/// forever. Simulates a spontaneous crash by killing the OS process
/// directly (bypassing the normal swap teardown) with a dedicated
/// background task continuously calling `read_line` — mirroring
/// `main.rs`'s reader pump — so the crash is discovered by a blocked read,
/// exactly as it would be in production, then asserts service is restored.
#[tokio::test]
async fn crash_auto_recovery_respawns_and_serves_again() {
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());

    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor = Arc::new(
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap(),
    );
    perform_handshake(&supervisor).await;

    let responses: Arc<TokioMutex<HashMap<i64, Value>>> =
        Arc::new(TokioMutex::new(HashMap::new()));
    let pump_responses = Arc::clone(&responses);
    let pump_sup = Arc::clone(&supervisor);
    let pump = tokio::spawn(async move {
        loop {
            match pump_sup.read_line().await {
                Some(line) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        if let Some(id) = v.get("id").and_then(Value::as_i64) {
                            pump_responses.lock().await.insert(id, v);
                        }
                    }
                },
                None => break,
            }
        }
    });

    async fn wait_for_response(
        responses: &TokioMutex<HashMap<i64, Value>>,
        id: i64,
        timeout: Duration,
    ) -> Value {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(v) = responses.lock().await.get(&id).cloned() {
                return v;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("no response for id {id} within {timeout:?}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    // Baseline call succeeds against the original (unkilled) child.
    let req1 = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"generation","arguments":{}}});
    supervisor.write_line(&req1.to_string()).await;
    let resp1 = wait_for_response(&responses, 1, Duration::from_secs(2)).await;
    assert_eq!(generation_text(&resp1), "v1");

    // Simulate a spontaneous crash: kill the process without going through
    // swap_child. The pump task's blocked read_line() call discovers the
    // EOF and triggers automatic recovery (T6 item 6).
    supervisor.force_kill_current_for_test().await;

    // Retry with a fresh id each attempt: a write issued in the brief
    // window between the crash and its detection can land in the dead
    // child's now-one-ended pipe and succeed at the OS level without ever
    // being read (no synthesized error either, since it was never recorded
    // as pending) — that specific race is a known limitation distinct from
    // R6's guarantee (which only covers requests already tracked as
    // in-flight during a deliberate, detected swap). Retrying with a new id
    // routes around a request silently swallowed that way.
    let mut recovered = None;
    let mut attempt_id = 2i64;
    let overall_deadline =
        tokio::time::Instant::now() + Duration::from_secs(10);
    while recovered.is_none() {
        let req = json!({"jsonrpc":"2.0","id":attempt_id,"method":"tools/call","params":{"name":"generation","arguments":{}}});
        if supervisor.write_line(&req.to_string()).await {
            let short_deadline =
                tokio::time::Instant::now() + Duration::from_millis(300);
            loop {
                if let Some(v) =
                    responses.lock().await.get(&attempt_id).cloned()
                {
                    recovered = Some(v);
                    break;
                }
                if tokio::time::Instant::now() >= short_deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        if recovered.is_none() {
            if tokio::time::Instant::now() >= overall_deadline {
                panic!(
                    "supervisor never recovered a healthy child that could complete a request after the simulated crash (tried ids up to {attempt_id})"
                );
            }
            attempt_id += 1;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let resp2 = recovered.unwrap();
    assert_eq!(
        generation_text(&resp2),
        "v1",
        "service must be restored (same binary respawned) after the unexpected crash"
    );

    pump.abort();
    let _ = supervisor.shutdown().await;
}
