#[allow(unused_imports)]
use super::*;

#[test]
pub(in super::super) fn runtime_pin_accepts_mission_urn() {
    let tempdir = tempfile::tempdir().unwrap();
    let workspace = tempdir.path();
    let config = super::super::super::super::SessionStoreConfig::new(
        workspace.join(".workflow-tools").join("session"),
    );
    let session_id = uuid::Uuid::new_v4().to_string();
    config
        .init_runtime_context(crate::SessionRuntimeInitRequest {
            session_id: Some(session_id.clone()),
            ..Default::default()
        })
        .unwrap();

    let mission_id = uuid::Uuid::new_v4();
    let mission_urn = format!("ce://default/mission/{mission_id}");
    let accepted_path = workspace
        .join(".workflow-tools/mission/missions")
        .join(mission_id.to_string())
        .join("accepted.json");
    std::fs::create_dir_all(accepted_path.parent().unwrap()).unwrap();
    std::fs::write(
        &accepted_path,
        serde_json::json!({
            "mission_id": mission_id,
            "bundle": { "manifest": { "title": "Shared mission" } }
        })
        .to_string(),
    )
    .unwrap();

    let context = config
        .pin_runtime_entity(&session_id, &mission_urn, None, None)
        .unwrap();
    assert_eq!(
        context.pinned_entities[0].kind,
        crate::SessionPinnedEntityKind::Mission
    );
    assert_eq!(
        config
            .resolve_pinned_mission(&session_id, &mission_urn)
            .unwrap()["bundle"]["manifest"]["title"],
        "Shared mission"
    );
}

#[test]
pub(in super::super) fn runtime_pin_rejects_invalid_mission_urn() {
    let tempdir = tempfile::tempdir().unwrap();
    let config = super::super::super::super::SessionStoreConfig::new(
        tempdir.path().join(".workflow-tools").join("session"),
    );
    let session_id = uuid::Uuid::new_v4().to_string();
    config
        .init_runtime_context(crate::SessionRuntimeInitRequest {
            session_id: Some(session_id.clone()),
            ..Default::default()
        })
        .unwrap();

    let error = config
        .pin_runtime_entity(&session_id, "ce://default/mission/not-a-uuid", None, None)
        .unwrap_err();
    assert!(
        error.to_string().contains("canonical lowercase UUID"),
        "unexpected error: {error}"
    );
    assert!(
        config
            .read_runtime_context(&session_id)
            .unwrap()
            .pinned_entities
            .is_empty()
    );
}

#[test]
pub(in super::super) fn two_sessions_attach_same_mission_without_shared_workflow_graph() {
    let tempdir = tempfile::tempdir().unwrap();
    let workspace = tempdir.path();
    let config = super::super::super::super::SessionStoreConfig::new(
        workspace.join(".workflow-tools").join("session"),
    );
    let mission_id = uuid::Uuid::new_v4();
    let mission_urn = format!("ce://default/mission/{mission_id}");
    let accepted_path = workspace
        .join(".workflow-tools/mission/missions")
        .join(mission_id.to_string())
        .join("accepted.json");
    std::fs::create_dir_all(accepted_path.parent().unwrap()).unwrap();
    std::fs::write(
        &accepted_path,
        serde_json::json!({
            "mission_id": mission_id,
            "bundle": { "manifest": { "title": "Shared mission" } }
        })
        .to_string(),
    )
    .unwrap();

    let first_id = uuid::Uuid::new_v4().to_string();
    let second_id = uuid::Uuid::new_v4().to_string();
    for session_id in [&first_id, &second_id] {
        config
            .init_runtime_context(crate::SessionRuntimeInitRequest {
                session_id: Some(session_id.clone()),
                ..Default::default()
            })
            .unwrap();
        config
            .pin_runtime_entity(session_id, &mission_urn, None, None)
            .unwrap();
    }

    for (session_id, node_id, title) in [
        (&first_id, "first-task", "First session task"),
        (&second_id, "second-task", "Second session task"),
    ] {
        config
            .workflow_add_node(
                session_id,
                crate::SessionWorkflowNodeDraft {
                    node_id: Some(node_id.into()),
                    kind: crate::SessionWorkflowNodeKind::Task,
                    requirement: crate::SessionWorkflowNodeRequirement::Optional,
                    title: title.into(),
                    ticket_urn: None,
                    spec_urn: None,
                    anchor_urn: None,
                    category: None,
                    cached_ticket_title: None,
                    validation_spec_id: None,
                },
            )
            .unwrap();
    }

    for session_id in [&first_id, &second_id] {
        assert_eq!(
            config
                .resolve_pinned_mission(session_id, &mission_urn)
                .unwrap()["mission_id"],
            mission_id.to_string()
        );
    }
    let first = config.read_runtime_context(&first_id).unwrap();
    let second = config.read_runtime_context(&second_id).unwrap();
    assert_eq!(first.pinned_entities[0].urn, mission_urn);
    assert_eq!(second.pinned_entities[0].urn, mission_urn);
    assert_eq!(first.workflow.nodes.len(), 1);
    assert_eq!(second.workflow.nodes.len(), 1);
    assert_eq!(first.workflow.nodes[0].node_id, "first-task");
    assert_eq!(second.workflow.nodes[0].node_id, "second-task");
}

