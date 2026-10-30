#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn handoff_persists_before_render_and_resume_links_new_run() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    let _rendered = config
        .render_handoff_terminal(
            &workspace_id,
            None,
            vec![crate::SessionValidationGate {
                validation_spec_id: "val-session-handoff-continuity".to_string(),
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
pub(super) fn handoff_package_missing_objective_is_rejected() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(crate::SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
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
pub(super) fn handoff_package_round_trip_persists_schema_fields() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let session_id = uuid::Uuid::new_v4().to_string();
    let worktree = managed_worktree(
        &tempdir,
        &session_id,
        "handoff-round-trip",
        "agent/handoff-round-trip",
    );
    let target_file = worktree.join("src/lib.rs");
    std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
    std::fs::write(&target_file, "fixture").unwrap();
    config
        .check_in_worktree(crate::SessionWorktreeCheckInRequest {
            session_id: session_id.clone(),
            owner_id: "copilot".to_string(),
            ticket_id: "ticket-handoff-round-trip".to_string(),
            worktree_path: worktree,
            branch: "agent/handoff-round-trip".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();
    let init = config
        .init_runtime_context(crate::SessionRuntimeInitRequest {
            session_id: Some(session_id),
            ..Default::default()
        })
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
        target_files: vec!["src/lib.rs".to_string()],
        decisions: vec!["Use Option<SessionHandoffPackage> for backward compat".to_string()],
        non_goals: vec!["UI/viewer representation".to_string()],
        context_anchors: vec!["src/lib.rs".to_string()],
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
    assert_eq!(
        result.record.predecessor_handoff,
        package.predecessor_handoff
    );
    assert!(result.render.contains("implementation_ready: true"));
    assert!(result.render.contains("objective:"));

    // Verify the record is persisted and can be re-read from disk.
    let paths = config.runtime_paths_for_workspace(&workspace_id).unwrap();
    // Handoffs are now stored as folders with handoff.json inside
    let handoff_folder = paths.handoffs_dir.join(&result.record.handoff_id);
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
pub(super) fn handoff_package_without_worktree_uses_owning_store_workspace_for_all_layouts() {
    let tempdir = TempDir::new().unwrap();
    let cases = [
        (
            "canonical",
            tempdir.path().join(".workflow-tools/session"),
            tempdir.path().to_path_buf(),
        ),
        (
            "legacy",
            tempdir.path().join(".session"),
            tempdir.path().to_path_buf(),
        ),
        (
            "relocated",
            tempdir.path().join("relocated-session-store"),
            tempdir.path().join("relocated-session-store"),
        ),
    ];

    for (layout, store_root, validation_root) in cases {
        let target_file = validation_root.join("handoff-fixtures/owned-root.txt");
        std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
        std::fs::write(&target_file, layout).unwrap();

        let config = SessionStoreConfig::new(store_root);
        let session_id = uuid::Uuid::new_v4().to_string();
        config
            .init_runtime_context(crate::SessionRuntimeInitRequest {
                session_id: Some(session_id.clone()),
                ..Default::default()
            })
            .unwrap();
        let package = crate::SessionHandoffPackage {
            objective: format!("Validate the {layout} store layout"),
            target_tickets: vec![],
            higher_level_objective: String::new(),
            upward_context: vec![],
            target_files: vec!["handoff-fixtures/owned-root.txt".to_string()],
            decisions: vec![],
            non_goals: vec![],
            context_anchors: vec![],
            open_escalations: vec!["Test package is intentionally incomplete".to_string()],
            risk_notes: None,
            predecessor_handoff: None,
        };

        config
            .create_handoff_record(&session_id, Some(package), vec![], None)
            .unwrap_or_else(|error| {
                panic!("{layout} store must validate against its own root: {error}")
            });
    }
}

#[test]
pub(super) fn legacy_inline_handoff_package_still_deserializes() {
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

    let legacy: LegacyInlineHandoffPackage =
        serde_json::from_str(legacy_json).expect("deserialize legacy inline handoff package");

    assert_eq!(legacy.objective, "Fix the serialization regression");
    assert_eq!(legacy.target_tickets, vec!["ticket-123"]);
    assert_eq!(legacy.target_files, vec!["src/lib.rs"]);
    assert_eq!(legacy.decisions, vec!["Keep the inline package shape"]);
    assert_eq!(legacy.non_goals, vec!["No schema migration"]);
    assert_eq!(legacy.context_anchors, vec!["old session transcript"]);
    assert!(legacy.open_escalations.is_empty());
    assert_eq!(
        legacy.risk_notes.as_deref(),
        Some("Back-compat must remain intact")
    );
    assert_eq!(
        legacy.predecessor_handoff.as_deref(),
        Some("handoff-inline-0007")
    );
}

#[test]
pub(super) fn handoff_package_with_open_escalations_persists_but_not_ready() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(crate::SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    let package = crate::SessionHandoffPackage {
        objective: "Implement something".to_string(),
        open_escalations: vec!["Must resolve cross-crate dep question first".to_string()],
        ..Default::default()
    };

    let result = config
        .create_handoff_result(&workspace_id, Some(package), vec![], None)
        .expect("handoff with escalations should persist");

    assert!(!result.record.open_escalations.is_empty());
    assert!(result.render.contains("implementation_ready: false"));
    assert!(result.render.contains("open_escalations:"));
}

#[path = "snapshot_and_handoff/workflow_rendering.rs"]
mod workflow_rendering;
#[allow(unused_imports)]
pub use workflow_rendering::*;

#[path = "snapshot_and_handoff/target_files.rs"]
mod target_files;
#[allow(unused_imports)]
pub use target_files::*;
