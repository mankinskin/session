use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{
        Command,
        Stdio,
    },
};

mod common;

use session_api::{
    PersistedSessionEvents,
    SessionStoreConfig,
    copilot_payload_from_transcript_path,
};
use tempfile::tempdir;

use common::fixture_harness::{
    FIXTURE_SESSION_ID,
    LOCAL_FIXTURE_SESSION_ID,
    ScriptWorkspaceFixture,
    find_cargo_bin,
    local_fixture_a,
    local_fixture_scenarios,
    repo_root_from_manifest,
    shell_single_quote,
    unique_suffix,
    write_fixture_transcript,
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
    fs::create_dir_all(path.join(".session"))
        .expect("create fixture session store");
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
fn e2e_session_start_provisions_and_captures_a_fresh_session() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let session_id = "11111111-1111-4111-8111-111111111111";
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, &session_id);
    let transcript_path =
        write_fixture_transcript(&checkout, "fresh-session.jsonl", &transcript);
    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let output = run_hook_with_payload(
        &hook_bin,
        &checkout,
        serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": session_id,
            "transcript_path": transcript_path,
        }),
    );

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let worktree = checkout.join(".worktrees").join(session_id).join("session");
    assert!(
        worktree.is_dir(),
        "fresh SessionStart must provision a worktree; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        worktree
            .join(".session")
            .join("sessions")
            .join(&session_id)
            .is_dir(),
        "fresh SessionStart must capture a session record in its worktree"
    );
    assert!(
        checkout
            .join(".session")
            .join("sessions")
            .join(session_id)
            .join("session.json")
            .is_file(),
        "fresh SessionStart must register the session in the main checkout store"
    );

    let prompt = "Persist this submitted prompt.";
    let output = run_hook_with_payload(
        &hook_bin,
        &checkout,
        serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": session_id,
            "transcript_path": transcript_path,
            "prompt": prompt,
        }),
    );
    assert!(
        output.status.success(),
        "UserPromptSubmit capture failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events: PersistedSessionEvents = serde_json::from_str(
        &fs::read_to_string(
            worktree
                .join(".session")
                .join("sessions")
                .join(session_id)
                .join("events.json"),
        )
        .expect("read persisted hook events"),
    )
    .expect("deserialize persisted hook events");
    assert!(events.events.iter().any(|event| {
        event.event_type.as_deref() == Some("UserPromptSubmit")
            && event
                .data_json
                .as_ref()
                .and_then(|data| data.get("prompt"))
                .and_then(serde_json::Value::as_str)
                == Some(prompt)
    }));

    for hook_event_name in ["SubagentStart", "SubagentStop"] {
        let output = run_hook_with_payload(
            &hook_bin,
            &checkout,
            serde_json::json!({
                "hook_event_name": hook_event_name,
                "session_id": session_id,
                "transcript_path": transcript_path,
                "timestamp": "2026-08-14T12:00:00Z",
                "agent_id": "agent-42",
                "agent_type": "Implement Agent",
                "stop_hook_active": hook_event_name == "SubagentStop",
            }),
        );
        assert!(
            output.status.success(),
            "{hook_event_name} capture failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let events: PersistedSessionEvents = serde_json::from_str(
        &fs::read_to_string(
            worktree
                .join(".session")
                .join("sessions")
                .join(session_id)
                .join("events.json"),
        )
        .expect("read lifecycle hook events"),
    )
    .expect("deserialize lifecycle hook events");
    for hook_event_name in ["SubagentStart", "SubagentStop"] {
        assert!(events.events.iter().any(|event| {
            event.event_type.as_deref() == Some(hook_event_name)
                && event
                    .data_json
                    .as_ref()
                    .and_then(|data| data.get("agent_id"))
                    .and_then(serde_json::Value::as_str)
                    == Some("agent-42")
                && event
                    .data_json
                    .as_ref()
                    .and_then(|data| data.get("agent_type"))
                    .and_then(serde_json::Value::as_str)
                    == Some("Implement Agent")
        }));
    }
    assert_eq!(output.stdout, b"{}\n");
}

