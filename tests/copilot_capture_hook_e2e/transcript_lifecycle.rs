#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn e2e_empty_transcript_skips_capture_and_persists_session_events() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let session_id = "34343434-3434-4343-8343-343434343434";
    let transcript_path = checkout.join("not-yet-flushed.jsonl");
    fs::write(&transcript_path, "").expect("create empty transcript");
    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    for (hook_event_name, prompt) in [
        ("SessionStart", None),
        ("UserPromptSubmit", Some("persist despite empty transcript")),
    ] {
        let mut payload = serde_json::json!({
            "hook_event_name": hook_event_name,
            "session_id": session_id,
            "transcript_path": transcript_path,
        });
        if let Some(prompt) = prompt {
            payload["prompt"] = serde_json::Value::String(prompt.to_owned());
        }
        let output = run_hook_with_payload(&hook_bin, &checkout, payload);
        assert!(
            output.status.success(),
            "{hook_event_name} must skip an empty transcript: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"{}\n");
        assert!(
            !String::from_utf8_lossy(&output.stderr)
                .contains("session capture did not include any turns"),
            "{hook_event_name} must not report an empty transcript as an error"
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
                == Some("persist despite empty transcript")
    }));
}

#[test]
pub(super) fn e2e_lifecycle_only_transcript_skips_empty_turns() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let session_id = "35353535-3535-4353-8353-353535353535";
    let transcript_path = checkout.join("lifecycle-only.jsonl");
    fs::write(
        &transcript_path,
        serde_json::json!({
            "type": "session.start",
            "timestamp": "2026-08-22T00:00:00Z",
            "data": { "sessionId": session_id }
        })
        .to_string(),
    )
    .expect("write lifecycle-only transcript");
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
        "SessionStart must skip lifecycle-only transcript: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"{}\n");
    assert!(
        !String::from_utf8_lossy(&output.stderr)
            .contains("session capture did not include any turns")
    );
}

#[test]
pub(super) fn e2e_missing_transcript_session_start_and_stop_do_not_provision() {
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
    assert!(
        !prompt_checkout.join(".worktrees").exists(),
        "SessionStart must not provision for a missing transcript"
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
            prompt_checkout
                .join(".workflow-tools/session")
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
pub(super) fn e2e_user_prompt_submit_captures_in_main_checkout_without_session_start() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    // Fixed valid UUID: SessionWorkspaceResolver rejects non-UUID session
    // ids, and the isolated tempdir checkout makes a literal id safe to reuse.
    let session_id = "44444444-4444-4444-8444-444444444444";
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, session_id);
    let transcript_path = write_fixture_transcript(&checkout, "missed-start.jsonl", &transcript);
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
    assert!(
        !checkout.join(".worktrees").exists(),
        "UserPromptSubmit must not provision a missed SessionStart"
    );
    assert!(
        checkout
            .join(".workflow-tools/session")
            .join("sessions")
            .join(&session_id)
            .join("session.json")
            .is_file(),
        "capture should persist in the main checkout"
    );
}

#[test]
pub(super) fn e2e_user_prompt_submit_self_heals_a_deleted_main_checkout_registration() {
    let fixture = tempdir().expect("temp fixture dir");
    let checkout = fixture.path().join("checkout");
    create_fixture_checkout(&checkout);
    let session_id = "55555555-5555-4555-8555-555555555555";
    let transcript = local_fixture_a().replace(FIXTURE_SESSION_ID, session_id);
    let transcript_path = write_fixture_transcript(&checkout, "self-heal.jsonl", &transcript);
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
        .join(".workflow-tools/session")
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
