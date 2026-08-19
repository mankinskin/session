// T-KINDS regression coverage: behavioral `Spec` node gating is symmetric to
// `Ticket`, the open descriptive `category` field never affects gating, and
// legacy cosmetic kinds deserialize as `Task` for back-compat.

/// A resolver returning a caller-controlled live spec state for a specific URN.
/// Ticket resolution is intentionally unsupported so the tests exercise the
/// spec resolution path exclusively.
struct FixedSpecStateResolver {
    urn: String,
    state: Option<String>,
}

impl SessionTicketStateResolver for FixedSpecStateResolver {
    fn resolve_ticket_state(
        &self,
        ticket_urn: &str,
    ) -> Result<Option<String>, String> {
        Err(format!("unexpected ticket urn: {ticket_urn}"))
    }

    fn resolve_spec_state(
        &self,
        spec_urn: &str,
    ) -> Result<Option<String>, String> {
        if spec_urn == self.urn {
            Ok(self.state.clone())
        } else {
            Err(format!("unexpected spec urn: {spec_urn}"))
        }
    }
}

fn spec_workspace() -> (SessionStoreConfig, String, TempDir) {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;
    (config, workspace_id, tempdir)
}

/// A required `Spec` node whose live state cannot be resolved fails finish
/// closed with an explicit unavailable reason (fail closed), symmetric to the
/// `Ticket` availability gate.
#[test]
fn workflow_finish_blocks_spec_node_with_unavailable_state() {
    let (config, workspace_id, _tempdir) = spec_workspace();
    let spec_urn =
        "ce://context-engine/specs/11111111-1111-4111-8111-111111111111";
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("spec-node".to_string()),
                kind: SessionWorkflowNodeKind::Spec,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "backing spec".to_string(),
                ticket_urn: None,
                spec_urn: Some(spec_urn.to_string()),
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();

    // The default resolver reports spec resolution as unavailable, which must
    // fail closed rather than silently pass.
    let error = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap_err();
    let SessionError::FinishBlocked { reason } = error else {
        panic!("expected FinishBlocked, got {error:?}");
    };
    assert!(
        reason.contains("spec") && reason.contains("unavailable"),
        "expected spec unavailable diagnostic, got: {reason}"
    );
}

/// A required `Spec` node whose live state is a non-terminal spec state blocks
/// finish; a terminal spec state (`verified`) permits finish.
#[test]
fn workflow_finish_gates_spec_node_on_terminal_state() {
    let (config, workspace_id, _tempdir) = spec_workspace();
    let spec_urn =
        "ce://context-engine/specs/22222222-2222-4222-8222-222222222222";
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("spec-node".to_string()),
                kind: SessionWorkflowNodeKind::Spec,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "backing spec".to_string(),
                ticket_urn: None,
                spec_urn: Some(spec_urn.to_string()),
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();

    // Non-terminal live spec state blocks finish.
    let non_terminal = FixedSpecStateResolver {
        urn: spec_urn.to_string(),
        state: Some("draft".to_string()),
    };
    let error = config
        .finish_workflow(&workspace_id, vec![], vec![], Some(&non_terminal))
        .unwrap_err();
    assert!(matches!(error, SessionError::FinishBlocked { .. }));

    // Terminal live spec state (verified) permits finish.
    let terminal = FixedSpecStateResolver {
        urn: spec_urn.to_string(),
        state: Some("verified".to_string()),
    };
    let finished = config
        .finish_workflow(&workspace_id, vec![], vec![], Some(&terminal))
        .unwrap();
    assert!(!finished.already_finished);
}

/// The open descriptive `category` field never affects finish gating: a
/// required `Task` node carrying a category still finishes on local `Done`.
#[test]
fn workflow_finish_ignores_descriptive_category() {
    let (config, workspace_id, _tempdir) = spec_workspace();
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("task-node".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "descriptive work".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: Some("investigation".to_string()),
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();
    config
        .workflow_update_node_status(
            &workspace_id,
            "task-node",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();

    let finished = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap();
    assert!(!finished.already_finished);

    // The category round-trips on the persisted node and does not gate.
    let context = config.read_runtime_context(&workspace_id).unwrap();
    let node = context
        .workflow
        .nodes
        .iter()
        .find(|node| node.node_id == "task-node")
        .unwrap();
    assert_eq!(node.category.as_deref(), Some("investigation"));
}

/// Legacy cosmetic node kinds (`action`, `decision`, `checkpoint`) deserialize
/// as the generic `Task` bucket and re-serialize as `task`, so existing
/// persisted runtime contexts keep loading.
#[test]
fn legacy_node_kinds_deserialize_as_task() {
    for legacy in ["action", "decision", "checkpoint"] {
        let kind: SessionWorkflowNodeKind =
            serde_json::from_str(&format!("\"{legacy}\"")).unwrap();
        assert_eq!(kind, SessionWorkflowNodeKind::Task);
    }
    assert_eq!(
        serde_json::to_string(&SessionWorkflowNodeKind::Task).unwrap(),
        "\"task\""
    );

    // A full persisted node using a legacy kind deserializes without error.
    let node_json = r#"{
        "node_id": "legacy",
        "kind": "checkpoint",
        "requirement": "optional",
        "status": "pending",
        "title": "legacy node",
        "created_at": "2026-06-02T13:00:00Z",
        "updated_at": "2026-06-02T13:00:00Z"
    }"#;
    let node: crate::SessionWorkflowNode =
        serde_json::from_str(node_json).unwrap();
    assert_eq!(node.kind, SessionWorkflowNodeKind::Task);
    assert!(node.spec_urn.is_none());
    assert!(node.category.is_none());
}
