#[allow(unused_imports)]
use super::*;

#[test]
pub(in super::super) fn check_in_rolls_back_successor_registry_failure() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let successor_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_B,
        "session-b",
        "session/session-b",
    );

    config.set_worktree_check_in_failure(Some(WorktreeCheckInFailurePoint::AfterSuccessorRegistry));
    assert!(
        config
            .check_in_worktree(sample_worktree_request(
                WORKTREE_SESSION_B,
                "github-copilot",
                "ticket-b",
                successor_path,
                "session/session-b",
            ))
            .is_err()
    );

    assert!(
        !tempdir
            .path()
            .join(".workflow-tools/session/local/worktrees")
            .join(format!("{WORKTREE_SESSION_B}.json"))
            .exists()
    );
    assert!(
        !tempdir
            .path()
            .join("store/sessions")
            .join(WORKTREE_SESSION_B)
            .join("session.json")
            .exists()
    );
}

#[test]
pub(in super::super) fn check_in_rolls_back_predecessor_update_failure() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let predecessor_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a",
        "session/session-a",
    );
    let successor_path = managed_worktree(
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
            predecessor_path,
            "session/session-a",
        ))
        .unwrap();

    let mut handoff = sample_worktree_request(
        WORKTREE_SESSION_B,
        "github-copilot-2",
        "ticket-a",
        successor_path,
        "session/session-b",
    );
    handoff.predecessor_session_id = Some(WORKTREE_SESSION_A.to_string());
    config.set_worktree_check_in_failure(Some(WorktreeCheckInFailurePoint::AfterPredecessorUpdate));
    assert!(config.check_in_worktree(handoff).is_err());

    assert_eq!(
        config.lookup_worktree(WORKTREE_SESSION_A).unwrap().status,
        SessionWorktreeStatus::Active
    );
    assert!(
        !tempdir
            .path()
            .join(".workflow-tools/session/local/worktrees")
            .join(format!("{WORKTREE_SESSION_B}.json"))
            .exists()
    );
    assert!(
        !tempdir
            .path()
            .join("store/sessions")
            .join(WORKTREE_SESSION_B)
            .join("session.json")
            .exists()
    );
}

#[test]
pub(in super::super) fn successful_rotation_supersedes_predecessor_once() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let predecessor_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a",
        "session/session-a",
    );
    let successor_path = managed_worktree(
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
            predecessor_path,
            "session/session-a",
        ))
        .unwrap();

    let mut handoff = sample_worktree_request(
        WORKTREE_SESSION_B,
        "github-copilot-2",
        "ticket-a",
        successor_path.clone(),
        "session/session-b",
    );
    handoff.predecessor_session_id = Some(WORKTREE_SESSION_A.to_string());
    config.check_in_worktree(handoff.clone()).unwrap();
    let predecessor_registry = tempdir
        .path()
        .join(".workflow-tools/session/local/worktrees")
        .join(format!("{WORKTREE_SESSION_A}.json"));
    let predecessor_after_rotation = std::fs::read(&predecessor_registry).unwrap();

    let receipt = config.check_in_worktree(handoff).unwrap();

    assert_eq!(
        receipt.allocation_mode,
        SessionWorktreeAllocationMode::Reused
    );
    assert_eq!(receipt.worktree_path, successor_path);
    assert_eq!(
        config.lookup_worktree(WORKTREE_SESSION_A).unwrap().status,
        SessionWorktreeStatus::Superseded
    );
    assert_eq!(
        std::fs::read(predecessor_registry).unwrap(),
        predecessor_after_rotation
    );
}