#[test]
pub(in super::super) fn pinned_rule_render_contains_only_rule_pins_in_canonical_order() {
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
    let mut rule_store = rule_api::RuleStore::open_or_init(&store_root.join(".rule")).unwrap();

    let mut later = rule_api::RuleManifest::new(
        "session/render/later",
        "Later",
        ".instructions",
        "later",
        "Later guidance.",
    );
    later.set_order_key(20);
    let later_id = rule_store.create(&later, None).unwrap();
    let mut earlier = rule_api::RuleManifest::new(
        "session/render/earlier",
        "Earlier",
        ".instructions",
        "earlier",
        "Earlier guidance.",
    );
    earlier.set_order_key(10);
    let earlier_id = rule_store.create(&earlier, None).unwrap();

    config
        .pin_runtime_entity(
            &workspace_id,
            &format!("ce://default/rules/{later_id}"),
            None,
            None,
        )
        .unwrap();
    config
        .pin_runtime_entity(
            &workspace_id,
            "ce://default/tickets/11111111-1111-4111-8111-111111111111",
            None,
            None,
        )
        .unwrap();
    config
        .pin_runtime_entity(
            &workspace_id,
            &format!("ce://default/rules/{earlier_id}"),
            None,
            None,
        )
        .unwrap();

    let rendered = config
        .render_pinned_rule_instructions(&workspace_id)
        .unwrap();
    assert!(rendered.contains("Earlier guidance."));
    assert!(rendered.contains("Later guidance."));
    assert!(!rendered.contains("11111111-1111-4111-8111-111111111111"));
    assert!(
        rendered.find("Earlier guidance.").unwrap() < rendered.find("Later guidance.").unwrap()
    );
}

#[test]
pub(in super::super) fn pinned_rule_render_skips_missing_rule() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    rule_api::RuleStore::open_or_init(&store_root.join(".rule")).unwrap();
    config
        .pin_runtime_entity(
            &init.context.session_id,
            "ce://default/rules/22222222-2222-4222-8222-222222222222",
            None,
            None,
        )
        .unwrap();

    let rendered = config
        .render_pinned_rule_instructions(&init.context.session_id)
        .unwrap();
    assert!(!rendered.contains("22222222-2222-4222-8222-222222222222"));
}

#[test]
pub(in super::super) fn pinned_rule_render_succeeds_when_rule_store_is_absent() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    // No `.rule` directory created: the store is absent entirely.
    config
        .pin_runtime_entity(
            &init.context.session_id,
            "ce://default/rules/22222222-2222-4222-8222-222222222222",
            None,
            None,
        )
        .unwrap();

    let rendered = config
        .render_pinned_rule_instructions(&init.context.session_id)
        .unwrap();
    assert!(!rendered.contains("22222222-2222-4222-8222-222222222222"));
}
