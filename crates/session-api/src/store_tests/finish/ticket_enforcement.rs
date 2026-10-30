#[allow(unused_imports)]
use super::*;

/// Critical: a caller submitting `passed` cannot substitute for a missing
/// authoritative execution record.
#[test]
pub(super) fn workflow_finish_rejects_caller_passed_when_no_execution_exists() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    let spec_id = "val-remediation-missing-exec";
    let test_store = test_store_for(&store_root);
    seed_validation_spec(&test_store, spec_id);
    // Intentionally record no execution.

    add_required_validation_node(&config, &workspace_id, spec_id);

    let error = config
        .finish_workflow(
            &workspace_id,
            vec![crate::SessionValidationGate {
                validation_spec_id: spec_id.to_string(),
                required: true,
                outcome: Some("passed".to_string()),
                command: None,
            }],
            vec![],
            None,
        )
        .unwrap_err();
    assert!(matches!(error, SessionError::FinishBlocked { .. }));
}

/// Positive control: finish succeeds only when the authoritative execution is
/// `passed`, regardless of caller-provided outcomes.
#[test]
pub(super) fn workflow_finish_accepts_authoritative_passed_execution() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    let spec_id = "val-remediation-passed";
    let test_store = test_store_for(&store_root);
    seed_validation_spec(&test_store, spec_id);
    seed_execution(
        &test_store,
        "exec-authority-passed",
        spec_id,
        test_api::ValidationOutcome::Passed,
    );

    add_required_validation_node(&config, &workspace_id, spec_id);

    // Caller omits any gate; authority alone certifies the outcome.
    let finished = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap();
    assert!(!finished.already_finished);
    assert!(finished.record.validation.iter().any(|gate| {
        gate.validation_spec_id == spec_id && gate.outcome.as_deref() == Some("passed")
    }));
}

/// Critical: a ticket node marked locally `Done` must not certify completion
/// when the live ticket state is non-terminal.
#[test]
pub(super) fn workflow_finish_rejects_local_done_when_live_ticket_non_terminal() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root);
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    let ticket_urn = "ce://context-engine/tickets/11111111-1111-4111-8111-111111111111";
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("ticket-node".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "ticket-backed".to_string(),
                ticket_urn: Some(ticket_urn.to_string()),
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();
    // Local status is Done, but live state below is non-terminal.
    config
        .workflow_update_node_status(
            &workspace_id,
            "ticket-node",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();

    let resolver = FixedStateResolver {
        urn: ticket_urn.to_string(),
        state: Some("in-implementation".to_string()),
    };
    let error = config
        .finish_workflow(&workspace_id, vec![], vec![], Some(&resolver))
        .unwrap_err();
    assert!(matches!(error, SessionError::FinishBlocked { .. }));

    // Positive control: a live terminal state permits finish.
    let terminal = FixedStateResolver {
        urn: ticket_urn.to_string(),
        state: Some("done".to_string()),
    };
    let finished = config
        .finish_workflow(&workspace_id, vec![], vec![], Some(&terminal))
        .unwrap();
    assert!(!finished.already_finished);
}

/// High: production path — the real default resolver blocks finish when a
/// required ticket node references a non-terminal live ticket.
#[test]
pub(super) fn workflow_finish_production_path_blocks_non_terminal_ticket() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    let ticket_id = uuid::Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
    let ticket_store =
        ticket_api::storage::TicketStore::open_or_init(&store_root.join(".ticket")).unwrap();
    ticket_store
        .create(
            Some(ticket_id),
            "tracker-improvement",
            Some("live ticket"),
            Some("in-implementation"),
            std::collections::BTreeMap::new(),
            None,
            None,
        )
        .unwrap();

    let ticket_urn = format!("ce://context-engine/tickets/{ticket_id}");
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("ticket-node".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "live ticket".to_string(),
                ticket_urn: Some(ticket_urn),
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();
    config
        .workflow_update_node_status(
            &workspace_id,
            "ticket-node",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();

    // resolver=None exercises the real default resolver + store layout.
    let error = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap_err();
    assert!(matches!(error, SessionError::FinishBlocked { .. }));
}

/// High: production path — an absent required ticket resolves to an unavailable
/// diagnostic that blocks finish (fail closed).
#[test]
pub(super) fn workflow_finish_production_path_blocks_missing_ticket() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    // Initialize an empty ticket store so the resolver can open it, but the
    // referenced ticket does not exist.
    ticket_api::storage::TicketStore::open_or_init(&store_root.join(".ticket")).unwrap();

    let ticket_urn = "ce://context-engine/tickets/33333333-3333-4333-8333-333333333333";
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("ticket-node".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "missing ticket".to_string(),
                ticket_urn: Some(ticket_urn.to_string()),
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();
    config
        .workflow_update_node_status(
            &workspace_id,
            "ticket-node",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();

    let error = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap_err();
    let SessionError::FinishBlocked { reason } = error else {
        panic!("expected FinishBlocked, got {error:?}");
    };
    assert!(
        reason.contains("unavailable"),
        "expected unavailable diagnostic, got: {reason}"
    );
}

#[path = "ticket_enforcement/workspace_routing.rs"]
mod workspace_routing;
#[allow(unused_imports)]
pub use workspace_routing::*;
