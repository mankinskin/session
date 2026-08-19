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

    let hook_bin = std::env::var("CARGO_BIN_EXE_copilot-stop-hook")
        .or_else(|_| std::env::var("CARGO_BIN_EXE_session-capture-hook"))
        .expect("cargo should expose stop or capture hook binary path for integration tests");

    let output = Command::new(hook_bin)
        .env("MCP_MAIN_CHECKOUT", fixture_dir.path())
        .arg("--transcript-path")
        .arg(&transcript_path)
        .arg("--store-root")
        .arg(&store_root)
        .arg("--trigger")
        .arg("SessionStart")
        .output()
        .expect("run copilot hook binary");

    assert!(
        output.status.success(),
        "copilot hook binary failed: stdout={} stderr={}",
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
}

#[test]
fn e2e_stop_hook_script_persists_fixture_from_nested_workspace_cwd() {
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
        .expect("stop hook should emit valid JSON");
    assert!(
        payload.get("decision").is_none(),
        "stop-hook observability output must not alter hook control flow"
    );
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
        "stop hook should persist fixture transcript into the temp store",
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
