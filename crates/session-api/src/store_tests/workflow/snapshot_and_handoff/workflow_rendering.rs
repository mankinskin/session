#[allow(unused_imports)]
use super::*;

#[test]
pub(in super::super) fn workflow_snapshot_resolves_live_state_and_emits_missing_diagnostics() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("node-live".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "Existing ticket".to_string(),
                ticket_urn: Some(
                    "ce://default/tickets/412964a3-e1c3-47da-94ad-268ff20441c0".to_string(),
                ),
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();
    let missing_urn = "ce://default/tickets/deadbeef-dead-beef-dead-beefdeadbeef";
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("node-missing".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "Missing ticket".to_string(),
                ticket_urn: Some(missing_urn.to_string()),
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();

    let snapshot = config
        .workflow_snapshot(
            &workspace_id,
            Some(&MockTicketResolver {
                missing_urn: missing_urn.to_string(),
            }),
        )
        .unwrap();

    assert!(
        snapshot
            .resolutions
            .iter()
            .any(|item| item.node_id == "node-live"
                && item.live_ticket_state.as_deref() == Some("in-review"))
    );
    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|diag| diag.node_id == "node-missing" && diag.code == "ticket-state-unavailable")
    );
    let target_file = tempdir.path().join("store/src/model/handoff.rs");
    std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
    std::fs::write(&target_file, "fixture").unwrap();
}

#[test]
pub(in super::super) fn workflow_render_outputs_are_deterministic_and_escaped() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("node-a".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "Run \"workflow\" check".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("node-b".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Optional,
                title: "Ticket fallback".to_string(),
                ticket_urn: Some(
                    "ce://default/tickets/deadbeef-dead-beef-dead-beefdeadbeef".to_string(),
                ),
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();
    config
        .workflow_add_edge(
            &workspace_id,
            "node-a",
            "node-b",
            SessionWorkflowEdgeKind::DependsOn,
        )
        .unwrap();

    let resolver = MockTicketResolver {
        missing_urn: "ce://default/tickets/deadbeef-dead-beef-dead-beefdeadbeef".to_string(),
    };

    let terminal_first = config
        .workflow_render_terminal(&workspace_id, Some(&resolver))
        .unwrap();
    let terminal_second = config
        .workflow_render_terminal(&workspace_id, Some(&resolver))
        .unwrap();
    assert_eq!(terminal_first, terminal_second);
    assert!(terminal_first.contains("ticket-state-unavailable"));
    assert!(terminal_first.contains("node-a"));
    assert!(terminal_first.contains("blockers=node-b"));

    let mermaid_first = config
        .workflow_render_mermaid(&workspace_id, Some(&resolver))
        .unwrap();
    let mermaid_second = config
        .workflow_render_mermaid(&workspace_id, Some(&resolver))
        .unwrap();
    assert_eq!(mermaid_first, mermaid_second);
    assert!(mermaid_first.starts_with("flowchart TD\n"));
    assert!(mermaid_first.contains("Run \\\"workflow\\\" check"));
    assert!(mermaid_first.contains("-->|depends_on|"));
}

#[test]
pub(in super::super) fn workflow_render_is_read_only_for_runtime_persistence() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("node-read-only".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "render check".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();

    let manifest_path = config
        .paths_for_session_id(&workspace_id)
        .unwrap()
        .manifest_path;
    let before = std::fs::read(&manifest_path).unwrap();

    let _ = config
        .workflow_render_terminal(&workspace_id, None)
        .unwrap();
    let _ = config.workflow_render_mermaid(&workspace_id, None).unwrap();

    let after = std::fs::read(&manifest_path).unwrap();
    assert_eq!(before, after);
}
