#[allow(unused_imports)]
use super::*;

#[test]
pub(in super::super) fn handoff_package_with_nonexistent_target_file_fails_at_creation_time() {
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
    let result = config.create_handoff_record(&workspace_id, Some(package), vec![], None);
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
pub(in super::super) fn handoff_package_validates_target_files_in_assigned_worktree() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
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
pub(in super::super) fn handoff_package_normalizes_backslash_target_files_to_forward_slash() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let session_id = uuid::Uuid::new_v4().to_string();
    let worktree = managed_worktree(
        &tempdir,
        &session_id,
        "handoff-normalized-path",
        "agent/handoff-normalized-path",
    );
    let target_file = worktree.join("src/lib.rs");
    std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
    std::fs::write(&target_file, "fixture").unwrap();
    config
        .check_in_worktree(crate::SessionWorktreeCheckInRequest {
            session_id: session_id.clone(),
            owner_id: "copilot".to_string(),
            ticket_id: "ticket-handoff-normalized-path".to_string(),
            worktree_path: worktree,
            branch: "agent/handoff-normalized-path".to_string(),
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
        objective: "Verify repo-root-relative forward-slash normalization".to_string(),
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
        target_files: vec!["src\\lib.rs".to_string()],
        decisions: vec!["n/a".to_string()],
        non_goals: vec!["n/a".to_string()],
        // Store-qualified nested-store anchor (AC1: verified-to-exist path).
        context_anchors: vec!["src/lib.rs".to_string()],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let record = config
        .create_handoff_record(&workspace_id, Some(package), vec![], None)
        .expect("valid repo-root-relative paths must pass creation-time validation");

    assert_eq!(
        record.target_files,
        vec!["src/lib.rs".to_string()],
        "target_files must be normalized to forward-slash form"
    );
    assert!(
        !record.target_files[0].contains('\\'),
        "no backslashes should remain in a persisted target_files entry"
    );
}