#[test]
fn e2e_session_start_registers_in_main_checkout_without_disturbing_other_sessions()
 {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let decoy = checkout
        .join(".session")
        .join("sessions")
        .join("different-session")
        .join("session.json");
    fs::create_dir_all(decoy.parent().expect("decoy parent"))
        .expect("create decoy session directory");
    fs::write(&decoy, "{\"session_id\":\"different-session\"}\n")
        .expect("write decoy session record");
    let session_id = "22222222-2222-4222-8222-222222222222";
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, session_id);
    let transcript_path = write_fixture_transcript(
        &checkout,
        "anchor-store-snapshot.jsonl",
        &transcript,
    );
    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let output = run_hook_with_payload(
        &hook_bin,
        &checkout,
        serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": session_id,
            "transcript_path": transcript_path,
        }),
    );

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Ticket 842d74cb D1: the main checkout is the authoritative
    // session-to-worktree registry, so SessionStart seeds a minimal
    // registration record there, but must not touch an unrelated session's
    // existing record.
    assert!(
        checkout
            .join(".session")
            .join("sessions")
            .join(session_id)
            .join("session.json")
            .is_file(),
        "SessionStart should register the session in the main checkout store"
    );
    assert_eq!(
        fs::read_to_string(&decoy).expect("read decoy session record"),
        "{\"session_id\":\"different-session\"}\n"
    );
}

#[test]
fn e2e_stop_does_not_provision_a_fresh_session() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let session_id = format!("fresh-session-{}", unique_suffix());
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, &session_id);
    let transcript_path =
        write_fixture_transcript(&checkout, "fresh-session.jsonl", &transcript);
    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let output = run_hook_with_payload(
        &hook_bin,
        &checkout,
        serde_json::json!({
            "hook_event_name": "Stop",
            "session_id": session_id,
            "transcript_path": transcript_path,
        }),
    );

    assert!(output.status.success());
    assert!(
        !checkout.join(".worktrees").exists(),
        "Stop must not provision a worktree"
    );
}

#[test]
fn e2e_missing_transcript_session_start_provisions_but_stop_does_not() {
    let fixture = tempdir().expect("temp fixture dir");
    let prompt_checkout = fixture.path().join("prompt-checkout");
    create_fixture_checkout(&prompt_checkout);
    let prompt_session_id = "33333333-3333-4333-8333-333333333333";
    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let output = run_hook_with_payload(
        &hook_bin,
        &prompt_checkout,
        serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": prompt_session_id,
            "transcript_path": prompt_checkout.join("missing.jsonl"),
        }),
    );

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let worktree = prompt_checkout
        .join(".worktrees")
        .join(&prompt_session_id)
        .join("session");
    assert!(
        worktree.is_dir(),
        "SessionStart must provision despite a missing transcript; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let prompt = "Preserve this prompt before transcript flush.";
    let output = run_hook_with_payload(
        &hook_bin,
        &prompt_checkout,
        serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": prompt_session_id,
            "transcript_path": prompt_checkout.join("missing.jsonl"),
            "prompt": prompt,
        }),
    );
    assert!(output.status.success());
    let events: PersistedSessionEvents = serde_json::from_str(
        &fs::read_to_string(
            worktree
                .join(".session")
                .join("sessions")
                .join(prompt_session_id)
                .join("events.json"),
        )
        .expect("read prompt event sidecar"),
    )
    .expect("deserialize prompt event sidecar");
    assert!(events.events.iter().any(|event| {
        event
            .data_json
            .as_ref()
            .and_then(|data| data.get("prompt"))
            .and_then(serde_json::Value::as_str)
            == Some(prompt)
    }));

    let stop_checkout = fixture.path().join("stop-checkout");
    create_fixture_checkout(&stop_checkout);
    let stop_session_id = format!("missing-transcript-{}", unique_suffix());
    let output = run_hook_with_payload(
        &hook_bin,
        &stop_checkout,
        serde_json::json!({
            "hook_event_name": "Stop",
            "session_id": stop_session_id,
            "transcript_path": stop_checkout.join("missing.jsonl"),
        }),
    );

    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}\n");
    assert!(
        !stop_checkout.join(".worktrees").exists(),
        "Stop must not provision when the transcript is missing"
    );
}

