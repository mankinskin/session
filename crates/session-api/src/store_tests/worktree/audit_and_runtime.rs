#[allow(unused_imports)]
use super::*;

pub(super) fn unassign_fixture(
    legacy: bool,
) -> (
    TempDir,
    SessionStoreConfig,
    crate::SessionWorktreeUnassignRequest,
) {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(if legacy {
        tempdir.path().join(".session")
    } else {
        tempdir.path().join(".workflow-tools/session")
    });
    let path = managed_worktree(&tempdir, WORKTREE_SESSION_A, "assigned", "session/assigned");
    config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "agent",
            "ticket",
            path.clone(),
            "session/assigned",
        ))
        .unwrap();
    let request = crate::SessionWorktreeUnassignRequest {
        session_id: WORKTREE_SESSION_A.to_string(),
        owner_id: "agent".to_string(),
        ticket_id: "ticket".to_string(),
        expected_worktree_path: path,
        expected_branch: "session/assigned".to_string(),
        reason: "Requester selected main".to_string(),
    };
    (tempdir, config, request)
}

#[test]
pub(super) fn check_in_worktree_rotates_when_existing_path_is_missing() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let first_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a",
        "session/session-a",
    );
    let second_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a-rotated",
        "session/session-a-rotated",
    );

    config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            first_path.clone(),
            "session/session-a",
        ))
        .unwrap();
    std::fs::remove_dir_all(&first_path).unwrap();

    let receipt = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            second_path.clone(),
            "session/session-a-rotated",
        ))
        .unwrap();

    assert_eq!(
        receipt.allocation_mode,
        SessionWorktreeAllocationMode::Rotated
    );
    assert_eq!(receipt.predecessor_session_id, None);
    assert_eq!(receipt.predecessor_path, Some(first_path));
    assert_eq!(receipt.worktree_path, second_path);
    assert!(receipt.worktree_path.exists());
}

#[test]
pub(super) fn cross_session_reuse_requires_adopt_flow() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let shared_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a",
        "session/session-a",
    );

    config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            shared_path.clone(),
            "session/session-a",
        ))
        .unwrap();

    let mut handoff = sample_worktree_request(
        WORKTREE_SESSION_B,
        "github-copilot-2",
        "ticket-a",
        shared_path.clone(),
        "session/session-a",
    );
    handoff.predecessor_session_id = Some(WORKTREE_SESSION_A.to_string());

    let error = config.check_in_worktree(handoff).unwrap_err();

    assert!(
        matches!(error, SessionError::CrossSessionReuseRequiresAdopt { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
pub(super) fn duplicate_active_canonical_path_without_predecessor_is_a_conflict() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let first_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a",
        "session/session-a",
    );
    let second_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_B,
        "session-b",
        "session/session-b",
    );
    config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            first_path,
            "session/session-a",
        ))
        .unwrap();

    let first_registry = tempdir
        .path()
        .join(".workflow-tools/session/local/worktrees")
        .join(format!("{WORKTREE_SESSION_A}.json"));
    let mut registry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&first_registry).unwrap()).unwrap();
    registry["assignment"]["path"] = serde_json::json!(second_path);
    std::fs::write(
        &first_registry,
        serde_json::to_vec_pretty(&registry).unwrap(),
    )
    .unwrap();

    let error = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_B,
            "github-copilot-2",
            "ticket-b",
            second_path,
            "session/session-b",
        ))
        .unwrap_err();

    assert!(matches!(error, SessionError::WorktreeConflict { .. }));
}

