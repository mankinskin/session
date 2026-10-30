#[allow(unused_imports)]
use super::*;

#[test]
pub(in super::super) fn unassign_deleted_checkout_clears_routing_and_preserves_audit() {
    for legacy in [false, true] {
        let (tempdir, config, request) = unassign_fixture(legacy);
        std::fs::remove_dir_all(&request.expected_worktree_path).unwrap();
        let before = config.read_session(WORKTREE_SESSION_A).unwrap();
        let receipt = config.unassign_worktree(request.clone()).unwrap();
        assert_eq!(
            receipt.main_checkout,
            tempdir.path().canonicalize().unwrap()
        );
        assert_eq!(
            receipt.previous_assignment.worktree_path,
            request.expected_worktree_path
        );
        assert_eq!(receipt.reason, request.reason);
        assert!(
            config
                .registered_worktree(WORKTREE_SESSION_A)
                .unwrap()
                .is_none()
        );
        let after = config.read_session(WORKTREE_SESSION_A).unwrap();
        assert!(after.metadata.worktree.is_none());
        assert_eq!(before.turns, after.turns);
        let audit: crate::SessionWorktreeUnassignReceipt =
            serde_json::from_slice(&std::fs::read(&receipt.audit_path).unwrap()).unwrap();
        assert_eq!(audit, receipt);
        assert!(matches!(
            config.unassign_worktree(request),
            Err(SessionError::MissingWorktreeAssignment { .. })
        ));
        assert!(receipt.audit_path.exists());
    }
}

#[test]
pub(in super::super) fn unassign_preserves_live_checkout_and_other_assignments() {
    let (tempdir, config, request) = unassign_fixture(false);
    let other_path = managed_worktree(&tempdir, WORKTREE_SESSION_B, "other", "session/other");
    config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_B,
            "other",
            "other-ticket",
            other_path,
            "session/other",
        ))
        .unwrap();
    let other_before = config.registered_worktree(WORKTREE_SESSION_B).unwrap();
    let content = request.expected_worktree_path.join("important.txt");
    std::fs::write(&content, "unchanged").unwrap();
    config.unassign_worktree(request.clone()).unwrap();
    assert_eq!(std::fs::read_to_string(content).unwrap(), "unchanged");
    assert!(request.expected_worktree_path.join(".git").exists());
    assert_eq!(
        config.registered_worktree(WORKTREE_SESSION_B).unwrap(),
        other_before
    );
    // A later explicit check-in is still supported; audit history survives.
    config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "agent",
            "ticket",
            request.expected_worktree_path,
            "session/assigned",
        ))
        .unwrap();
    assert!(
        config
            .registered_worktree(WORKTREE_SESSION_A)
            .unwrap()
            .is_some()
    );
}

#[test]
pub(in super::super) fn unassign_rejects_mismatched_owner_ticket_path_branch_and_empty_reason() {
    let (_tempdir, config, request) = unassign_fixture(false);
    let before = config.registered_worktree(WORKTREE_SESSION_A).unwrap();
    for field in ["owner", "ticket", "path", "branch", "reason"] {
        let mut bad = request.clone();
        match field {
            "owner" => bad.owner_id = "other".to_string(),
            "ticket" => bad.ticket_id = "other".to_string(),
            "path" => bad.expected_worktree_path.push("different"),
            "branch" => bad.expected_branch = "different".to_string(),
            _ => bad.reason = " ".to_string(),
        }
        let error = config.unassign_worktree(bad).unwrap_err();
        match field {
            "owner" | "ticket" => assert!(matches!(
                error,
                SessionError::SessionOwnershipMismatch { .. }
            )),
            "path" | "branch" => assert!(matches!(
                error,
                SessionError::WorktreeAssignmentChanged { .. }
            )),
            _ => assert!(matches!(error, SessionError::MissingWorktreeUnassignReason)),
        }
        assert_eq!(
            config.registered_worktree(WORKTREE_SESSION_A).unwrap(),
            before
        );
    }
}

