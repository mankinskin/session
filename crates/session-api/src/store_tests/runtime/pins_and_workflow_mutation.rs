#[test]
fn pinned_rule_render_contains_only_rule_pins_in_canonical_order() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;
    let mut rule_store =
        rule_api::RuleStore::open_or_init(&store_root.join(".rule")).unwrap();

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
            &format!("ce://context-engine/rules/{later_id}"),
            None,
            None,
        )
        .unwrap();
    config
        .pin_runtime_entity(
            &workspace_id,
            "ce://context-engine/tickets/11111111-1111-4111-8111-111111111111",
            None,
            None,
        )
        .unwrap();
    config
        .pin_runtime_entity(
            &workspace_id,
            &format!("ce://context-engine/rules/{earlier_id}"),
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
        rendered.find("Earlier guidance.").unwrap()
            < rendered.find("Later guidance.").unwrap()
    );
}

#[test]
fn pinned_rule_render_skips_missing_rule() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    rule_api::RuleStore::open_or_init(&store_root.join(".rule")).unwrap();
    config
        .pin_runtime_entity(
            &init.context.session_id,
            "ce://context-engine/rules/22222222-2222-4222-8222-222222222222",
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
fn pinned_rule_render_succeeds_when_rule_store_is_absent() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    // No `.rule` directory created: the store is absent entirely.
    config
        .pin_runtime_entity(
            &init.context.session_id,
            "ce://context-engine/rules/22222222-2222-4222-8222-222222222222",
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
fn context_capture_persistence_isolation_is_byte_stable() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    let capture = config
        .persist_capture(sample_request(
            "session-isolation",
            Some("conversation-isolation"),
            sample_time(),
            &["capture first"],
        ))
        .unwrap();
    let manifest_before = std::fs::read(&capture.paths.manifest_path).unwrap();
    let transcript_before =
        std::fs::read(&capture.paths.transcript_path).unwrap();

    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;
    config
        .pin_runtime_entity(
            &workspace_id,
            "ce://default/rules/084fd4e6-660b-4227-a13e-514edf44e393",
            Some("handoff".to_string()),
            None,
        )
        .unwrap();

    let manifest_after = std::fs::read(&capture.paths.manifest_path).unwrap();
    let transcript_after =
        std::fs::read(&capture.paths.transcript_path).unwrap();
    assert_eq!(manifest_before, manifest_after);
    assert_eq!(transcript_before, transcript_after);

    let manifest_path = config
        .paths_for_session_id(&workspace_id)
        .unwrap()
        .manifest_path;
    let runtime_before = std::fs::read(&manifest_path).unwrap();

    config
        .persist_capture(sample_request(
            "session-isolation",
            Some("conversation-isolation"),
            sample_time_later(),
            &["capture first", "capture second"],
        ))
        .unwrap();

    let runtime_after = std::fs::read(&manifest_path).unwrap();
    assert_eq!(runtime_before, runtime_after);
}

struct MockTicketResolver {
    missing_urn: String,
}

impl SessionTicketStateResolver for MockTicketResolver {
    fn resolve_ticket_state(
        &self,
        ticket_urn: &str,
    ) -> Result<Option<String>, String> {
        if ticket_urn == self.missing_urn {
            Err("ticket not found".to_string())
        } else {
            Ok(Some("in-review".to_string()))
        }
    }
}

#[test]
fn workflow_persists_mutation_and_reload() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    let after_ticket = config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("node-ticket".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "Implement runtime model".to_string(),
                ticket_urn: Some(
                    "ce://default/tickets/412964a3-e1c3-47da-94ad-268ff20441c0"
                        .to_string(),
                ),
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: Some(
                    "Runtime session context".to_string(),
                ),
                validation_spec_id: None,
            },
        )
        .unwrap();
    assert_eq!(after_ticket.workflow.nodes.len(), 1);

    let after_action = config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("node-action".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "Write workflow tests".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: Some(
                    "ce://default/tickets/412964a3-e1c3-47da-94ad-268ff20441c0"
                        .to_string(),
                ),
                category: Some("review-criterion".to_string()),
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();
    assert_eq!(after_action.workflow.nodes.len(), 2);

    let linked = config
        .workflow_add_edge(
            &workspace_id,
            "node-action",
            "node-ticket",
            SessionWorkflowEdgeKind::DependsOn,
        )
        .unwrap();
    assert_eq!(linked.workflow.edges.len(), 1);

    let updated = config
        .workflow_update_node_status(
            &workspace_id,
            "node-action",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();
    assert_eq!(
        updated
            .workflow
            .nodes
            .iter()
            .find(|node| node.node_id == "node-action")
            .unwrap()
            .status,
        SessionWorkflowNodeStatus::Done
    );

    let reloaded = config.read_runtime_context(&workspace_id).unwrap();
    assert_eq!(reloaded.workflow.nodes.len(), 2);
    assert_eq!(reloaded.workflow.edges.len(), 1);
    let reloaded_action = reloaded
        .workflow
        .nodes
        .iter()
        .find(|node| node.node_id == "node-action")
        .unwrap();
    assert_eq!(
        reloaded_action.anchor_urn.as_deref(),
        Some("ce://default/tickets/412964a3-e1c3-47da-94ad-268ff20441c0")
    );
    assert_eq!(
        reloaded_action.category.as_deref(),
        Some("review-criterion")
    );
}

#[test]
fn workflow_promotion_preserves_node_identity() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("node-temp".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Optional,
                title: "Investigate follow-up".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();

    let promoted = config
        .workflow_promote_node_to_ticket(
            &workspace_id,
            "node-temp",
            "ce://default/tickets/70cd7056-c342-4433-ad60-5bc798f61aa6",
            Some("Workflow persistence".to_string()),
        )
        .unwrap();

    let node = promoted
        .workflow
        .nodes
        .iter()
        .find(|node| node.node_id == "node-temp")
        .unwrap();
    assert_eq!(node.kind, SessionWorkflowNodeKind::Ticket);
    assert_eq!(
        node.ticket_urn.as_deref(),
        Some("ce://default/tickets/70cd7056-c342-4433-ad60-5bc798f61aa6")
    );
}

#[test]
fn workflow_ticket_node_rejects_non_ticket_urn() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();

    let error = config
        .workflow_add_node(
            &init.context.session_id,
            SessionWorkflowNodeDraft {
                node_id: Some("node-ticket".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "bad type".to_string(),
                ticket_urn: Some(
                    "ce://default/specs/709f067a-21b6-41b6-8879-3cacef4bacaf"
                        .to_string(),
                ),
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap_err();

    assert!(matches!(error, SessionError::InvalidHookInput(_)));
}

#[test]
fn workflow_batches_are_atomic_and_preserve_duplicate_no_ops() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;
    let draft = |node_id: &str, title: &str| SessionWorkflowNodeDraft {
        node_id: Some(node_id.to_string()),
        kind: SessionWorkflowNodeKind::Task,
        requirement: SessionWorkflowNodeRequirement::Optional,
        title: title.to_string(),
        ticket_urn: None,
        spec_urn: None,
        anchor_urn: None,
        category: None,
        cached_ticket_title: None,
        validation_spec_id: None,
    };

    let node_error = config
        .workflow_add_nodes(
            &workspace_id,
            vec![draft("a", "first"), draft("bad", " ")],
        )
        .unwrap_err();
    assert!(node_error.to_string().contains("nodes[1]"));
    assert!(
        config
            .read_runtime_context(&workspace_id)
            .unwrap()
            .workflow
            .nodes
            .is_empty()
    );

    let nodes = config
        .workflow_add_nodes(
            &workspace_id,
            vec![
                draft("a", "first"),
                draft("b", "second"),
                draft("a", "duplicate"),
            ],
        )
        .unwrap();
    assert_eq!(nodes.workflow.nodes.len(), 2);

    let edge = |from: &str, to: &str| crate::SessionWorkflowEdge {
        from: from.to_string(),
        to: to.to_string(),
        kind: SessionWorkflowEdgeKind::DependsOn,
    };
    let edge_error = config
        .workflow_add_edges(
            &workspace_id,
            vec![edge("a", "b"), edge("a", "missing")],
        )
        .unwrap_err();
    assert!(edge_error.to_string().contains("edges[1]"));
    assert!(
        config
            .read_runtime_context(&workspace_id)
            .unwrap()
            .workflow
            .edges
            .is_empty()
    );

    let edges = config
        .workflow_add_edges(
            &workspace_id,
            vec![edge("a", "b"), edge("b", "a"), edge("a", "b")],
        )
        .unwrap();
    assert_eq!(edges.workflow.edges.len(), 2);
}

#[test]
fn session_run_lineage_round_trip() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    // Create the initial runtime context (first run).
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let ctx = &init.context;
    let session_id = ctx.canonical_session_id();
    let first_run_id = init.run.run_id.clone();

    // The first run must be stamped with the session id.
    assert_eq!(
        init.run.captured_session_id.as_deref(),
        Some(session_id.as_str()),
        "first run must carry captured_session_id"
    );

    // Force a second run on the same workspace.
    let resume = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(ctx.session_id.clone()),
            force_new_run: true,
            predecessor_run_id: None,
        })
        .unwrap();
    let second_run_id = resume.run.run_id.clone();

    assert_eq!(
        resume.run.captured_session_id.as_deref(),
        Some(session_id.as_str()),
        "second run must also carry captured_session_id"
    );

    // Read back and verify both-direction navigation.
    let ctx2 = config
        .read_runtime_context(&ctx.session_id)
        .unwrap();

    let runs = ctx2.runs_for_session(&session_id);
    let run_ids: Vec<&str> = runs.iter().map(|r| r.run_id.as_str()).collect();
    assert!(
        run_ids.contains(&first_run_id.as_str()),
        "runs_for_session must include first run"
    );
    assert!(
        run_ids.contains(&second_run_id.as_str()),
        "runs_for_session must include second run"
    );

    assert_eq!(
        ctx2.session_for_run(&first_run_id),
        Some(session_id.as_str()),
        "session_for_run must return session_id for first run"
    );
    assert_eq!(
        ctx2.session_for_run(&second_run_id),
        Some(session_id.as_str()),
        "session_for_run must return session_id for second run"
    );
    assert_eq!(
        ctx2.session_for_run("nonexistent-run-id"),
        None,
        "session_for_run must return None for unknown run"
    );
}

#[test]
fn read_runtime_context_missing_surfaces_not_found() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let err = config
        .read_runtime_context("11111111-1111-4111-8111-111111111111")
        .unwrap_err();

    assert!(
        matches!(err, SessionError::RuntimeContextNotFound { .. }),
        "expected RuntimeContextNotFound, got {err:?}"
    );
}

#[test]
fn writes_never_target_legacy_runtime_tree() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id.clone();

    config
        .pin_runtime_entity(
            &workspace_id,
            "ce://default/tickets/11111111-1111-4111-8111-111111111111",
            Some("test-relation".to_string()),
            Some("test-reason".to_string()),
        )
        .unwrap();

    let legacy_runtime_dir = store_root.join("runtime");
    assert!(
        !legacy_runtime_dir.exists(),
        "no write should ever create a legacy .session/runtime/ tree, found: {}",
        legacy_runtime_dir.display()
    );
}
