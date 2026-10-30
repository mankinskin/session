#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn handoff_markdown_shows_open_escalations_warning() {
    let (config, _temp_dir) = setup_test_store();
    let session_id = "10000000-0000-4000-8000-000000000002";

    init_test_session(&config, session_id);

    let package = SessionHandoffPackage {
        objective: "Fix the bug".to_string(),
        target_tickets: vec![target_ticket("ticket-456")],
        higher_level_objective: String::new(),
        upward_context: vec![],
        target_files: vec!["session/crates/session-api/src/error.rs".to_string()],
        decisions: vec!["Decision made".to_string()],
        non_goals: vec!["Non-goal".to_string()],
        context_anchors: vec!["Anchor".to_string()],
        open_escalations: vec![
            "Need clarification".to_string(),
            "Blocked on upstream".to_string(),
        ],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let result = config
        .create_handoff_result(session_id, Some(package.clone()), vec![], None)
        .expect("create handoff result");

    let handoff_folder = PathBuf::from(&result.record_path);
    let md_path = handoff_folder.join("handoff.md");
    let md_content = std::fs::read_to_string(&md_path).expect("read handoff.md");

    // When open_escalations is not empty, implementation_ready should be false
    assert!(
        md_content.contains("Implementation Ready: false")
            || md_content.contains("**Implementation Ready**: false"),
        "markdown should show implementation_ready=false when escalations exist"
    );
    assert!(
        md_content.contains("Open Escalations"),
        "markdown should have an Open Escalations section"
    );
    assert!(
        md_content.contains("Need clarification"),
        "markdown should list first escalation"
    );
    assert!(
        md_content.contains("Blocked on upstream"),
        "markdown should list second escalation"
    );
}

