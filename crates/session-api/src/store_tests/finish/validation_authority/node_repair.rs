#[allow(unused_imports)]
use super::*;

// ── Reject-at-creation coverage (ticket 980cf1fa) ───────────────────────────

/// AC1: a `validation` node with an absent `validation_spec_id` is rejected
/// at creation, naming the field, instead of being persisted as a wedge.
#[test]
pub(in super::super) fn workflow_add_node_rejects_validation_kind_with_absent_spec_id() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    let error = config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("wedge-attempt".to_string()),
                kind: SessionWorkflowNodeKind::Validation,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "would wedge finish".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("validation_spec_id"),
        "error should name validation_spec_id, got: {message}"
    );
}

/// AC1 (batch half) + AC5: the batch tool identifies the offending
/// `nodes[index]` and rejects a `validation_spec_id` that does not resolve
/// to a real spec in the `.test` store.
#[test]
pub(in super::super) fn workflow_add_nodes_rejects_unresolvable_validation_spec_id_with_index() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    let error = config
        .workflow_add_nodes(
            &workspace_id,
            vec![
                SessionWorkflowNodeDraft {
                    node_id: Some("ok-node".to_string()),
                    kind: SessionWorkflowNodeKind::Task,
                    requirement: SessionWorkflowNodeRequirement::Optional,
                    title: "fine".to_string(),
                    ticket_urn: None,
                    spec_urn: None,
                    anchor_urn: None,
                    category: None,
                    cached_ticket_title: None,
                    validation_spec_id: None,
                },
                SessionWorkflowNodeDraft {
                    node_id: Some("bad-node".to_string()),
                    kind: SessionWorkflowNodeKind::Validation,
                    requirement: SessionWorkflowNodeRequirement::Required,
                    title: "would wedge finish".to_string(),
                    ticket_urn: None,
                    spec_urn: None,
                    anchor_urn: None,
                    category: None,
                    cached_ticket_title: None,
                    // A node UUID (or any other non-spec id) must not resolve.
                    validation_spec_id: Some("00000000-0000-0000-0000-000000000000".to_string()),
                },
            ],
        )
        .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("nodes[1]"),
        "error should identify offending nodes[index], got: {message}"
    );
    assert!(
        message.contains("validation_spec_id"),
        "error should name validation_spec_id, got: {message}"
    );
}

/// AC2: a `ticket` node without `ticket_urn` and a `spec` node without
/// `spec_urn` fail fast and consistently, matching the `validation` case.
#[test]
pub(in super::super) fn workflow_add_node_rejects_ticket_and_spec_kinds_without_urn() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    let ticket_error = config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("ticket-wedge-attempt".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "would wedge finish".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap_err();
    assert!(ticket_error.to_string().contains("ticket_urn"));

    let spec_error = config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("spec-wedge-attempt".to_string()),
                kind: SessionWorkflowNodeKind::Spec,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "would wedge finish".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap_err();
    assert!(spec_error.to_string().contains("spec_urn"));
}

// ── Repair surface coverage (ticket 980cf1fa) ───────────────────────────────

/// Simulate a legacy graph wedged by the exact bug this ticket fixes: a
/// `validation` node persisted (for example by a pre-fix build) with a null
/// `validation_spec_id`. The repair surface must fix it in place without
/// adding a second node, and the finish rejection must name the repair tool.
#[test]
pub(in super::super) fn wedged_validation_node_is_repaired_via_update_node_and_finish_then_succeeds()
 {
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

    // Directly persist a wedged node, bypassing the create-time validation,
    // to reproduce data left over from before this fix existed.
    let mut context = config.read_runtime_context(&workspace_id).unwrap();
    let now = chrono::Utc::now();
    context.workflow.nodes.push(crate::SessionWorkflowNode {
        node_id: "wedged".to_string(),
        kind: SessionWorkflowNodeKind::Validation,
        requirement: SessionWorkflowNodeRequirement::Required,
        status: SessionWorkflowNodeStatus::Pending,
        title: "legacy wedged validation gate".to_string(),
        created_at: now,
        updated_at: now,
        ticket_urn: None,
        spec_urn: None,
        anchor_urn: None,
        category: None,
        cached_ticket_title: None,
        deferred_reason: None,
        validation_spec_id: None,
    });
    let manifest_path = config
        .paths_for_session_id(&workspace_id)
        .unwrap()
        .manifest_path;
    let mut persisted: super::super::super::super::PersistedSessionManifest =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    persisted.workflow = context.workflow;
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&persisted).unwrap(),
    )
    .unwrap();
    config
        .workflow_update_node_status(
            &workspace_id,
            "wedged",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();

    // The wedge blocks finish, and the rejection names the repair tool
    // (AC4) instead of only stating the field is missing.
    let blocked = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap_err();
    let blocked_message = blocked.to_string();
    assert!(
        blocked_message.contains("workflow_update_node")
            && blocked_message.contains("workflow_remove_node"),
        "finish rejection should name the repair tools, got: {blocked_message}"
    );

    // Seed a real spec so the repair patch resolves (AC5), then repair the
    // wedged node in place via the new patch surface (AC3).
    let spec_id = "val-repair-wedge";
    let test_store = test_store_for(&store_root);
    seed_validation_spec(&test_store, spec_id);
    seed_execution(
        &test_store,
        "exec-repair-wedge",
        spec_id,
        test_api::ValidationOutcome::Passed,
    );

    config
        .workflow_update_node(
            &workspace_id,
            "wedged",
            crate::SessionWorkflowNodePatch {
                validation_spec_id: Some(spec_id.to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    config
        .workflow_update_node_status(
            &workspace_id,
            "wedged",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();

    // Round-trip: handoff/finish now succeeds after repair.
    let finished = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap();
    assert!(!finished.already_finished);
}

/// Repair via deletion: a wedged node that should never have existed can be
/// removed outright instead of patched, and finish then succeeds because the
/// graph no longer carries the offending required node.
#[test]
pub(in super::super) fn wedged_validation_node_is_repaired_via_remove_node() {
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

    let mut context = config.read_runtime_context(&workspace_id).unwrap();
    let now = chrono::Utc::now();
    context.workflow.nodes.push(crate::SessionWorkflowNode {
        node_id: "wedged-2".to_string(),
        kind: SessionWorkflowNodeKind::Validation,
        requirement: SessionWorkflowNodeRequirement::Required,
        status: SessionWorkflowNodeStatus::Pending,
        title: "legacy wedged validation gate".to_string(),
        created_at: now,
        updated_at: now,
        ticket_urn: None,
        spec_urn: None,
        anchor_urn: None,
        category: None,
        cached_ticket_title: None,
        deferred_reason: None,
        validation_spec_id: None,
    });
    let manifest_path = config
        .paths_for_session_id(&workspace_id)
        .unwrap()
        .manifest_path;
    let mut persisted: super::super::super::super::PersistedSessionManifest =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    persisted.workflow = context.workflow;
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&persisted).unwrap(),
    )
    .unwrap();

    assert!(matches!(
        config.finish_workflow(&workspace_id, vec![], vec![], None),
        Err(SessionError::FinishBlocked { .. })
    ));

    config
        .workflow_remove_node(&workspace_id, "wedged-2")
        .unwrap();

    let finished = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap();
    assert!(!finished.already_finished);
}
