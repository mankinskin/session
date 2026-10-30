use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

mod common;

use session_api::{
    PersistedSessionEvents, SessionStoreConfig, copilot_payload_from_transcript_path,
};
use tempfile::tempdir;

use common::fixture_harness::{
    FIXTURE_SESSION_ID, LOCAL_FIXTURE_SESSION_ID, ScriptWorkspaceFixture,
    context_engine_hook_script, find_cargo_bin, local_fixture_a, local_fixture_scenarios,
    repo_root_from_manifest, shell_single_quote, unique_suffix, write_fixture_transcript,
};

fn repo_root() -> PathBuf {
    repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"))
}

fn create_fixture_checkout(path: &std::path::Path) {
    fs::create_dir_all(path).expect("create fixture checkout");
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "hook@example.com"],
        vec!["config", "user.name", "hook"],
        vec!["commit", "--quiet", "--allow-empty", "-m", "init"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .expect("run git fixture command");
        assert!(status.success(), "git fixture command should succeed");
    }
    fs::create_dir_all(path.join(".workflow-tools/session")).expect("create fixture session store");
}

fn run_hook_with_payload(
    hook_bin: &str,
    checkout: &std::path::Path,
    payload: serde_json::Value,
) -> std::process::Output {
    let mut child = Command::new(hook_bin)
        .arg("--from-hook-stdin")
        .current_dir(checkout)
        .env("MCP_MAIN_CHECKOUT", checkout)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn session-capture-hook");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("write hook stdin payload");
    child
        .wait_with_output()
        .expect("wait for session-capture-hook")
}

#[test]
fn e2e_parses_fixture_transcript_payload() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path =
        write_fixture_transcript(fixture_dir.path(), "fixture-a.jsonl", local_fixture_a());

    let payload = copilot_payload_from_transcript_path(
        &transcript_path,
        "default",
        Some("e2e-parse".to_string()),
    )
    .expect("fixture transcript should parse into payload");

    assert_eq!(payload.session_id, FIXTURE_SESSION_ID);
    assert!(!payload.messages.is_empty());
}

#[test]
fn e2e_hook_binary_persists_fixture_transcript() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path =
        write_fixture_transcript(fixture_dir.path(), "fixture-a.jsonl", local_fixture_a());

    let store_dir = tempdir().expect("tempdir");
    let store_root = store_dir.path().join("memory-api-store");
    fs::create_dir_all(&store_root).expect("create temp store root");

    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let output = Command::new(hook_bin)
        .env("MCP_MAIN_CHECKOUT", fixture_dir.path())
        .arg("--transcript-path")
        .arg(&transcript_path)
        .arg("--store-root")
        .arg(&store_root)
        .arg("--trigger")
        .arg("SessionStart")
        .output()
        .expect("run session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let config = SessionStoreConfig::new(&store_root);
    let record = config
        .read_session(FIXTURE_SESSION_ID)
        .expect("persisted session should be readable from temp store");

    assert!(!record.turns.is_empty());
    assert_eq!(record.session_id, FIXTURE_SESSION_ID);
    assert_eq!(
        record.metadata.workspace_path,
        config.workspace_path.as_str()
    );
    assert_eq!(record.metadata.trigger.as_deref(), Some("SessionStart"));

    // A transcript with no tool execution must not leave an empty sidecar.
    let tool_metrics_path = store_root
        .join("sessions")
        .join(FIXTURE_SESSION_ID)
        .join("tool-metrics.json");
    assert!(
        !tool_metrics_path.exists(),
        "tool-metrics.json must be created lazily, only when a tool call was captured"
    );
}