#[test]
pub(super) fn handoff_markdown_includes_workflow_mermaid_diagram_when_nodes_exist() {
    let (config, _temp_dir) = setup_test_store();
    let session_id = "10000000-0000-4000-8000-000000000004";

    init_test_session(&config, session_id);

    config
        .workflow_add_node(
            session_id,
            SessionWorkflowNodeDraft {
                node_id: Some("node-a".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "Do the thing".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .expect("add workflow node");

    let package = SessionHandoffPackage {
        objective: "Ship the workflow".to_string(),
        target_tickets: vec![],
        higher_level_objective: "Deliver the program objective".to_string(),
        upward_context: upward_context(),
        target_files: vec![],
        decisions: vec![],
        non_goals: vec![],
        context_anchors: vec![],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let result = config
        .create_handoff_result(
            session_id,
            Some(package),
            vec![],
            Some(&AvailableTicketResolver),
        )
        .expect("create handoff result");

    let handoff_folder = PathBuf::from(&result.record_path);
    let md_content =
        std::fs::read_to_string(handoff_folder.join("handoff.md")).expect("read handoff.md");

    assert!(
        md_content.contains("```mermaid"),
        "markdown should contain a fenced mermaid block"
    );
    assert!(
        md_content.contains("flowchart TD"),
        "mermaid block should contain flowchart TD"
    );
    assert!(
        md_content.contains("node_a"),
        "mermaid block should contain a line for the seeded node"
    );
    assert!(
        md_content.contains("\n\n```mermaid"),
        "fence must be preceded by a blank line or Markdown treats it as list continuation"
    );
}

#[test]
pub(super) fn handoff_markdown_omits_mermaid_diagram_when_workflow_empty() {
    let (config, _temp_dir) = setup_test_store();
    let session_id = "10000000-0000-4000-8000-000000000005";

    init_test_session(&config, session_id);

    let package = SessionHandoffPackage {
        objective: "No workflow yet".to_string(),
        target_tickets: vec![],
        higher_level_objective: "Deliver the program objective".to_string(),
        upward_context: upward_context(),
        target_files: vec![],
        decisions: vec![],
        non_goals: vec![],
        context_anchors: vec![],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let result = config
        .create_handoff_result(session_id, Some(package), vec![], None)
        .expect("create handoff result");

    let handoff_folder = PathBuf::from(&result.record_path);
    let md_content =
        std::fs::read_to_string(handoff_folder.join("handoff.md")).expect("read handoff.md");

    assert!(
        !md_content.contains("```mermaid"),
        "markdown should not contain a fenced mermaid block when workflow has no nodes"
    );
}

#[test]
pub(super) fn handoff_markdown_renders_upward_context_and_resolved_ticket_table() {
    let (config, store_root) = setup_test_store();
    let session_id = "10000000-0000-4000-8000-000000000006";
    let epic_id =
        uuid::Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").expect("valid epic id");
    let phase_id =
        uuid::Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").expect("valid phase id");
    let ticket_id =
        uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111").expect("valid ticket id");
    let ticket_store_root = store_root.path().join(".ticket");
    let ticket_store = ticket_api::storage::TicketStore::open_or_init(&ticket_store_root)
        .expect("open ticket store");
    for (id, title) in [(epic_id, "Program objective"), (phase_id, "Delivery phase")] {
        ticket_store
            .create(
                Some(id),
                "task",
                Some(title),
                None,
                BTreeMap::new(),
                None,
                None,
            )
            .expect("create ticket");
    }
    ticket_store
        .create(
            Some(ticket_id),
            "task",
            Some("Resolved ticket title"),
            None,
            BTreeMap::new(),
            None,
            Some("Resolve the handoff rendering transport."),
        )
        .expect("create ticket");
    init_test_session(&config, session_id);
    config
        .workflow_add_node(
            session_id,
            SessionWorkflowNodeDraft {
                node_id: Some("workflow-ticket".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: format!("Keep {ticket_id} literal in Mermaid"),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .expect("add workflow node");

    let mut ticket = target_ticket(&ticket_id.to_string());
    ticket.why = "The next session needs the renderer contract.".to_string();
    let package = SessionHandoffPackage {
        objective: "Render the handoff".to_string(),
        target_tickets: vec![ticket],
        higher_level_objective: "Ship a useful handoff package.".to_string(),
        upward_context: vec![
            SessionHandoffUpwardContextEntry {
                entity_urn: format!("ce://default/ticket/{epic_id}"),
                title: "Program objective".to_string(),
                role: SessionHandoffUpwardContextRole::Epic,
            },
            SessionHandoffUpwardContextEntry {
                entity_urn: format!("ce://default/ticket/{phase_id}"),
                title: "Delivery phase".to_string(),
                role: SessionHandoffUpwardContextRole::Phase,
            },
        ],
        target_files: vec![],
        decisions: vec![],
        non_goals: vec![],
        context_anchors: vec![format!(
            "Program {epic_id} is linked; [already {phase_id}](https://example.test) and `{ticket_id}` stay literal; 22222222 is unresolved."
        )],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let result = config
        .create_handoff_result(session_id, Some(package), vec![], None)
        .expect("create handoff result");
    let markdown = std::fs::read_to_string(PathBuf::from(result.record_path).join("handoff.md"))
        .expect("read handoff markdown");

    assert!(markdown.starts_with(&format!(
        "# Handoff: {}\n\nShip a useful handoff package.",
        result.record.handoff_id
    )));
    assert!(markdown.contains(&format!(
        "[aaaaaaaa Program objective](.ticket/tickets/{epic_id}/ticket.toml) (epic) -> [bbbbbbbb Delivery phase](.ticket/tickets/{phase_id}/ticket.toml) (phase) -> [11111111 Resolved ticket title](.ticket/tickets/{ticket_id}/ticket.toml)"
    )));
    assert!(markdown.contains("| Ticket | What it does | Why |"));
    assert!(markdown.contains("[11111111 Resolved ticket title](.ticket/tickets/11111111-1111-4111-8111-111111111111/ticket.toml)"));
    assert!(markdown.contains("Resolve the handoff rendering transport."));
    assert!(markdown.contains("The next session needs the renderer contract."));
    assert!(markdown.contains(&format!(
        "Program [aaaaaaaa Program objective](.ticket/tickets/{epic_id}/ticket.toml) is linked; [already {phase_id}](https://example.test) and `{ticket_id}` stay literal; 22222222 is unresolved."
    )));
    assert!(markdown.contains(&format!("```bash\n{}\n```", result.record.resume_command)));
    assert!(markdown.contains(&format!(
        "workflow_ticket[\"Keep {ticket_id} literal in Mermaid"
    )));
}

#[test]
pub(super) fn handoff_markdown_degrades_when_target_ticket_is_unresolvable() {
    let (config, _temp_dir) = setup_test_store();
    let session_id = "10000000-0000-4000-8000-000000000009";
    let ticket_id = "22222222-2222-4222-8222-222222222222";
    init_test_session(&config, session_id);
    let package = SessionHandoffPackage {
        objective: "Degrade gracefully".to_string(),
        target_tickets: vec![target_ticket(ticket_id)],
        higher_level_objective: "Keep rendered handoffs available.".to_string(),
        upward_context: upward_context(),
        target_files: vec![],
        decisions: vec![],
        non_goals: vec![],
        context_anchors: vec![],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let result = config
        .create_handoff_result(session_id, Some(package), vec![], None)
        .expect("create handoff result");
    let markdown = std::fs::read_to_string(PathBuf::from(result.record_path).join("handoff.md"))
        .expect("read handoff markdown");

    assert!(markdown.contains("22222222-2222-4222-8222-222222222222"));
}