#[test]
fn e2e_user_prompt_submit_lazily_provisions_a_missed_session_start() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    // Fixed valid UUID: SessionWorkspaceResolver rejects non-UUID session
    // ids, and the isolated tempdir checkout makes a literal id safe to reuse.
    let session_id = "44444444-4444-4444-8444-444444444444";
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, session_id);
    let transcript_path =
        write_fixture_transcript(&checkout, "missed-start.jsonl", &transcript);
    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    // No SessionStart ever ran for this session id; UserPromptSubmit is the
    // first hook event it sees.
    let output = run_hook_with_payload(
        &hook_bin,
        &checkout,
        serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": session_id,
            "transcript_path": transcript_path,
            "prompt": "recover from a missed SessionStart",
        }),
    );

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let worktree = checkout.join(".worktrees").join(&session_id).join("session");
    assert!(
        worktree.is_dir(),
        "UserPromptSubmit must lazily provision a missed SessionStart; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        worktree
            .join(".session")
            .join("sessions")
            .join(&session_id)
            .join("session.json")
            .is_file(),
        "capture should persist once lazy provisioning resolves a store root"
    );
}

#[test]
fn e2e_user_prompt_submit_self_heals_a_deleted_main_checkout_registration() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let session_id = "55555555-5555-4555-8555-555555555555";
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, session_id);
    let transcript_path =
        write_fixture_transcript(&checkout, "self-heal.jsonl", &transcript);
    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let output = run_hook_with_payload(
        &hook_bin,
        &checkout,
        serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": session_id,
            "transcript_path": transcript_path,
        }),
    );
    assert!(output.status.success());
    let main_record = checkout
        .join(".session")
        .join("sessions")
        .join(session_id)
        .join("session.json");
    assert!(
        main_record.is_file(),
        "SessionStart should register the session in the main checkout store"
    );

    // Simulate an operator deleting the main-checkout registration (or it
    // never having been written by an older binary) while the worktree
    // itself, and its own nested store, remain intact.
    fs::remove_dir_all(main_record.parent().expect("main record parent"))
        .expect("delete main-checkout registration");
    assert!(!main_record.is_file());

    let output = run_hook_with_payload(
        &hook_bin,
        &checkout,
        serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": session_id,
            "transcript_path": transcript_path,
            "prompt": "trigger self-heal",
        }),
    );
    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        main_record.is_file(),
        "UserPromptSubmit must self-heal the deleted main-checkout registration; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn e2e_parses_fixture_transcript_payload() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path = write_fixture_transcript(
        fixture_dir.path(),
        "fixture-a.jsonl",
        local_fixture_a(),
    );

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
    let transcript_path = write_fixture_transcript(
        fixture_dir.path(),
        "fixture-a.jsonl",
        local_fixture_a(),
    );

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

    let config = SessionStoreConfig::new(&store_root, "default");
    let record = config
        .read_session(FIXTURE_SESSION_ID)
        .expect("persisted session should be readable from temp store");

    assert!(!record.turns.is_empty());
    assert_eq!(record.session_id, FIXTURE_SESSION_ID);
    assert_eq!(record.metadata.workspace_slug, "default");
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

