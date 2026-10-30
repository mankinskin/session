#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn e2e_session_start_with_external_store_does_not_provision_cwd_checkout() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path =
        write_fixture_transcript(fixture_dir.path(), "fixture-a.jsonl", local_fixture_a());
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
    fs::create_dir_all(cwd_checkout.path().join(".workflow-tools/session"))
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
    let config = SessionStoreConfig::new(&store_root);
    let record = config
        .read_session(FIXTURE_SESSION_ID)
        .expect("provisioning diagnostic should remain readable after the hook exits");
    let diagnostic = record
        .metadata
        .provisioning
        .expect("provisioning diagnostic");
    assert_eq!(diagnostic.outcome, "skipped");
    assert_eq!(diagnostic.reason.as_deref(), Some("no_registered_worktree"));
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
        !cwd_checkout.path().join(".worktrees").exists(),
        "hook must not provision a worktree beneath the unrelated current directory"
    );
}

#[test]
pub(super) fn e2e_mismatched_store_emits_nonblocking_observability_payload() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path =
        write_fixture_transcript(fixture_dir.path(), "fixture-a.jsonl", local_fixture_a());
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
    assert!(String::from_utf8_lossy(&output.stderr).is_empty());
}