const TOOL_CALL_TRANSCRIPT: &str = concat!(
    r#"{"id":"evt-start-t","type":"session.start","timestamp":"2026-06-02T23:06:54.049Z","data":{"sessionId":"fixture-tool-calls","producer":"copilot-agent","startTime":"2026-06-02T23:06:54.049Z"}}"#,
    "\n",
    r#"{"id":"evt-user-t","type":"user.message","timestamp":"2026-06-02T23:07:00.000Z","data":{"content":"run a search"}}"#,
    "\n",
    r#"{"id":"evt-tool-start","type":"tool.execution_start","timestamp":"2026-06-02T23:07:01.000Z","data":{"toolCallId":"call-1","toolName":"grep_search","arguments":{"query":"needle"}}}"#,
    "\n",
    r#"{"id":"evt-tool-complete","type":"tool.execution_complete","timestamp":"2026-06-02T23:07:02.500Z","data":{"toolCallId":"call-1","success":true}}"#,
    "\n",
    r#"{"id":"evt-assistant-t","type":"assistant.message","timestamp":"2026-06-02T23:07:05.000Z","data":{"content":"done"}}"#,
    "\n",
);

/// Regression fixture for `val-session-api-tool-metrics-e2e` (ticket
/// `ce7b7bde`). This is the recurrence guardrail: unit tests in
/// `tool_metrics.rs` hand-construct `role: Tool` turns the real Copilot
/// producer never emits, so a green unit-test count proved nothing about the
/// artifact the hook binary actually writes. This test drives the real
/// `session-capture-hook` binary end-to-end and reads back the persisted
/// `tool-metrics.json`.
const PRODUCER_SHAPED_TOOL_TRANSCRIPT: &str = concat!(
    r#"{"id":"evt-start-g","type":"session.start","timestamp":"2026-07-30T10:00:00.000Z","data":{"sessionId":"fixture-tool-metrics-gate","producer":"copilot-agent","startTime":"2026-07-30T10:00:00.000Z"}}"#,
    "\n",
    r#"{"id":"evt-user-g","type":"user.message","timestamp":"2026-07-30T10:00:01.000Z","data":{"content":"read a file"}}"#,
    "\n",
    r#"{"id":"evt-tool-start-g","type":"tool.execution_start","timestamp":"2026-07-30T10:00:02.000Z","data":{"toolCallId":"call-gate-1","toolName":"read_file","arguments":{"path":"README.md"}}}"#,
    "\n",
    r#"{"id":"evt-tool-complete-g","type":"tool.execution_complete","timestamp":"2026-07-30T10:00:03.250Z","data":{"toolCallId":"call-gate-1","success":true}}"#,
    "\n",
    r#"{"id":"evt-assistant-g","type":"assistant.message","timestamp":"2026-07-30T10:00:04.000Z","data":{"content":"done"}}"#,
    "\n",
);

/// AC3 of ticket `44119807` (T2): drives the real `session-capture-hook`
/// binary with `--from-hook-stdin`, feeding a PostToolUse-shaped stdin
/// payload whose `tool_response` carries real output text and whose
/// `tool_use_id` matches the transcript's `toolCallId`. Asserts the
/// persisted `tool-metrics.json` records a non-zero output size sourced
/// from the hook payload, in the style of
/// `e2e_hook_binary_populates_tool_metrics_from_captured_tool_events`.
const HOOK_STDIN_TOOL_TRANSCRIPT: &str = concat!(
    r#"{"id":"evt-start-h","type":"session.start","timestamp":"2026-07-30T11:00:00.000Z","data":{"sessionId":"fixture-hook-stdin-output","producer":"copilot-agent","startTime":"2026-07-30T11:00:00.000Z"}}"#,
    "\n",
    r#"{"id":"evt-user-h","type":"user.message","timestamp":"2026-07-30T11:00:01.000Z","data":{"content":"run a command"}}"#,
    "\n",
    r#"{"id":"evt-tool-start-h","type":"tool.execution_start","timestamp":"2026-07-30T11:00:02.000Z","data":{"toolCallId":"call-hook-1","toolName":"run_in_terminal","arguments":{"command":"echo hi"}}}"#,
    "\n",
    r#"{"id":"evt-tool-complete-h","type":"tool.execution_complete","timestamp":"2026-07-30T11:00:03.000Z","data":{"toolCallId":"call-hook-1","success":true}}"#,
    "\n",
    r#"{"id":"evt-assistant-h","type":"assistant.message","timestamp":"2026-07-30T11:00:04.000Z","data":{"content":"done"}}"#,
    "\n",
);