#[test]
fn e2e_session_start_with_external_store_does_not_provision_cwd_checkout() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path = write_fixture_transcript(
        fixture_dir.path(),
        "fixture-a.jsonl",
        local_fixture_a(),
    );
    let store_dir = tempdir().expect("external session store tempdir");
    let store_root = store_dir.path().join("session-store");
    fs::create_dir_all(&store_root).expect("create external session store");

    let cwd_checkout = tempdir().expect("temporary cwd checkout");
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "hook@example.com"],
        vec!["config", "user.name", "hook"],
        vec!["commit", "--quiet", "--allow-empty", "-m", "init"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd_checkout.path())
            .status()
            .expect("run git fixture command");
        assert!(status.success(), "git fixture command should succeed");
    }
    fs::create_dir_all(cwd_checkout.path().join(".session"))
        .expect("create cwd session store");

    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");
    let stdin_payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": FIXTURE_SESSION_ID,
        "transcript_path": transcript_path,
    })
    .to_string();
    let mut child = Command::new(hook_bin)
        .env_remove("MCP_MAIN_CHECKOUT")
        .env("WORKTREE_EAGER_PROVISION", "1")
        .arg("--store-root")
        .arg(&store_root)
        .arg("--from-hook-stdin")
        .current_dir(cwd_checkout.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn session-capture-hook");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin_payload.as_bytes())
        .expect("write hook stdin payload");
    let output = child
        .wait_with_output()
        .expect("wait for session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"{}\n");
    let config = SessionStoreConfig::new(&store_root, "default");
    let record = config.read_session(FIXTURE_SESSION_ID).expect(
        "provisioning diagnostic should remain readable after the hook exits",
    );
    let diagnostic = record
        .metadata
        .provisioning
        .expect("provisioning diagnostic");
    assert_eq!(diagnostic.outcome, "skipped");
    assert_eq!(
        diagnostic.reason.as_deref(),
        Some("external_store_mismatch")
    );
    assert_eq!(diagnostic.hook_event_name, "SessionStart");
    assert!(
        !store_root
            .join("sessions")
            .join(FIXTURE_SESSION_ID)
            .join("provisioning.json")
            .exists(),
        "provisioning.json must not be written under the session store"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("worktree provisioning skipped"),
        "mismatched store should be diagnosed on stderr"
    );
    assert!(
        !cwd_checkout.path().join(".worktrees").exists(),
        "hook must not provision a worktree beneath the unrelated current directory"
    );
}

#[test]
fn e2e_mismatched_store_emits_nonblocking_observability_payload() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path = write_fixture_transcript(
        fixture_dir.path(),
        "fixture-a.jsonl",
        local_fixture_a(),
    );
    let store_root = fixture_dir.path().join("memory-api-store");
    fs::create_dir_all(&store_root).expect("create temp store root");

    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");
    let stdin_payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": FIXTURE_SESSION_ID,
        "transcript_path": transcript_path,
    })
    .to_string();

    let mut child = Command::new(hook_bin)
        .arg("--store-root")
        .arg(&store_root)
        .arg("--from-hook-stdin")
        .env("MCP_MAIN_CHECKOUT", fixture_dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn session-capture-hook");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin_payload.as_bytes())
        .expect("write hook stdin payload");
    let output = child
        .wait_with_output()
        .expect("wait for session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"{}\n");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("worktree provisioning skipped"),
        "mismatched provisioning should be diagnosed on stderr"
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

#[test]
fn e2e_hook_binary_populates_tool_metrics_from_captured_tool_events() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path = write_fixture_transcript(
        fixture_dir.path(),
        "fixture-tool-calls.jsonl",
        TOOL_CALL_TRANSCRIPT,
    );

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
        .arg("Stop")
        .output()
        .expect("run session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let tool_metrics_path = store_root
        .join("sessions")
        .join("fixture-tool-calls")
        .join("tool-metrics.json");
    let raw = fs::read_to_string(&tool_metrics_path)
        .expect("tool-metrics.json should exist once a tool call was captured");
    let summary: serde_json::Value = serde_json::from_str(&raw)
        .expect("tool-metrics.json should be valid json");

    let grep = &summary["tools"]["grep_search"];
    assert_eq!(grep["call_count"], 1);
    assert_eq!(grep["success_count"], 1);
    assert_eq!(
        grep["duration_ms_values"][0], 1500,
        "duration is derived from the start/complete bracket"
    );
}

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

#[test]
fn e2e_val_session_api_tool_metrics_gate_asserts_nonempty_tools_map() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path = write_fixture_transcript(
        fixture_dir.path(),
        "fixture-tool-metrics-gate.jsonl",
        PRODUCER_SHAPED_TOOL_TRANSCRIPT,
    );

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
        .arg("Stop")
        .output()
        .expect("run session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let tool_metrics_path = store_root
        .join("sessions")
        .join("fixture-tool-metrics-gate")
        .join("tool-metrics.json");
    let raw = fs::read_to_string(&tool_metrics_path).expect(
        "tool-metrics.json should exist once a tool call was captured from \
         a producer-shaped transcript",
    );
    let summary: serde_json::Value = serde_json::from_str(&raw)
        .expect("tool-metrics.json should be valid json");

    let tools = summary["tools"]
        .as_object()
        .expect("tools field should be a json object");
    assert!(
        !tools.is_empty(),
        "tools map in {} must be non-empty for a transcript with captured \
         tool execution events; got: {raw}",
        tool_metrics_path.display()
    );
    assert_eq!(tools["read_file"]["call_count"], 1);
}

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
fn e2e_hook_binary_captures_output_chars_from_hook_stdin_tool_response() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path = write_fixture_transcript(
        fixture_dir.path(),
        "fixture-hook-stdin-output.jsonl",
        HOOK_STDIN_TOOL_TRANSCRIPT,
    );

    let store_dir = tempdir().expect("tempdir");
    let store_root = store_dir.path().join("memory-api-store");
    fs::create_dir_all(&store_root).expect("create temp store root");

    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let stdin_payload = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "run_in_terminal",
        "tool_response": "hi\n",
        "tool_use_id": "call-hook-1__vscode-1785422593000",
    })
    .to_string();

    let mut child = Command::new(hook_bin)
        .env("MCP_MAIN_CHECKOUT", fixture_dir.path())
        .arg("--transcript-path")
        .arg(&transcript_path)
        .arg("--store-root")
        .arg(&store_root)
        .arg("--trigger")
        .arg("Stop")
        .arg("--from-hook-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn session-capture-hook");

    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin_payload.as_bytes())
        .expect("write hook stdin payload");

    let output = child
        .wait_with_output()
        .expect("wait for session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let tool_metrics_path = store_root
        .join("sessions")
        .join("fixture-hook-stdin-output")
        .join("tool-metrics.json");
    let raw = fs::read_to_string(&tool_metrics_path)
        .expect("tool-metrics.json should exist once a tool call was captured");
    let summary: serde_json::Value = serde_json::from_str(&raw)
        .expect("tool-metrics.json should be valid json");

    let output_sizes = summary["tools"]["run_in_terminal"]["output_char_sizes"]
        .as_array()
        .expect("output_char_sizes should be a json array");
    assert_eq!(
        output_sizes.as_slice(),
        &[serde_json::Value::from(3)],
        "output size should reflect the hook stdin tool_response char count (\"hi\\n\" = 3 chars); got: {raw}"
    );

    let output_source = summary["tools"]["run_in_terminal"]["output_source"]
        .as_array()
        .expect("output_source should be a json array");
    assert_eq!(
        output_source.as_slice(),
        &[serde_json::Value::from("hook_payload")],
        "output_source should attribute the size to the hook stdin payload; got: {raw}"
    );
}

