//! Hook subprocess to stored event/receipt; fixture stdin is not human approval.
use session_api::requester_decision::Uuid;
use session_api::{
    DecisionKind, PersistedSessionEvents, RequesterChallengeRequest, SessionRuntimeInitRequest,
    SessionStoreConfig,
};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

const SESSION: &str = "55555555-5555-4555-8555-555555555555";
const MISSION: &str = "22222222-2222-4222-8222-222222222222";
const P: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const R: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Fx {
    _dir: tempfile::TempDir,
    checkout: std::path::PathBuf,
    config: SessionStoreConfig,
}
fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let checkout = dir.path().join("checkout");
    fs::create_dir_all(&checkout).unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "h@example.com"],
        vec!["config", "user.name", "h"],
        vec!["commit", "--quiet", "--allow-empty", "-m", "init"],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&checkout)
                .status()
                .unwrap()
                .success()
        );
    }
    let config = SessionStoreConfig::new(checkout.join(".workflow-tools/session"));
    config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(SESSION.into()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap();
    config
        .register_requester_challenge(&RequesterChallengeRequest {
            session_id: SESSION.into(),
            challenge_id: "c1".into(),
            mission_id: Uuid::parse_str(MISSION).unwrap(),
            mission_root: checkout.clone(),
            revision: 3,
            plan_digest: P.into(),
            review_snapshot_digest: R.into(),
        })
        .unwrap();
    Fx {
        _dir: dir,
        checkout,
        config,
    }
}
fn hook(f: &Fx, mut payload: serde_json::Value) -> std::process::Output {
    let transcript = f.checkout.join("t.jsonl");
    fs::write(&transcript, "").unwrap();
    payload["session_id"] = SESSION.into();
    payload["transcript_path"] = transcript.to_str().unwrap().into();
    let mut child = Command::new(env!("CARGO_BIN_EXE_session-capture-hook"))
        .arg("--from-hook-stdin")
        .current_dir(&f.checkout)
        .env("MCP_MAIN_CHECKOUT", &f.checkout)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
fn decision(kind: &str) -> String {
    format!("mission-decision {kind} {MISSION} 3 {P} {R} c1")
}
fn find(f: &Fx) -> Option<session_api::RequesterDecisionReceipt> {
    f.config
        .find_requester_decision_for_challenge(SESSION, "c1")
        .unwrap()
}

#[test]
fn user_prompt_submit_persists_event_and_receipt_with_host_event_id() {
    let f = fx();
    let raw = decision("approve");
    let out = hook(
        &f,
        serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": raw, "message_id": "host-1"}),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let events: PersistedSessionEvents = serde_json::from_str(
        &fs::read_to_string(
            f.checkout
                .join(".workflow-tools/session/sessions")
                .join(SESSION)
                .join("events.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        events
            .events
            .iter()
            .any(|e| e.data_json.as_ref().and_then(|d| d["prompt"].as_str()) == Some(raw.as_str()))
    );
    let receipt = find(&f).expect("receipt");
    assert_eq!(receipt.kind, DecisionKind::Approve);
    assert_eq!(receipt.reference.source_event_id, "host-1");
    assert_eq!(receipt.read_raw_prompt().unwrap(), raw.as_bytes());
}

#[test]
fn missing_event_id_is_allocated_and_duplicate_delivery_keeps_identity() {
    let f = fx();
    let payload =
        serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": decision("approve")});
    assert!(hook(&f, payload.clone()).status.success());
    let first = find(&f).unwrap();
    assert!(!first.ingress.host_event_id_supplied && !first.reference.source_event_id.is_empty());
    let events: PersistedSessionEvents = serde_json::from_str(
        &fs::read_to_string(
            f.checkout
                .join(".workflow-tools/session/sessions")
                .join(SESSION)
                .join("events.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        events
            .events
            .iter()
            .filter(|e| e.event_id.as_deref() == Some(first.reference.source_event_id.as_str()))
            .count(),
        1
    );
    assert!(hook(&f, payload).status.success());
    assert_eq!(find(&f).unwrap().reference, first.reference);
}

#[test]
fn non_user_prompt_hooks_and_embedded_text_create_no_receipt() {
    let f = fx();
    for payload in [
        serde_json::json!({"hook_event_name": "Stop", "prompt": decision("approve")}),
        serde_json::json!({"hook_event_name": "PostToolUse", "prompt": decision("approve")}),
        serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": format!("hi\n{}", decision("approve"))}),
        serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": "nothing relevant"}),
    ] {
        let _ = hook(&f, payload);
        assert!(find(&f).is_none());
    }

    #[test]
    fn stale_decision_line_exits_zero_without_receipt() {
        let f = fx();
        let stale = decision("approve").replace(" 3 ", " 4 ");
        let out = hook(
            &f,
            serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": stale}),
        );
        assert!(out.status.success());
        assert!(find(&f).is_none());
    }

    #[test]
    fn event_persistence_failure_exits_nonzero_and_leaves_no_usable_receipt() {
        let f = fx();
        let events = f
            .checkout
            .join(".workflow-tools/session/sessions")
            .join(SESSION)
            .join("events.json");
        let _ = fs::remove_file(&events);
        fs::create_dir(&events).unwrap();
        let out = hook(
            &f,
            serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": decision("approve")}),
        );
        assert!(!out.status.success());
        assert!(find(&f).is_none());
    }
}
