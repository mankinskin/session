//! Tests for handoff record round-trip persistence.
//!
//! Validates that all fields accepted by `session_handoff` are persisted
//! and returned unchanged, with no silent drops.

use session_api::{
    SessionHandoffPackage,
    SessionHandoffTargetTicket,
    SessionHandoffUpwardContextEntry,
    SessionHandoffUpwardContextRole,
    SessionRuntimeInitRequest,
    SessionStoreConfig,
    SessionValidationGate,
};
use std::path::PathBuf;

fn setup_test_store() -> (SessionStoreConfig, PathBuf) {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let store_root = temp_dir.path().to_path_buf();
    let config = SessionStoreConfig::new(&store_root, "test-workspace");
    (config, store_root)
}

fn init_test_session(
    config: &SessionStoreConfig,
    session_id: &str,
) {
    config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(session_id.to_string()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .expect("init runtime context");
}

fn target_ticket(id: &str) -> SessionHandoffTargetTicket {
    SessionHandoffTargetTicket {
        id: id.to_string(),
        why: "Required by the implementation unit".to_string(),
        state: "ready".to_string(),
        acceptance_criteria: vec![
            "The implementation unit completes".to_string(),
        ],
    }
}

fn upward_context() -> Vec<SessionHandoffUpwardContextEntry> {
    vec![SessionHandoffUpwardContextEntry {
        entity_urn: "ce://default/ticket/program".to_string(),
        title: "Program objective".to_string(),
        role: SessionHandoffUpwardContextRole::Epic,
    }]
}

#[test]
fn open_escalations_field_persists_and_round_trips() {
    let (config, _temp_dir) = setup_test_store();
    let session_id = "20000000-0000-4000-8000-000000000001";

    init_test_session(&config, session_id);

    // Create a handoff package with non-empty open_escalations
    let package = SessionHandoffPackage {
        objective: "Fix the bug".to_string(),
        target_tickets: vec![target_ticket("ticket-123")],
        higher_level_objective: "Deliver the program objective".to_string(),
        upward_context: upward_context(),
        target_files: vec![
            "workflow-tools/session/crates/session-api/src/lib.rs".to_string(),
        ],
        decisions: vec!["Use async/await".to_string()],
        non_goals: vec!["No refactoring".to_string()],
        context_anchors: vec!["Related PR #456".to_string()],
        open_escalations: vec![
            "Need clarification on API design".to_string(),
            "Blocked on upstream merge".to_string(),
        ],
        risk_notes: Some("Database migration required".to_string()),
        predecessor_handoff: None,
    };

    let validation = vec![];

    // Create handoff record
    let record = config
        .create_handoff_record(
            session_id,
            Some(package.clone()),
            validation,
            None,
        )
        .expect("create handoff record");

    assert_eq!(
        record.higher_level_objective,
        package.higher_level_objective
    );
    assert_eq!(record.upward_context, package.upward_context);

    // ASSERT: open_escalations should persist unchanged
    assert_eq!(
        record.open_escalations, package.open_escalations,
        "open_escalations should round-trip unchanged; got {:?} but expected {:?}",
        record.open_escalations, package.open_escalations
    );
    assert_eq!(record.open_escalations.len(), 2);
    assert!(
        record
            .open_escalations
            .contains(&"Need clarification on API design".to_string())
    );
    assert!(
        record
            .open_escalations
            .contains(&"Blocked on upstream merge".to_string())
    );
}

#[test]
fn empty_open_escalations_is_persisted_as_empty_list() {
    let (config, _temp_dir) = setup_test_store();
    let session_id = "20000000-0000-4000-8000-000000000002";

    init_test_session(&config, session_id);

    let package = SessionHandoffPackage {
        objective: "Implement feature".to_string(),
        target_tickets: vec![target_ticket("ticket-789")],
        higher_level_objective: "Deliver the program objective".to_string(),
        upward_context: upward_context(),
        target_files: vec![
            "workflow-tools/session/crates/session-api/src/error.rs".to_string(),
        ],
        decisions: vec!["Use trait bounds".to_string()],
        non_goals: vec!["No optimization yet".to_string()],
        context_anchors: vec!["Spec doc#12".to_string()],
        open_escalations: vec![], // Explicitly empty
        risk_notes: None,
        predecessor_handoff: None,
    };

    let record = config
        .create_handoff_record(session_id, Some(package.clone()), vec![], None)
        .expect("create handoff record");

    assert_eq!(
        record.higher_level_objective,
        package.higher_level_objective
    );
    assert_eq!(record.upward_context, package.upward_context);

    // ASSERT: empty open_escalations should persist as empty list (not absent/null)
    assert_eq!(record.open_escalations, Vec::<String>::new());
    assert!(record.open_escalations.is_empty());
}

#[test]
fn validation_gate_command_field_persists_and_round_trips() {
    let (config, _temp_dir) = setup_test_store();
    let session_id = "20000000-0000-4000-8000-000000000003";

    init_test_session(&config, session_id);

    let package = SessionHandoffPackage {
        objective: "Run tests".to_string(),
        target_tickets: vec![target_ticket("ticket-101")],
        higher_level_objective: "Deliver the program objective".to_string(),
        upward_context: upward_context(),
        target_files: vec![
            "workflow-tools/session/crates/session-api/src/store.rs".to_string(),
        ],
        decisions: vec!["Use Criterion benchmarks".to_string()],
        non_goals: vec!["No UI tests".to_string()],
        context_anchors: vec!["Test plan doc".to_string()],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let validation = vec![SessionValidationGate {
        validation_spec_id: "val-test-suite".to_string(),
        required: true,
        outcome: None,
        command: Some("cargo test -p session-api".to_string()),
    }];

    let record = config
        .create_handoff_record(
            session_id,
            Some(package),
            validation.clone(),
            None,
        )
        .expect("create handoff record");

    assert_eq!(
        record.higher_level_objective,
        "Deliver the program objective"
    );
    assert_eq!(record.upward_context, upward_context());

    // ASSERT: command field should persist unchanged
    assert_eq!(record.validation.len(), 1);
    let gate = &record.validation[0];
    assert_eq!(gate.validation_spec_id, "val-test-suite");
    assert_eq!(gate.required, true);
    assert_eq!(
        gate.command,
        Some("cargo test -p session-api".to_string()),
        "command field should round-trip unchanged"
    );
}

#[test]
fn legacy_target_ticket_strings_and_absent_context_fields_deserialize() {
    let (config, _temp_dir) = setup_test_store();
    let session_id = "20000000-0000-4000-8000-000000000004";
    init_test_session(&config, session_id);

    let package = SessionHandoffPackage {
        objective: "Implement compatibility".to_string(),
        target_tickets: vec![target_ticket("ticket-legacy")],
        higher_level_objective: "Deliver the program objective".to_string(),
        upward_context: upward_context(),
        target_files: vec![
            "workflow-tools/session/crates/session-api/src/lib.rs".to_string(),
        ],
        decisions: vec!["Use serde compatibility".to_string()],
        non_goals: vec!["No renderer changes".to_string()],
        context_anchors: vec!["Spec context".to_string()],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };
    let record = config
        .create_handoff_record(session_id, Some(package), vec![], None)
        .expect("create handoff record");

    let mut legacy_json =
        serde_json::to_value(record).expect("serialize record");
    let object = legacy_json.as_object_mut().expect("record object");
    object.remove("higher_level_objective");
    object.remove("upward_context");
    object.insert(
        "target_tickets".to_string(),
        serde_json::json!(["ticket-legacy"]),
    );

    let legacy: session_api::SessionHandoffRecord =
        serde_json::from_value(legacy_json)
            .expect("deserialize legacy handoff");
    assert_eq!(legacy.target_tickets[0].id, "ticket-legacy");
    assert!(legacy.target_tickets[0].why.is_empty());
    assert!(legacy.higher_level_objective.is_empty());
    assert!(legacy.upward_context.is_empty());
}

#[test]
fn ready_handoff_missing_upward_context_fails_before_writing_files() {
    let (config, store_root) = setup_test_store();
    let session_id = "20000000-0000-4000-8000-000000000005";
    init_test_session(&config, session_id);

    let package = SessionHandoffPackage {
        objective: "Implement the current unit".to_string(),
        target_tickets: vec![target_ticket("ticket-123")],
        higher_level_objective: String::new(),
        upward_context: vec![],
        target_files: vec![
            "workflow-tools/session/crates/session-api/src/lib.rs".to_string(),
        ],
        decisions: vec!["Use serde".to_string()],
        non_goals: vec!["No renderer changes".to_string()],
        context_anchors: vec!["Spec context".to_string()],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let result =
        config.create_handoff_record(session_id, Some(package), vec![], None);
    assert!(matches!(
        result,
        Err(session_api::SessionError::HandoffPackageIncomplete { .. })
    ));
    assert!(
        !store_root
            .join(".session")
            .join("sessions")
            .join(session_id)
            .join("handoffs")
            .exists()
    );
}

#[test]
fn non_ready_handoff_missing_upward_context_persists() {
    let (config, _temp_dir) = setup_test_store();
    let session_id = "20000000-0000-4000-8000-000000000006";
    init_test_session(&config, session_id);

    let package = SessionHandoffPackage {
        objective: "Implement the current unit".to_string(),
        target_tickets: vec![target_ticket("ticket-123")],
        higher_level_objective: String::new(),
        upward_context: vec![],
        target_files: vec![
            "workflow-tools/session/crates/session-api/src/lib.rs".to_string(),
        ],
        decisions: vec!["Use serde".to_string()],
        non_goals: vec!["No renderer changes".to_string()],
        context_anchors: vec!["Spec context".to_string()],
        open_escalations: vec!["Awaiting a decision".to_string()],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let record = config
        .create_handoff_record(session_id, Some(package), vec![], None)
        .expect("non-ready handoff persists");
    assert!(record.higher_level_objective.is_empty());
    assert!(record.upward_context.is_empty());
}
