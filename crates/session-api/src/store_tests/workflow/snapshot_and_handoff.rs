
#[test]
fn workflow_snapshot_resolves_live_state_and_emits_missing_diagnostics() {
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
                node_id: Some("node-live".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "Existing ticket".to_string(),
                ticket_urn: Some(
                    "ce://default/tickets/412964a3-e1c3-47da-94ad-268ff20441c0"
                        .to_string(),
                ),
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();
    let missing_urn =
        "ce://default/tickets/deadbeef-dead-beef-dead-beefdeadbeef";
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
            .any(|diag| diag.node_id == "node-missing"
                && diag.code == "ticket-state-unavailable")
    );
}

#[test]
fn workflow_render_outputs_are_deterministic_and_escaped() {
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
                    "ce://default/tickets/deadbeef-dead-beef-dead-beefdeadbeef"
                        .to_string(),
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
        missing_urn:
            "ce://default/tickets/deadbeef-dead-beef-dead-beefdeadbeef"
                .to_string(),
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
fn workflow_render_is_read_only_for_runtime_persistence() {
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

#[test]
fn handoff_persists_before_render_and_resume_links_new_run() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    let _rendered = config
        .render_handoff_terminal(
            &workspace_id,
            None,
            vec![crate::SessionValidationGate {
                validation_spec_id: "val-session-handoff-continuity"
                    .to_string(),
                required: true,
                outcome: Some("passed".to_string()),
                command: None,
            }],
            None,
        )
        .unwrap();

    let paths = config.runtime_paths_for_workspace(&workspace_id).unwrap();
    let handoff_files = std::fs::read_dir(&paths.handoffs_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(handoff_files.len(), 1);

    // Handoffs are now stored as folders; read handoff.json from inside
    let handoff_folder = handoff_files[0].path();
    assert!(handoff_folder.is_dir(), "handoff should be a folder");
    let handoff_json_path = handoff_folder.join("handoff.json");
    let handoff: crate::SessionHandoffRecord =
        serde_json::from_slice(&std::fs::read(handoff_json_path).unwrap()).unwrap();
    assert_eq!(handoff.session_id, workspace_id);
    assert_eq!(handoff.outgoing_run_id, init.context.active_run_id);
    assert!(handoff.resume_command.contains(&workspace_id));
    assert!(handoff.resume_command.contains(&handoff.outgoing_run_id));

    let resumed = config
        .resume_workspace_context(&workspace_id, &handoff.outgoing_run_id)
        .unwrap();
    assert_eq!(resumed.context.session_id, workspace_id);
    assert_ne!(resumed.run.run_id, handoff.outgoing_run_id);
    assert_eq!(
        resumed.run.predecessor_run_id.as_deref(),
        Some(handoff.outgoing_run_id.as_str())
    );
}

#[test]
fn handoff_package_missing_objective_is_rejected() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(crate::SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    let result = config.create_handoff_record(
        &workspace_id,
        Some(crate::SessionHandoffPackage {
            objective: "".to_string(), // missing
            target_tickets: vec![],
            ..Default::default()
        }),
        vec![],
        None,
    );

    assert!(
        result.is_err(),
        "expected Err for incomplete package but got Ok"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("incomplete") || err.contains("missing"),
        "unexpected error message: {err}"
    );
}

#[test]
fn handoff_package_round_trip_persists_schema_fields() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(crate::SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    let package = crate::SessionHandoffPackage {
        objective: "Implement required-field enforcement".to_string(),
        target_tickets: vec![crate::SessionHandoffTargetTicket {
            id: "d3af78d7-9486-43c0-aae7-ddd5681d9807".to_string(),
            why: "Implement the handoff contract".to_string(),
            state: "ready".to_string(),
            acceptance_criteria: vec![],
        }],
        higher_level_objective: "Complete the program work".to_string(),
        upward_context: vec![crate::SessionHandoffUpwardContextEntry {
            entity_urn: "ce://default/ticket/program".to_string(),
            title: "Program".to_string(),
            role: crate::SessionHandoffUpwardContextRole::Epic,
        }],
        target_files: vec![
            "workflow-tools/session/crates/session-api/src/model/handoff.rs".to_string(),
        ],
        decisions: vec!["Use Option<SessionHandoffPackage> for backward compat"
            .to_string()],
        non_goals: vec!["UI/viewer representation".to_string()],
        context_anchors: vec!["spec:5e52039d".to_string()],
        open_escalations: vec![],
        risk_notes: Some("Blocked on upstream ticket ownership".to_string()),
        predecessor_handoff: Some("handoff-previous-0001".to_string()),
    };

    let result = config
        .create_handoff_result(&workspace_id, Some(package.clone()), vec![], None)
        .expect("handoff with complete package");

    // Contract: every field accepted by `session_handoff` gets an explicit
    // round-trip assertion here so schema drift is auditable.
    assert_eq!(result.record.objective, package.objective);
    assert_eq!(result.record.target_tickets, package.target_tickets);
    assert_eq!(result.record.target_files, package.target_files);
    assert_eq!(result.record.decisions, package.decisions);
    assert_eq!(result.record.non_goals, package.non_goals);
    assert_eq!(result.record.context_anchors, package.context_anchors);
    assert!(result.record.open_escalations.is_empty());
    assert_eq!(result.record.risk_notes, package.risk_notes);
    assert_eq!(result.record.predecessor_handoff, package.predecessor_handoff);
    assert!(result.render.contains("implementation_ready: true"));
    assert!(result.render.contains("objective:"));

    // Verify the record is persisted and can be re-read from disk.
    let paths = config.runtime_paths_for_workspace(&workspace_id).unwrap();
    // Handoffs are now stored as folders with handoff.json inside
    let handoff_folder =
        paths.handoffs_dir.join(&result.record.handoff_id);
    let handoff_json_path = handoff_folder.join("handoff.json");
    let on_disk: crate::SessionHandoffRecord =
        serde_json::from_slice(&std::fs::read(&handoff_json_path).unwrap()).unwrap();
    assert_eq!(on_disk.objective, package.objective);
    assert_eq!(on_disk.target_tickets, package.target_tickets);
    assert_eq!(on_disk.target_files, package.target_files);
    assert_eq!(on_disk.decisions, package.decisions);
    assert_eq!(on_disk.non_goals, package.non_goals);
    assert_eq!(on_disk.context_anchors, package.context_anchors);
    assert_eq!(on_disk.open_escalations, package.open_escalations);
    assert_eq!(on_disk.risk_notes, package.risk_notes);
    assert_eq!(on_disk.predecessor_handoff, package.predecessor_handoff);
}

#[test]
fn handoff_package_with_nonexistent_target_file_fails_at_creation_time() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(crate::SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    let package = crate::SessionHandoffPackage {
        objective: "Implement a nonexistent-path regression".to_string(),
        target_tickets: vec![crate::SessionHandoffTargetTicket {
            id: "d3af78d7-9486-43c0-aae7-ddd5681d9807".to_string(),
            why: "Verify creation-time validation".to_string(),
            state: "ready".to_string(),
            acceptance_criteria: vec![],
        }],
        higher_level_objective: "Complete the program work".to_string(),
        upward_context: vec![crate::SessionHandoffUpwardContextEntry {
            entity_urn: "ce://default/ticket/program".to_string(),
            title: "Program".to_string(),
            role: crate::SessionHandoffUpwardContextRole::Epic,
        }],
        target_files: vec![
            "workflow-tools/session/crates/session-api/src/does_not_exist.rs".to_string(),
        ],
        decisions: vec!["n/a".to_string()],
        non_goals: vec!["n/a".to_string()],
        context_anchors: vec!["spec:5e52039d".to_string()],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };

    // AC2: creation-time failure, not consumption-time.
    let result =
        config.create_handoff_record(&workspace_id, Some(package), vec![], None);
    let err = result.expect_err(
        "handoff with a non-existent target_files path must fail at \
         creation time",
    );
    assert!(
        matches!(err, crate::SessionError::HandoffPathNotFound { .. }),
        "unexpected error variant: {err:?}"
    );

    // No handoff folder should have been persisted for the rejected package.
    let paths = config.runtime_paths_for_workspace(&workspace_id).unwrap();
    let handoff_count = std::fs::read_dir(&paths.handoffs_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(
        handoff_count, 0,
        "a rejected handoff package must not be written to disk"
    );
}

#[test]
fn handoff_package_validates_target_files_in_assigned_worktree() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let session_id = uuid::Uuid::new_v4().to_string();
    let worktree = managed_worktree(
        &tempdir,
        &session_id,
        "handoff-worktree",
        "agent/handoff-worktree",
    );
    let target_file = worktree.join("handoff-fixtures/assigned-only.txt");
    std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
    std::fs::write(&target_file, "---\nname: Explainer Agent\n---\n").unwrap();

    config
        .check_in_worktree(crate::SessionWorktreeCheckInRequest {
            session_id: session_id.clone(),
            owner_id: "copilot".to_string(),
            ticket_id: "ticket-handoff-worktree".to_string(),
            worktree_path: worktree,
            branch: "agent/handoff-worktree".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();
    config
        .init_runtime_context(crate::SessionRuntimeInitRequest {
            session_id: Some(session_id.clone()),
            ..Default::default()
        })
        .unwrap();

    let package = crate::SessionHandoffPackage {
        objective: "Validate an assigned-worktree handoff path".to_string(),
        target_tickets: vec![],
        higher_level_objective: "Keep handoffs session-anchored".to_string(),
        upward_context: vec![],
        target_files: vec!["handoff-fixtures/assigned-only.txt".to_string()],
        decisions: vec![],
        non_goals: vec![],
        context_anchors: vec![],
        open_escalations: vec!["Regression test is not implementation-ready".to_string()],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let handoff = config
        .create_handoff_record(&session_id, Some(package), vec![], None)
        .expect("target file in the assigned worktree must be accepted");

    assert_eq!(
        handoff.target_files,
        vec!["handoff-fixtures/assigned-only.txt".to_string()]
    );
}

#[test]
fn handoff_package_normalizes_backslash_target_files_to_forward_slash() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(crate::SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    let package = crate::SessionHandoffPackage {
        objective: "Verify repo-root-relative forward-slash normalization"
            .to_string(),
        target_tickets: vec![crate::SessionHandoffTargetTicket {
            id: "d3af78d7-9486-43c0-aae7-ddd5681d9807".to_string(),
            why: "Verify path normalization".to_string(),
            state: "ready".to_string(),
            acceptance_criteria: vec![],
        }],
        higher_level_objective: "Complete the program work".to_string(),
        upward_context: vec![crate::SessionHandoffUpwardContextEntry {
            entity_urn: "ce://default/ticket/program".to_string(),
            title: "Program".to_string(),
            role: crate::SessionHandoffUpwardContextRole::Epic,
        }],
        // A real, existing repo file referenced with backslashes, as a
        // Windows-authored handoff payload might supply.
        target_files: vec![
            "workflow-tools\\session\\crates\\session-api\\src\\model\\handoff.rs"
                .to_string(),
        ],
        decisions: vec!["n/a".to_string()],
        non_goals: vec!["n/a".to_string()],
        // Store-qualified nested-store anchor (AC1: verified-to-exist path).
        context_anchors: vec![
            "workflow-tools/session/crates/session-api/src/model/handoff.rs".to_string(),
        ],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let record = config
        .create_handoff_record(&workspace_id, Some(package), vec![], None)
        .expect("valid repo-root-relative paths must pass creation-time validation");

    assert_eq!(
        record.target_files,
        vec![
            "workflow-tools/session/crates/session-api/src/model/handoff.rs".to_string()
        ],
        "target_files must be normalized to forward-slash form"
    );
    assert!(
        !record.target_files[0].contains('\\'),
        "no backslashes should remain in a persisted target_files entry"
    );
}

#[test]
fn legacy_inline_handoff_package_still_deserializes() {
    #[derive(serde::Deserialize)]
    struct LegacyInlineHandoffPackage {
        objective: String,
        target_tickets: Vec<String>,
        target_files: Vec<String>,
        decisions: Vec<String>,
        non_goals: Vec<String>,
        context_anchors: Vec<String>,
        open_escalations: Vec<String>,
        risk_notes: Option<String>,
        predecessor_handoff: Option<String>,
    }

    let legacy_json = r#"{
        "objective": "Fix the serialization regression",
        "target_tickets": ["ticket-123"],
        "target_files": ["src/lib.rs"],
        "decisions": ["Keep the inline package shape"],
        "non_goals": ["No schema migration"],
        "context_anchors": ["old session transcript"],
        "open_escalations": [],
        "risk_notes": "Back-compat must remain intact",
        "predecessor_handoff": "handoff-inline-0007"
    }"#;

    let legacy: LegacyInlineHandoffPackage = serde_json::from_str(legacy_json)
        .expect("deserialize legacy inline handoff package");

    assert_eq!(legacy.objective, "Fix the serialization regression");
    assert_eq!(legacy.target_tickets, vec!["ticket-123"]);
    assert_eq!(legacy.target_files, vec!["src/lib.rs"]);
    assert_eq!(legacy.decisions, vec!["Keep the inline package shape"]);
    assert_eq!(legacy.non_goals, vec!["No schema migration"]);
    assert_eq!(legacy.context_anchors, vec!["old session transcript"]);
    assert!(legacy.open_escalations.is_empty());
    assert_eq!(legacy.risk_notes.as_deref(), Some("Back-compat must remain intact"));
    assert_eq!(legacy.predecessor_handoff.as_deref(), Some("handoff-inline-0007"));
}

#[test]
fn handoff_package_with_open_escalations_persists_but_not_ready() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(crate::SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    let package = crate::SessionHandoffPackage {
        objective: "Implement something".to_string(),
        open_escalations: vec!["Must resolve cross-crate dep question first"
            .to_string()],
        ..Default::default()
    };

    let result = config
        .create_handoff_result(&workspace_id, Some(package), vec![], None)
        .expect("handoff with escalations should persist");

    assert!(!result.record.open_escalations.is_empty());
    assert!(result.render.contains("implementation_ready: false"));
    assert!(result.render.contains("open_escalations:"));
}
