#[allow(unused_imports)]
use super::*;

/// High: cross-workspace ticket routing is rejected explicitly rather than
/// silently resolved against the wrong store.
#[test]
pub(in super::super) fn workflow_finish_rejects_cross_workspace_ticket_routing() {
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

    ticket_api::storage::TicketStore::open_or_init(&store_root.join(".ticket")).unwrap();

    // URN addresses a different workspace than the session's `context-engine`.
    let ticket_urn = "ce://other-workspace/tickets/44444444-4444-4444-8444-444444444444";
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("ticket-node".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "cross-workspace".to_string(),
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
        reason.contains("cross-workspace") || reason.contains("unavailable"),
        "expected routing rejection, got: {reason}"
    );
}

/// High: a ticket URN addressing a different (nested) workspace slug resolves
/// live state from that workspace's own sibling ticket store, with no
/// diagnostic, once the nested store actually exists.
#[test]
pub(in super::super) fn workflow_finish_resolves_ticket_from_nested_workspace_store() {
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

    let ticket_id = uuid::Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap();
    let nested_store = ticket_api::storage::TicketStore::open_or_init(
        &store_root.join("nested-workspace").join(".ticket"),
    )
    .unwrap();
    nested_store
        .create(
            Some(ticket_id),
            "tracker-improvement",
            Some("lives in the nested store"),
            Some("done"),
            std::collections::BTreeMap::new(),
            None,
            None,
        )
        .unwrap();

    let ticket_urn = format!("ce://nested-workspace/tickets/{ticket_id}");
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("ticket-node".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "nested-workspace ticket".to_string(),
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

    let finished = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap();
    assert!(!finished.already_finished);
}

/// High: an unknown workspace slug fails closed with a descriptive diagnostic
/// and never creates a store directory as a side effect.
#[test]
pub(in super::super) fn workflow_finish_rejects_unknown_workspace_path_without_creating_store() {
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

    let ticket_urn = "ce://unknown-workspace/tickets/66666666-6666-4666-8666-666666666666";
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("ticket-node".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "unknown workspace".to_string(),
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
        reason.contains("unavailable") || reason.contains("not initialized"),
        "expected unavailable diagnostic, got: {reason}"
    );
    assert!(
        !store_root.join("unknown-workspace").exists(),
        "resolver must never create a store directory as a side effect"
    );
}

/// High: a path-traversal workspace slug is rejected by validation before any
/// path is built, and creates no directory.
#[test]
pub(in super::super) fn workflow_finish_rejects_path_traversal_workspace_path() {
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

    let ticket_urn = "ce://../tickets/77777777-7777-4777-8777-777777777777";
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("ticket-node".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "path traversal".to_string(),
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
        reason.contains("invalid path characters"),
        "expected slug validation rejection, got: {reason}"
    );
    assert!(
        !tempdir.path().join(".ticket").exists(),
        "resolver must never create a store directory as a side effect"
    );
}