/// Ticket 44119807 AC1 real-capture fix: the live PostToolUse hook payload's
/// `tool_response` is observed to always be sent empty, so `output_chars`
/// must instead come from the on-disk `chat-session-resources` spill-file
/// convention, keyed by `<transcript_path>/../chat-session-resources/
/// <session_id>/<tool_use_id>/content.txt`.
#[test]
fn e2e_hook_binary_captures_output_chars_from_spill_file_when_hook_payload_empty()
 {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let chat_root = fixture_dir.path().join("GitHub.copilot-chat");
    let transcripts_dir = chat_root.join("transcripts");
    fs::create_dir_all(&transcripts_dir)
        .expect("create fixture transcripts dir");

    let transcript_path = write_fixture_transcript(
        &transcripts_dir,
        "fixture-spill-output.jsonl",
        HOOK_STDIN_SPILL_TRANSCRIPT,
    );

    let spill_dir = chat_root
        .join("chat-session-resources")
        .join("fixture-spill-output")
        .join("call-spill-1__vscode-1785422594630");
    fs::create_dir_all(&spill_dir).expect("create fixture spill dir");
    fs::write(spill_dir.join("content.txt"), "spilled tool output")
        .expect("write fixture spill content.txt");

    let store_dir = tempdir().expect("tempdir");
    let store_root = store_dir.path().join("memory-api-store");
    fs::create_dir_all(&store_root).expect("create temp store root");

    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let stdin_payload = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "run_in_terminal",
        "tool_response": "",
        "tool_use_id": "call-spill-1__vscode-1785422594630",
        "session_id": "fixture-spill-output",
    })
    .to_string();

    let mut child = Command::new(hook_bin)
        .env("MCP_MAIN_CHECKOUT", fixture_dir.path())
        .arg("--transcript-path")
        .arg(&transcript_path)
        .arg("--store-root")
        .arg(&store_root)
        .arg("--trigger")
        .arg("Stop")
        .arg("--from-hook-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn session-capture-hook");

    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin_payload.as_bytes())
        .expect("write hook stdin payload");

    let output = child
        .wait_with_output()
        .expect("wait for session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let tool_metrics_path = store_root
        .join("sessions")
        .join("fixture-spill-output")
        .join("tool-metrics.json");
    let raw = fs::read_to_string(&tool_metrics_path)
        .expect("tool-metrics.json should exist once a tool call was captured");
    let summary: serde_json::Value = serde_json::from_str(&raw)
        .expect("tool-metrics.json should be valid json");

    let output_sizes = summary["tools"]["run_in_terminal"]["output_char_sizes"]
        .as_array()
        .expect("output_char_sizes should be a json array");
    assert_eq!(
        output_sizes.as_slice(),
        &[serde_json::Value::from(19)],
        "output size should reflect the spill file's char count (\"spilled tool output\" = 19 chars); got: {raw}"
    );

    let output_source = summary["tools"]["run_in_terminal"]["output_source"]
        .as_array()
        .expect("output_source should be a json array");
    assert_eq!(
        output_source.as_slice(),
        &[serde_json::Value::from("spill_file")],
        "output_source should attribute the size to the on-disk spill file; got: {raw}"
    );
}