#[test]
pub(super) fn check_in_rejects_external_missing_and_branch_mismatched_worktrees() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let external = tempdir.path().join("external");
    git2::Repository::init(&external).unwrap();
    let external_error = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            external,
            "session/session-a",
        ))
        .unwrap_err();
    assert!(matches!(
        external_error,
        SessionError::InvalidManagedWorktree { .. }
    ));

    let missing_error = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            tempdir
                .path()
                .join(".worktrees")
                .join(WORKTREE_SESSION_A)
                .join("missing"),
            "session/session-a",
        ))
        .unwrap_err();
    assert!(matches!(
        missing_error,
        SessionError::InvalidManagedWorktree { .. }
    ));

    let worktree = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a",
        "session/session-a",
    );
    let branch_error = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            worktree,
            "session/wrong-branch",
        ))
        .unwrap_err();
    assert!(matches!(
        branch_error,
        SessionError::WorktreeBranchMismatch { .. }
    ));
}

#[test]
pub(super) fn check_in_rejects_symlink_path_escape() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let target = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a",
        "session/session-a",
    );
    let link = tempdir
        .path()
        .join(".worktrees")
        .join(WORKTREE_SESSION_B)
        .join("escaped");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    if let Err(error) = std::os::windows::fs::symlink_dir(&target, &link) {
        if error.kind() == std::io::ErrorKind::PermissionDenied
            || error.raw_os_error() == Some(1314)
        {
            return;
        }
        panic!("failed to create test symlink: {error}");
    }

    let error = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_B,
            "github-copilot",
            "ticket-b",
            link,
            "session/session-a",
        ))
        .unwrap_err();
    assert!(matches!(error, SessionError::InvalidManagedWorktree { .. }));
}

#[test]
pub(super) fn session_audit_supports_latest_and_explicit_session_selectors() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));

    let mut older = sample_payload(
        "session-old",
        Some("conversation-old"),
        sample_time(),
        &["first"],
    );
    older.events = vec![crate::CopilotHookEvent {
        event_id: Some("evt-old-1".to_string()),
        parent_event_id: None,
        event_type: Some("assistant.tool_plan".to_string()),
        captured_at: Some(sample_time()),
        turn_id: None,
        message_id: None,
        tool_call_id: None,
        tool_name: None,
        tool_success: None,
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: None,
        data_json: Some(serde_json::json!({})),
        raw_event_json: None,
    }];
    config
        .persist_capture(SessionCaptureRequest::copilot(older))
        .unwrap();

    let mut newer = sample_payload(
        "session-new",
        Some("conversation-new"),
        sample_time_later(),
        &["latest"],
    );
    newer.events = vec![crate::CopilotHookEvent {
        event_id: Some("evt-new-1".to_string()),
        parent_event_id: None,
        event_type: Some("tool.execution_result".to_string()),
        captured_at: Some(sample_time_later()),
        turn_id: None,
        message_id: None,
        tool_call_id: Some("call-1".to_string()),
        tool_name: Some("run_in_terminal".to_string()),
        tool_success: Some(true),
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: None,
        data_json: Some(serde_json::json!({
            "blocker": "sync-terminal-state-ambiguous"
        })),
        raw_event_json: None,
    }];
    config
        .persist_capture(SessionCaptureRequest::copilot(newer))
        .unwrap();

    let latest = config.session_audit(SessionAuditSelector::Latest).unwrap();
    let explicit = config
        .session_audit(SessionAuditSelector::SessionId("session-old".to_string()))
        .unwrap();

    assert_eq!(latest.session_id, "session-new");
    assert_eq!(latest.schema_version, SESSION_SCHEMA_VERSION);
    assert_eq!(latest.metrics.tool_execution_result_count, 1);
    assert_eq!(latest.metrics.ambiguous_sync_terminal_count, 1);
    assert_eq!(explicit.session_id, "session-old");
    assert_eq!(explicit.metrics.assistant_tool_plan_count, 1);
}

#[path = "audit_and_runtime/unassignment.rs"]
mod unassignment;
#[allow(unused_imports)]
pub use unassignment::*;

#[path = "audit_and_runtime/runtime_context.rs"]
mod runtime_context;
#[allow(unused_imports)]
pub use runtime_context::*;

#[path = "audit_and_runtime/rollback.rs"]
mod rollback;
#[allow(unused_imports)]
pub use rollback::*;
