#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn e2e_session_start_captures_a_fresh_session_in_main_checkout() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let session_id = "11111111-1111-4111-8111-111111111111";
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, &session_id);
    let transcript_path = write_fixture_transcript(&checkout, "fresh-session.jsonl", &transcript);
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
    assert!(
        !checkout.join(".worktrees").exists(),
        "fresh SessionStart must not provision a worktree"
    );
    assert!(
        checkout
            .join(".workflow-tools/session")
            .join("sessions")
            .join(&session_id)
            .is_dir(),
        "fresh SessionStart must capture a session record in its worktree"
    );
    assert!(
        checkout
            .join(".workflow-tools/session")
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
            checkout
                .join(".workflow-tools/session")
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
    let main_events: PersistedSessionEvents = serde_json::from_str(
        &fs::read_to_string(
            checkout
                .join(".workflow-tools/session")
                .join("sessions")
                .join(session_id)
                .join("events.json"),
        )
        .expect("read main-checkout hook events"),
    )
    .expect("deserialize main-checkout hook events");
    assert!(main_events.events.iter().any(|event| {
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
            checkout
                .join(".workflow-tools/session")
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
pub(super) fn e2e_session_start_captures_in_main_checkout_when_provisioning_fails() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let session_id = "12121212-1212-4121-8121-121212121212";
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, session_id);
    let transcript_path =
        write_fixture_transcript(&checkout, "provisioning-failure.jsonl", &transcript);
    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let mut child = Command::new(hook_bin)
        .arg("--from-hook-stdin")
        .current_dir(&checkout)
        .env("MCP_MAIN_CHECKOUT", &checkout)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn session-capture-hook");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(
            serde_json::json!({
                "hook_event_name": "SessionStart",
                "session_id": session_id,
                "transcript_path": transcript_path,
            })
            .to_string()
            .as_bytes(),
        )
        .expect("write hook stdin payload");
    let output = child
        .wait_with_output()
        .expect("wait for session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook should fall back to the main checkout: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !checkout.join(".worktrees").exists(),
        "unassigned sessions must not create a worktree"
    );
    let record = SessionStoreConfig::new(checkout.join(".workflow-tools/session"))
        .read_session(session_id)
        .expect("capture should persist in the main checkout store");
    assert_eq!(
        record.metadata.provisioning.map(|value| value.outcome),
        Some("skipped".to_string())
    );
}

#[test]
pub(super) fn e2e_session_start_registers_in_main_checkout_without_disturbing_other_sessions() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let decoy = checkout
        .join(".workflow-tools/session")
        .join("sessions")
        .join("different-session")
        .join("session.json");
    fs::create_dir_all(decoy.parent().expect("decoy parent"))
        .expect("create decoy session directory");
    fs::write(&decoy, "{\"session_id\":\"different-session\"}\n")
        .expect("write decoy session record");
    let session_id = "22222222-2222-4222-8222-222222222222";
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, session_id);
    let transcript_path =
        write_fixture_transcript(&checkout, "anchor-store-snapshot.jsonl", &transcript);
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
            .join(".workflow-tools/session")
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
pub(super) fn e2e_stop_does_not_provision_a_fresh_session() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let session_id = format!("fresh-session-{}", unique_suffix());
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, &session_id);
    let transcript_path = write_fixture_transcript(&checkout, "fresh-session.jsonl", &transcript);
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