#[test]
fn e2e_capture_hook_script_persists_fixture_from_nested_workspace_cwd() {
    let repo_root = repo_root();
    let script_source =
        repo_root.join("tools/agent-hooks/session-capture-stop.sh");
    assert!(
        script_source.is_file(),
        "missing hook script under repo root"
    );

    let fixture_text =
        include_str!("fixtures/capture_hook_workspace_e2e.jsonl");
    let suffix = unique_suffix();
    let workspace_fixture = ScriptWorkspaceFixture::new(&script_source);
    let fixture_root = &workspace_fixture.root;
    let fixture_store_root = &workspace_fixture.store_root;
    let fixture_hook_bin = fixture_root.join("session-capture-hook.exe");
    fs::copy(
        std::env::var("CARGO_BIN_EXE_session-capture-hook").expect(
            "cargo should expose session-capture-hook binary path for integration tests",
        ),
        &fixture_hook_bin,
    )
    .expect("copy session-capture-hook binary into shell fixture");

    let rel_transcript_path =
        PathBuf::from("transcripts").join("copilot.jsonl");
    let abs_transcript_path =
        workspace_fixture.transcript_path("copilot.jsonl");

    let session_id = format!("{LOCAL_FIXTURE_SESSION_ID}-{suffix}");

    let transcript_text =
        fixture_text.replace(LOCAL_FIXTURE_SESSION_ID, &session_id);
    fs::write(&abs_transcript_path, transcript_text)
        .expect("write transcript fixture");

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

    let manifest_path = repo_root
        .join("workflow-tools/session/Cargo.toml")
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
            eprintln!(
                "skipping e2e shell-hook test: bash not available on PATH"
            );
            return;
        },
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
        eprintln!(
            "skipping e2e shell-hook test: bash subprocess could not resolve cargo binary"
        );
        return;
    }

    assert!(
        output.status.success(),
        "session-capture-stop.sh failed: stdout={stdout} stderr={stderr}"
    );
    let payload: serde_json::Value = serde_json::from_str(stdout.trim())
        .expect("capture hook should emit valid JSON");
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
        .join(".session")
        .join("sessions")
        .join(&session_id)
        .join("session.json");
    assert!(
        !leaked_root_manifest.is_file(),
        "hook leaked test artifact into root store: {}",
        leaked_root_manifest.display()
    );

    let config = SessionStoreConfig::new(&fixture_store_root, "default");
    let record = config.read_session(&session_id).expect(
        "capture hook should persist fixture transcript into the temp store",
    );

    assert_eq!(record.session_id, session_id);
    assert_eq!(record.metadata.workspace_slug, "default");
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
        let payload = copilot_payload_from_transcript_path(
            &path,
            "default",
            Some("e2e-scan".to_string()),
        )
        .expect("local deterministic fixture transcript should parse");

        assert_eq!(payload.session_id, expected_session_id);
        assert!(
            !payload.messages.is_empty(),
            "expected visible messages for fixture {name}"
        );
    }
}
