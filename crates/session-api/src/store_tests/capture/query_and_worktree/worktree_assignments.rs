#[allow(unused_imports)]
use super::*;

#[test]
pub(in super::super) fn check_in_worktree_creates_and_returns_new_assignment() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let worktree_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a",
        "session/session-a",
    );

    let receipt = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            worktree_path.clone(),
            "session/session-a",
        ))
        .unwrap();

    assert_eq!(receipt.session_id, WORKTREE_SESSION_A);
    assert_eq!(receipt.owner_id, "github-copilot");
    assert_eq!(receipt.ticket_id, "ticket-a");
    assert_eq!(receipt.worktree_path, worktree_path);
    assert_eq!(receipt.branch, "session/session-a");
    assert_eq!(receipt.allocation_mode, SessionWorktreeAllocationMode::New);
    assert_eq!(receipt.status, SessionWorktreeStatus::Active);
    assert!(receipt.worktree_path.exists());
}

#[test]
pub(in super::super) fn check_in_worktree_reuses_existing_assignment_for_same_session() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let worktree_path = managed_worktree(
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
            worktree_path.clone(),
            "session/session-a",
        ))
        .unwrap();

    let receipt = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            worktree_path.clone(),
            "session/session-a",
        ))
        .unwrap();

    assert_eq!(
        receipt.allocation_mode,
        SessionWorktreeAllocationMode::Reused
    );
    assert_eq!(receipt.worktree_path, worktree_path);

    let lookup = config.lookup_worktree(WORKTREE_SESSION_A).unwrap();
    assert_eq!(
        lookup.allocation_mode,
        SessionWorktreeAllocationMode::Reused
    );
    assert_eq!(lookup.status, SessionWorktreeStatus::Active);
}

#[test]
pub(in super::super) fn check_in_worktree_claims_unclaimed_hook_record() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let worktree_path =
        managed_worktree(&tempdir, WORKTREE_SESSION_A, "session-a", "agent/initial");

    config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "copilot-agent",
            "initial-ticket",
            worktree_path.clone(),
            "agent/initial",
        ))
        .unwrap();
    let manifest_path = tempdir
        .path()
        .join("store/sessions")
        .join(WORKTREE_SESSION_A)
        .join("session.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["metadata"]["agent_id"] = serde_json::json!("copilot-agent");
    manifest["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("ticket_id");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let receipt = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "copilot-agent",
            "initial-ticket",
            worktree_path,
            "agent/initial",
        ))
        .unwrap();
    let record = config.read_session(WORKTREE_SESSION_A).unwrap();

    assert_eq!(receipt.owner_id, "copilot-agent");
    assert_eq!(receipt.ticket_id, "initial-ticket");
    assert_eq!(record.metadata.agent_id, None);
    assert_eq!(record.metadata.ticket_id, None);
}

#[test]
pub(in super::super) fn check_in_worktree_rejects_mismatched_claimed_owner() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let worktree_path =
        managed_worktree(&tempdir, WORKTREE_SESSION_A, "session-a", "agent/claimed");

    config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "claimed-owner",
            "claimed-ticket",
            worktree_path.clone(),
            "agent/claimed",
        ))
        .unwrap();

    let error = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "other-owner",
            "claimed-ticket",
            worktree_path,
            "agent/claimed",
        ))
        .unwrap_err();

    assert!(matches!(
        error,
        SessionError::SessionOwnershipMismatch { .. }
    ));
}

#[test]
pub(in super::super) fn check_in_worktree_rotates_for_handoff_and_supersedes_predecessor() {
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
            first_path.clone(),
            "session/session-a",
        ))
        .unwrap();

    let mut handoff = sample_worktree_request(
        WORKTREE_SESSION_B,
        "github-copilot-2",
        "ticket-a",
        second_path.clone(),
        "session/session-b",
    );
    handoff.predecessor_session_id = Some(WORKTREE_SESSION_A.to_string());

    let receipt = config.check_in_worktree(handoff).unwrap();
    let predecessor = config.lookup_worktree(WORKTREE_SESSION_A).unwrap();

    assert_eq!(
        receipt.allocation_mode,
        SessionWorktreeAllocationMode::Rotated
    );
    assert_eq!(
        receipt.predecessor_session_id.as_deref(),
        Some(WORKTREE_SESSION_A)
    );
    assert_eq!(receipt.predecessor_path, Some(first_path));
    assert_eq!(predecessor.status, SessionWorktreeStatus::Superseded);
}

#[test]
pub(in super::super) fn lookup_reads_branch_manifest_without_a_transcript() {
    let tempdir = TempDir::new().unwrap();
    let main_checkout = tempdir.path();
    let worktree = managed_worktree(&tempdir, WORKTREE_SESSION_A, "session", "agent/session-a");
    let config = SessionStoreConfig::new(main_checkout.join(".session"));

    config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            worktree,
            "agent/session-a",
        ))
        .unwrap();

    std::fs::remove_file(
        main_checkout
            .join(".session")
            .join("sessions")
            .join(WORKTREE_SESSION_A)
            .join("transcript.json"),
    )
    .unwrap();

    assert_eq!(
        config.lookup_worktree(WORKTREE_SESSION_A).unwrap().branch,
        "agent/session-a"
    );
    let range = config.peek_range(WORKTREE_SESSION_A, 0, None).unwrap();
    assert_eq!(range.total_turns, 0);
    assert!(range.turns.is_empty());
}

#[test]
pub(in super::super) fn check_in_writes_untracked_main_registry_and_branch_only_manifests() {
    let tempdir = TempDir::new().unwrap();
    let main_checkout = tempdir.path();
    let worktree = managed_worktree(&tempdir, WORKTREE_SESSION_A, "session", "agent/session-a");
    let worktree_config = SessionStoreConfig::new(worktree.join(".session"));
    let main_config = SessionStoreConfig::new(main_checkout.join(".session"));

    worktree_config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            worktree.clone(),
            "agent/session-a",
        ))
        .unwrap();

    let registry = main_checkout
        .join(".session/local/worktrees")
        .join(format!("{WORKTREE_SESSION_A}.json"));
    assert!(registry.exists());
    let worktree_manifest = worktree
        .join(".session/sessions")
        .join(WORKTREE_SESSION_A)
        .join("session.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(worktree_manifest).unwrap()).unwrap();
    assert_eq!(
        manifest["metadata"]["worktree"],
        serde_json::json!({ "branch": "agent/session-a" })
    );
    assert!(manifest["metadata"].get("agent_id").is_none());
    assert!(manifest["metadata"].get("ticket_id").is_none());
    assert_eq!(
        main_config
            .lookup_worktree(WORKTREE_SESSION_A)
            .unwrap()
            .worktree_path,
        worktree
    );
}

#[test]
pub(in super::super) fn lookup_rejects_registry_entry_for_missing_worktree() {
    let tempdir = TempDir::new().unwrap();
    let main_checkout = tempdir.path();
    let worktree = managed_worktree(&tempdir, WORKTREE_SESSION_A, "session", "agent/session-a");
    let config = SessionStoreConfig::new(main_checkout.join(".session"));
    config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            worktree.clone(),
            "agent/session-a",
        ))
        .unwrap();
    std::fs::remove_dir_all(worktree).unwrap();

    assert!(matches!(
        config.lookup_worktree(WORKTREE_SESSION_A),
        Err(SessionError::RegisteredWorktreeMissing { .. })
    ));
}