#[test]
pub(in super::super) fn unassign_rolls_back_metadata_and_audit_on_failure() {
    let (_tempdir, config, request) = unassign_fixture(false);
    let manifest = config
        .paths_for_session_id(WORKTREE_SESSION_A)
        .unwrap()
        .manifest_path;
    let before = std::fs::read(&manifest).unwrap();
    config.set_worktree_check_in_failure(Some(WorktreeCheckInFailurePoint::AfterUnassignManifest));
    assert!(config.unassign_worktree(request.clone()).is_err());
    assert_eq!(std::fs::read(manifest).unwrap(), before);
    assert!(
        config
            .registered_worktree(WORKTREE_SESSION_A)
            .unwrap()
            .is_some()
    );
    let audits = config
        .root
        .join("local/worktree-unassignments")
        .join(WORKTREE_SESSION_A);
    assert_eq!(std::fs::read_dir(audits).unwrap().count(), 0);
    config.unassign_worktree(request).unwrap();
}

#[test]
pub(in super::super) fn unassign_refuses_concurrent_mutation_and_preserves_registry() {
    let (_tempdir, config, request) = unassign_fixture(false);
    let lock_path = config
        .root
        .join("sessions")
        .join(WORKTREE_SESSION_A)
        .join(".context.lock");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
        .unwrap();
    lock.try_lock().unwrap();
    assert!(matches!(
        config.unassign_worktree(request.clone()),
        Err(SessionError::RuntimeMutationConflict { .. })
    ));
    assert!(
        config
            .registered_worktree(WORKTREE_SESSION_A)
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        config.check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "agent",
            "ticket",
            request.expected_worktree_path.clone(),
            "session/assigned",
        )),
        Err(SessionError::RuntimeMutationConflict { .. })
    ));
    drop(lock);
    config.unassign_worktree(request).unwrap();
}

#[test]
pub(in super::super) fn unassign_handles_missing_manifest_and_inactive_assignment_but_rejects_bad_metadata()
 {
    let (_tempdir, config, request) = unassign_fixture(false);
    let manifest = config
        .paths_for_session_id(WORKTREE_SESSION_A)
        .unwrap()
        .manifest_path;
    let original = std::fs::read(&manifest).unwrap();
    for contents in [
        b"invalid".as_slice(),
        br#"{"metadata":false}"#.as_slice(),
        b"[]".as_slice(),
        b"{}".as_slice(),
    ] {
        std::fs::write(&manifest, contents).unwrap();
        assert!(config.unassign_worktree(request.clone()).is_err());
        assert!(
            config
                .registered_worktree(WORKTREE_SESSION_A)
                .unwrap()
                .is_some()
        );
        assert_eq!(std::fs::read(&manifest).unwrap(), contents);
    }
    std::fs::write(&manifest, original).unwrap();
    std::fs::remove_file(manifest).unwrap();
    let registry = config
        .root
        .join("local/worktrees")
        .join(format!("{WORKTREE_SESSION_A}.json"));
    let mut entry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&registry).unwrap()).unwrap();
    entry["assignment"]["status"] = serde_json::json!("superseded");
    std::fs::write(registry, serde_json::to_vec(&entry).unwrap()).unwrap();
    let receipt = config.unassign_worktree(request).unwrap();
    assert_eq!(
        receipt.previous_assignment.status,
        SessionWorktreeStatus::Superseded
    );
    assert!(
        config
            .registered_worktree(WORKTREE_SESSION_A)
            .unwrap()
            .is_none()
    );
}

#[test]
pub(in super::super) fn unassign_rejects_checkout_store_and_corrupt_registry_without_writing() {
    let (_tempdir, config, request) = unassign_fixture(false);
    let checkout_store = SessionStoreConfig::new(
        request
            .expected_worktree_path
            .join(".workflow-tools/session"),
    );
    std::fs::create_dir_all(&checkout_store.root).unwrap();
    assert!(matches!(
        checkout_store.unassign_worktree(request.clone()),
        Err(SessionError::InvalidManagedWorktree { .. })
    ));
    let registry = config
        .root
        .join("local/worktrees")
        .join(format!("{WORKTREE_SESSION_A}.json"));
    std::fs::write(&registry, "broken").unwrap();
    assert!(matches!(
        config.unassign_worktree(request.clone()),
        Err(SessionError::Deserialize { .. })
    ));
    assert!(matches!(
        config.check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "agent",
            "ticket",
            request.expected_worktree_path,
            "session/assigned",
        )),
        Err(SessionError::Deserialize { .. })
    ));
    assert_eq!(std::fs::read_to_string(registry).unwrap(), "broken");
}