const HOOK_STDIN_SPILL_TRANSCRIPT: &str = concat!(
    r#"{"id":"evt-start-s","type":"session.start","timestamp":"2026-07-30T11:00:00.000Z","data":{"sessionId":"fixture-spill-output","producer":"copilot-agent","startTime":"2026-07-30T11:00:00.000Z"}}"#,
    "\n",
    r#"{"id":"evt-user-s","type":"user.message","timestamp":"2026-07-30T11:00:01.000Z","data":{"content":"run a command"}}"#,
    "\n",
    r#"{"id":"evt-tool-start-s","type":"tool.execution_start","timestamp":"2026-07-30T11:00:02.000Z","data":{"toolCallId":"call-spill-1","toolName":"run_in_terminal","arguments":{"command":"echo hi"}}}"#,
    "\n",
    r#"{"id":"evt-tool-complete-s","type":"tool.execution_complete","timestamp":"2026-07-30T11:00:03.000Z","data":{"toolCallId":"call-spill-1","success":true}}"#,
    "\n",
    r#"{"id":"evt-assistant-s","type":"assistant.message","timestamp":"2026-07-30T11:00:04.000Z","data":{"content":"done"}}"#,
    "\n",
);

#[test]
fn e2e_capture_hook_script_persists_fixture_from_nested_workspace_cwd() {
    let repo_root = repo_root();
    let Some(script_source) = context_engine_hook_script("session-capture-stop.sh") else {
        eprintln!(
            "skipping cross-repository hook test: set CONTEXT_ENGINE_ROOT or provide a sibling context-engine checkout"
        );
        return;
    };

    let fixture_text = include_str!("fixtures/capture_hook_workspace_e2e.jsonl");
    let suffix = unique_suffix();
    let workspace_fixture = ScriptWorkspaceFixture::new(&script_source);
    let fixture_root = &workspace_fixture.root;
    let fixture_store_root = &workspace_fixture.store_root;
    let fixture_hook_bin = fixture_root.join("session-capture-hook.exe");
    fs::copy(
        std::env::var("CARGO_BIN_EXE_session-capture-hook")
            .expect("cargo should expose session-capture-hook binary path for integration tests"),
        &fixture_hook_bin,
    )
    .expect("copy session-capture-hook binary into shell fixture");

    let rel_transcript_path = PathBuf::from("transcripts").join("copilot.jsonl");
    let abs_transcript_path = workspace_fixture.transcript_path("copilot.jsonl");

    let session_id = format!("{LOCAL_FIXTURE_SESSION_ID}-{suffix}");

    let transcript_text = fixture_text.replace(LOCAL_FIXTURE_SESSION_ID, &session_id);
    fs::write(&abs_transcript_path, transcript_text).expect("write transcript fixture");

    let payload = serde_json::json!({
        "transcript_path": rel_transcript_path,
        "hook_event_name": "SessionStart",
        "session_id": &session_id,
    })
    .to_string();

    let Some(cargo_bin) = find_cargo_bin() else {
        eprintln!(
            "skipping e2e shell-hook test: unable to locate cargo binary for bash subprocess"
        );
        return;
    };

    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("Cargo.toml")
        .to_string_lossy()
        .replace("\\\\?\\", "")
        .replace('\\', "/");
    let script_path_shell = ScriptWorkspaceFixture::script_path_shell();
    let command_line = format!(
        "SESSION_CAPTURE_STORE_ROOT={} SESSION_CAPTURE_MANIFEST_PATH={} SESSION_CAPTURE_CARGO_BIN={} SESSION_CAPTURE_HOOK_BIN={} bash {}",
        shell_single_quote("session-store"),
        shell_single_quote(&manifest_path),
        shell_single_quote(&cargo_bin),
        shell_single_quote("./session-capture-hook.exe"),
        shell_single_quote(&script_path_shell)
    );

    let mut command = Command::new("bash");
    command
        .arg("-lc")
        .arg(command_line)
        .current_dir(fixture_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    workspace_fixture.configure_hook_command(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping e2e shell-hook test: bash not available on PATH");
            return;
        }
        Err(error) => panic!("failed to spawn bash for hook test: {error}"),
    };

    child
        .stdin
        .as_mut()
        .expect("stdin should be piped")
        .write_all(payload.as_bytes())
        .expect("write hook payload to stdin");

    let output = child.wait_with_output().expect("wait for hook process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() && stderr.contains("cargo binary not found") {
        eprintln!("skipping e2e shell-hook test: bash subprocess could not resolve cargo binary");
        return;
    }

    assert!(
        output.status.success(),
        "session-capture-stop.sh failed: stdout={stdout} stderr={stderr}"
    );
    let payload: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("capture hook should emit valid JSON");
    assert_eq!(payload, serde_json::json!({}));
    assert!(
        !stderr.contains("skip: transcript not found"),
        "hook skipped transcript unexpectedly: stdout={stdout} stderr={stderr}"
    );

    let session_manifest = fixture_store_root
        .join("sessions")
        .join(&session_id)
        .join("session.json");
    assert!(
        session_manifest.is_file(),
        "session manifest missing at {} (stdout={} stderr={})",
        session_manifest.display(),
        stdout,
        stderr
    );

    let leaked_root_manifest = repo_root
        .join(".workflow-tools/session")
        .join("sessions")
        .join(&session_id)
        .join("session.json");
    assert!(
        !leaked_root_manifest.is_file(),
        "hook leaked test artifact into root store: {}",
        leaked_root_manifest.display()
    );

    let config = SessionStoreConfig::new(&fixture_store_root);
    let record = config
        .read_session(&session_id)
        .expect("capture hook should persist fixture transcript into the temp store");

    assert_eq!(record.session_id, session_id);
    assert_eq!(
        record.metadata.workspace_path,
        config.workspace_path.as_str()
    );
    assert_eq!(record.metadata.trigger.as_deref(), Some("SessionStart"));
    assert_eq!(record.turns.len(), 2);
    assert_eq!(
        record.turns[0].content,
        "Persist this transcript from fixture"
    );
    assert_eq!(
        record.turns[1].content,
        "Transcript persisted from fixture."
    );

    let session_dir = fixture_store_root.join("sessions").join(&session_id);
    assert!(session_dir.join("session.json").is_file());
    assert!(session_dir.join("transcript.json").is_file());
    assert!(session_dir.join("events.json").is_file());
}

#[test]
fn e2e_parses_multiple_local_fixture_scenarios() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let fixtures = local_fixture_scenarios();

    for (name, content, expected_session_id) in fixtures {
        let path = write_fixture_transcript(fixture_dir.path(), name, content);
        let payload =
            copilot_payload_from_transcript_path(&path, "default", Some("e2e-scan".to_string()))
                .expect("local deterministic fixture transcript should parse");

        assert_eq!(payload.session_id, expected_session_id);
        assert!(
            !payload.messages.is_empty(),
            "expected visible messages for fixture {name}"
        );
    }
}

#[path = "copilot_capture_hook_e2e/session_start.rs"]
mod session_start;
#[allow(unused_imports)]
pub use session_start::*;

#[path = "copilot_capture_hook_e2e/transcript_lifecycle.rs"]
mod transcript_lifecycle;
#[allow(unused_imports)]
pub use transcript_lifecycle::*;

#[path = "copilot_capture_hook_e2e/tool_metrics.rs"]
mod tool_metrics;
#[allow(unused_imports)]
pub use tool_metrics::*;

#[path = "copilot_capture_hook_e2e/external_store.rs"]
mod external_store;
#[allow(unused_imports)]
pub use external_store::*;
