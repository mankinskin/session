#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn receipt_without_persisted_source_event_never_resolves() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let receipt = ingest_c1(&config, Some("e1"));
    std::fs::remove_file(decision_dir(&config).join("source_event.json")).unwrap();
    let error = config
        .read_requester_decision(&receipt.reference)
        .unwrap_err();
    assert_eq!(code(&error), "requester_decision_corrupt");
    assert!(
        config
            .find_requester_decision_for_challenge(SESSION, "c1")
            .is_err()
    );
}

#[test]
pub(super) fn raw_tamper_and_forged_receipt_fail_closed() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let receipt = ingest_c1(&config, Some("e1"));
    let raw = decision_dir(&config).join("raw.bin");
    std::fs::write(&raw, line("reject", "c1")).unwrap();
    assert_eq!(
        code(
            &config
                .read_requester_decision(&receipt.reference)
                .unwrap_err()
        ),
        "requester_decision_corrupt"
    );
    std::fs::write(&raw, line("approve", "c1")).unwrap();
    config.read_requester_decision(&receipt.reference).unwrap();
    let path = decision_dir(&config).join("receipt.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["kind"] = "reject".into();
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        code(
            &config
                .read_requester_decision(&receipt.reference)
                .unwrap_err()
        ),
        "requester_decision_corrupt"
    );
}

#[test]
pub(super) fn event_id_collisions_with_other_decisions_or_canonical_events_conflict() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    config
        .register_requester_challenge(&request(dir.path(), "c2"))
        .unwrap();
    config
        .register_requester_challenge(&request(dir.path(), "c3"))
        .unwrap();
    ingest_c1(&config, Some("dup"));
    let error = config
        .ingest_requester_prompt(
            SESSION,
            &host("UserPromptSubmit", &line("approve", "c2"), Some("dup")),
        )
        .unwrap_err();
    assert_eq!(code(&error), "requester_decision_conflict");
    let other: session_api::CopilotHookEvent = serde_json::from_value(serde_json::json!({
        "event_id": "other", "event_type": "UserPromptSubmit", "data_json": {"prompt": "different"}
    }))
    .unwrap();
    config.persist_hook_event(SESSION, other).unwrap();
    let error = config
        .ingest_requester_prompt(
            SESSION,
            &host("UserPromptSubmit", &line("approve", "c3"), Some("other")),
        )
        .unwrap_err();
    assert_eq!(code(&error), "requester_decision_conflict");
    assert!(
        config
            .find_requester_decision_for_challenge(SESSION, "c3")
            .unwrap()
            .is_none()
    );
}

#[test]
pub(super) fn every_interrupted_step_is_pending_unusable_and_recovers_one_identity() {
    for fault in [
        DecisionFault::AfterIntent,
        DecisionFault::AfterRawPersisted,
        DecisionFault::AfterSourceEvent,
        DecisionFault::AfterEventMirror,
    ] {
        let (dir, config) = store();
        config
            .register_requester_challenge(&request(dir.path(), "c1"))
            .unwrap();
        let raw = line("approve", "c1");
        let error = config
            .ingest_requester_prompt_with_fault(
                SESSION,
                &host("UserPromptSubmit", &raw, Some("h1")),
                Some(fault),
            )
            .unwrap_err();
        assert_eq!(code(&error), "requester_decision_pending", "{fault:?}");
        assert!(
            config
                .find_requester_decision_for_challenge(SESSION, "c1")
                .unwrap()
                .is_none(),
            "{fault:?}"
        );
        assert_eq!(
            config.pending_requester_decisions(SESSION).unwrap().len(),
            1,
            "{fault:?}"
        );
        let done = config
            .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", &raw, None))
            .unwrap()
            .unwrap();
        assert_eq!(done.reference.source_event_id, "h1", "{fault:?}");
        assert!(
            config
                .pending_requester_decisions(SESSION)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
pub(super) fn orphan_raw_without_intent_is_discovered_and_only_same_bytes_adopt_it() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let ddir = decision_dir(&config);
    std::fs::create_dir_all(&ddir).unwrap();
    std::fs::write(ddir.join("raw.bin"), line("approve", "c1")).unwrap();
    assert_eq!(
        config.pending_requester_decisions(SESSION).unwrap().len(),
        1
    );
    let error = config
        .ingest_requester_prompt(
            SESSION,
            &host("UserPromptSubmit", &line("reject", "c1"), None),
        )
        .unwrap_err();
    assert_eq!(code(&error), "requester_decision_conflict");
    ingest_c1(&config, None);
    assert!(
        config
            .pending_requester_decisions(SESSION)
            .unwrap()
            .is_empty()
    );
}

#[test]
pub(super) fn canonical_mirror_failure_is_surfaced_and_recoverable() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let raw = line("approve", "c1");
    config
        .ingest_requester_prompt_with_fault(
            SESSION,
            &host("UserPromptSubmit", &raw, Some("h")),
            Some(DecisionFault::AfterSourceEvent),
        )
        .unwrap_err();
    let events = config
        .root
        .join("sessions")
        .join(SESSION)
        .join("events.json");
    let _ = std::fs::remove_file(&events);
    std::fs::create_dir(&events).unwrap();
    assert!(
        config
            .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", &raw, Some("h")))
            .is_err()
    );
    assert!(
        config
            .find_requester_decision_for_challenge(SESSION, "c1")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        config.pending_requester_decisions(SESSION).unwrap().len(),
        1
    );
    std::fs::remove_dir(&events).unwrap();
    assert_eq!(ingest_c1(&config, Some("h")).reference.source_event_id, "h");
}
