fn unassign_fixture(legacy: bool) -> (TempDir, SessionStoreConfig, crate::SessionWorktreeUnassignRequest) {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(if legacy {
        tempdir.path().join(".session")
    } else {
        tempdir.path().join(".workflow-tools/session")
    });
    let path = managed_worktree(&tempdir, WORKTREE_SESSION_A, "assigned", "session/assigned");
    config.check_in_worktree(sample_worktree_request(
        WORKTREE_SESSION_A, "agent", "ticket", path.clone(), "session/assigned",
    )).unwrap();
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
fn unassign_deleted_checkout_clears_routing_and_preserves_audit() {
    for legacy in [false, true] {
        let (tempdir, config, request) = unassign_fixture(legacy);
        std::fs::remove_dir_all(&request.expected_worktree_path).unwrap();
        let before = config.read_session(WORKTREE_SESSION_A).unwrap();
        let receipt = config.unassign_worktree(request.clone()).unwrap();
        assert_eq!(receipt.main_checkout, tempdir.path().canonicalize().unwrap());
        assert_eq!(receipt.previous_assignment.worktree_path, request.expected_worktree_path);
        assert_eq!(receipt.reason, request.reason);
        assert!(config.registered_worktree(WORKTREE_SESSION_A).unwrap().is_none());
        let after = config.read_session(WORKTREE_SESSION_A).unwrap();
        assert!(after.metadata.worktree.is_none());
        assert_eq!(before.turns, after.turns);
        let audit: crate::SessionWorktreeUnassignReceipt =
            serde_json::from_slice(&std::fs::read(&receipt.audit_path).unwrap()).unwrap();
        assert_eq!(audit, receipt);
        assert!(matches!(config.unassign_worktree(request), Err(SessionError::MissingWorktreeAssignment { .. })));
        assert!(receipt.audit_path.exists());
    }
}

#[test]
fn unassign_preserves_live_checkout_and_other_assignments() {
    let (tempdir, config, request) = unassign_fixture(false);
    let other_path = managed_worktree(&tempdir, WORKTREE_SESSION_B, "other", "session/other");
    config.check_in_worktree(sample_worktree_request(
        WORKTREE_SESSION_B, "other", "other-ticket", other_path, "session/other",
    )).unwrap();
    let other_before = config.registered_worktree(WORKTREE_SESSION_B).unwrap();
    let content = request.expected_worktree_path.join("important.txt");
    std::fs::write(&content, "unchanged").unwrap();
    config.unassign_worktree(request.clone()).unwrap();
    assert_eq!(std::fs::read_to_string(content).unwrap(), "unchanged");
    assert!(request.expected_worktree_path.join(".git").exists());
    assert_eq!(config.registered_worktree(WORKTREE_SESSION_B).unwrap(), other_before);
    // A later explicit check-in is still supported; audit history survives.
    config.check_in_worktree(sample_worktree_request(
        WORKTREE_SESSION_A, "agent", "ticket", request.expected_worktree_path, "session/assigned",
    )).unwrap();
    assert!(config.registered_worktree(WORKTREE_SESSION_A).unwrap().is_some());
}

#[test]
fn unassign_rejects_mismatched_owner_ticket_path_branch_and_empty_reason() {
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
            "owner" | "ticket" => assert!(matches!(error, SessionError::SessionOwnershipMismatch { .. })),
            "path" | "branch" => assert!(matches!(error, SessionError::WorktreeAssignmentChanged { .. })),
            _ => assert!(matches!(error, SessionError::MissingWorktreeUnassignReason)),
        }
        assert_eq!(config.registered_worktree(WORKTREE_SESSION_A).unwrap(), before);
    }
}

#[test]
fn unassign_rolls_back_metadata_and_audit_on_failure() {
    let (_tempdir, config, request) = unassign_fixture(false);
    let manifest = config.paths_for_session_id(WORKTREE_SESSION_A).unwrap().manifest_path;
    let before = std::fs::read(&manifest).unwrap();
    config.set_worktree_check_in_failure(Some(WorktreeCheckInFailurePoint::AfterUnassignManifest));
    assert!(config.unassign_worktree(request.clone()).is_err());
    assert_eq!(std::fs::read(manifest).unwrap(), before);
    assert!(config.registered_worktree(WORKTREE_SESSION_A).unwrap().is_some());
    let audits = config.root.join("local/worktree-unassignments").join(WORKTREE_SESSION_A);
    assert_eq!(std::fs::read_dir(audits).unwrap().count(), 0);
    config.unassign_worktree(request).unwrap();
}

#[test]
fn unassign_refuses_concurrent_mutation_and_preserves_registry() {
    let (_tempdir, config, request) = unassign_fixture(false);
    let lock_path = config.root.join("sessions").join(WORKTREE_SESSION_A).join(".context.lock");
    let lock = std::fs::OpenOptions::new().read(true).write(true).open(lock_path).unwrap();
    lock.try_lock().unwrap();
    assert!(matches!(config.unassign_worktree(request.clone()), Err(SessionError::RuntimeMutationConflict { .. })));
    assert!(config.registered_worktree(WORKTREE_SESSION_A).unwrap().is_some());
    assert!(matches!(config.check_in_worktree(sample_worktree_request(
        WORKTREE_SESSION_A, "agent", "ticket", request.expected_worktree_path.clone(), "session/assigned",
    )), Err(SessionError::RuntimeMutationConflict { .. })));
    drop(lock);
    config.unassign_worktree(request).unwrap();
}

#[test]
fn unassign_handles_missing_manifest_and_inactive_assignment_but_rejects_bad_metadata() {
    let (_tempdir, config, request) = unassign_fixture(false);
    let manifest = config.paths_for_session_id(WORKTREE_SESSION_A).unwrap().manifest_path;
    let original = std::fs::read(&manifest).unwrap();
    for contents in [
        b"invalid".as_slice(),
        br#"{"metadata":false}"#.as_slice(),
        b"[]".as_slice(),
        b"{}".as_slice(),
    ] {
        std::fs::write(&manifest, contents).unwrap();
        assert!(config.unassign_worktree(request.clone()).is_err());
        assert!(config.registered_worktree(WORKTREE_SESSION_A).unwrap().is_some());
        assert_eq!(std::fs::read(&manifest).unwrap(), contents);
    }
    std::fs::write(&manifest, original).unwrap();
    std::fs::remove_file(manifest).unwrap();
    let registry = config.root.join("local/worktrees").join(format!("{WORKTREE_SESSION_A}.json"));
    let mut entry: serde_json::Value = serde_json::from_slice(&std::fs::read(&registry).unwrap()).unwrap();
    entry["assignment"]["status"] = serde_json::json!("superseded");
    std::fs::write(registry, serde_json::to_vec(&entry).unwrap()).unwrap();
    let receipt = config.unassign_worktree(request).unwrap();
    assert_eq!(receipt.previous_assignment.status, SessionWorktreeStatus::Superseded);
    assert!(config.registered_worktree(WORKTREE_SESSION_A).unwrap().is_none());
}

#[test]
fn unassign_rejects_checkout_store_and_corrupt_registry_without_writing() {
    let (_tempdir, config, request) = unassign_fixture(false);
    let checkout_store = SessionStoreConfig::new(request.expected_worktree_path.join(".workflow-tools/session"));
    std::fs::create_dir_all(&checkout_store.root).unwrap();
    assert!(matches!(checkout_store.unassign_worktree(request.clone()), Err(SessionError::InvalidManagedWorktree { .. })));
    let registry = config.root.join("local/worktrees").join(format!("{WORKTREE_SESSION_A}.json"));
    std::fs::write(&registry, "broken").unwrap();
    assert!(matches!(config.unassign_worktree(request.clone()), Err(SessionError::Deserialize { .. })));
    assert!(matches!(config.check_in_worktree(sample_worktree_request(
        WORKTREE_SESSION_A, "agent", "ticket", request.expected_worktree_path, "session/assigned",
    )), Err(SessionError::Deserialize { .. })));
    assert_eq!(std::fs::read_to_string(registry).unwrap(), "broken");
}

#[test]
fn check_in_worktree_rotates_when_existing_path_is_missing() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
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
fn cross_session_reuse_requires_adopt_flow() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
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

    assert!(matches!(
        error,
        SessionError::CrossSessionReuseRequiresAdopt { .. }
    ), "unexpected error: {error:?}");
}

#[test]
fn check_in_rolls_back_successor_registry_failure() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
    let successor_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_B,
        "session-b",
        "session/session-b",
    );

    config.set_worktree_check_in_failure(Some(
        WorktreeCheckInFailurePoint::AfterSuccessorRegistry,
    ));
    assert!(config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_B,
            "github-copilot",
            "ticket-b",
            successor_path,
            "session/session-b",
        ))
        .is_err());

    assert!(!tempdir
        .path()
        .join(".workflow-tools/session/local/worktrees")
        .join(format!("{WORKTREE_SESSION_B}.json"))
        .exists());
    assert!(!tempdir
        .path()
        .join("store/sessions")
        .join(WORKTREE_SESSION_B)
        .join("session.json")
        .exists());
}

#[test]
fn check_in_rolls_back_predecessor_update_failure() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
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
    config.set_worktree_check_in_failure(Some(
        WorktreeCheckInFailurePoint::AfterPredecessorUpdate,
    ));
    assert!(config.check_in_worktree(handoff).is_err());

    assert_eq!(
        config.lookup_worktree(WORKTREE_SESSION_A).unwrap().status,
        SessionWorktreeStatus::Active
    );
    assert!(!tempdir
        .path()
        .join(".workflow-tools/session/local/worktrees")
        .join(format!("{WORKTREE_SESSION_B}.json"))
        .exists());
    assert!(!tempdir
        .path()
        .join("store/sessions")
        .join(WORKTREE_SESSION_B)
        .join("session.json")
        .exists());
}

#[test]
fn successful_rotation_supersedes_predecessor_once() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
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

    assert_eq!(receipt.allocation_mode, SessionWorktreeAllocationMode::Reused);
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

#[test]
fn duplicate_active_canonical_path_without_predecessor_is_a_conflict() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
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
    let mut registry: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&first_registry).unwrap(),
    )
    .unwrap();
    registry["assignment"]["path"] = serde_json::json!(second_path);
    std::fs::write(&first_registry, serde_json::to_vec_pretty(&registry).unwrap())
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
fn check_in_rejects_external_missing_and_branch_mismatched_worktrees() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
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
    assert!(matches!(external_error, SessionError::InvalidManagedWorktree { .. }));

    let missing_error = config
        .check_in_worktree(sample_worktree_request(
            WORKTREE_SESSION_A,
            "github-copilot",
            "ticket-a",
            tempdir.path().join(".worktrees").join(WORKTREE_SESSION_A).join("missing"),
            "session/session-a",
        ))
        .unwrap_err();
    assert!(matches!(missing_error, SessionError::InvalidManagedWorktree { .. }));

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
    assert!(matches!(branch_error, SessionError::WorktreeBranchMismatch { .. }));
}

#[test]
fn check_in_rejects_symlink_path_escape() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
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
fn read_session_rejects_unknown_schema_version() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));

    let plan = config
        .persist_capture(sample_request(
            "session-schema",
            Some("conversation-schema"),
            sample_time(),
            &["check schema"],
        ))
        .unwrap();

    let mut manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&plan.paths.manifest_path).unwrap(),
    )
    .unwrap();
    manifest["schema_version"] = serde_json::json!(SESSION_SCHEMA_VERSION + 1);
    std::fs::write(
        &plan.paths.manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let err = config.read_session("session-schema").unwrap_err();
    assert!(matches!(err, SessionError::SchemaVersionMismatch { .. }));
}

#[test]
fn runtime_init_uses_the_provisioned_worktree_uuid() {
    let current_dir = std::env::current_dir().unwrap();
    let tempdir = TempDir::new_in(&current_dir).unwrap();
    let session_id = Uuid::new_v4().to_string();
    let store_root = tempdir
        .path()
        .join(".worktrees")
        .join(&session_id)
        .join("workspace-policy-refactor")
        .join(".session");
    std::fs::create_dir_all(&store_root).unwrap();
    let config = SessionStoreConfig::new(
        store_root.strip_prefix(&current_dir).unwrap(),
    );

    let result = config
        .init_runtime_context(SessionRuntimeInitRequest::default())
        .unwrap();

    assert_eq!(result.context.session_id, session_id);
    assert!(
        store_root
            .join("sessions")
            .join(&result.context.session_id)
            .join("session.json")
            .is_file()
    );
}

#[test]
fn worktree_identity_rejects_slug_values() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
    let session_id = "epic-kickoff-8fdfe135";

    let error = config
        .check_in_worktree(sample_worktree_request(
            session_id,
            "github-copilot",
            "ticket-a",
            tempdir.path().join("worktree"),
            "agent/example",
        ))
        .unwrap_err();

    assert!(matches!(
        error,
        SessionError::InvalidSessionId(ref value) if value == session_id
    ));
    assert!(error.to_string().contains("must be a UUID"));
}

#[test]
fn legacy_slug_keyed_session_record_remains_readable() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
    let session_id = "structured-ticket-entities-iteration";

    config
        .persist_capture(sample_request(
            session_id,
            Some("legacy-conversation"),
            sample_time(),
            &["legacy session"],
        ))
        .unwrap();

    assert_eq!(
        config.read_session(session_id).unwrap().session_id,
        session_id
    );
}

#[test]
fn session_audit_supports_latest_and_explicit_session_selectors() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));

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
        .session_audit(SessionAuditSelector::SessionId(
            "session-old".to_string(),
        ))
        .unwrap();

    assert_eq!(latest.session_id, "session-new");
    assert_eq!(latest.schema_version, SESSION_SCHEMA_VERSION);
    assert_eq!(latest.metrics.tool_execution_result_count, 1);
    assert_eq!(latest.metrics.ambiguous_sync_terminal_count, 1);
    assert_eq!(explicit.session_id, "session-old");
    assert_eq!(explicit.metrics.assistant_tool_plan_count, 1);
}

#[test]
fn context_schema_init_is_idempotent_without_forcing_a_new_run() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));

    let first = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let second = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(first.context.session_id.clone()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap();

    assert!(first.created_workspace);
    assert!(first.created_run);
    assert!(!second.created_workspace);
    assert!(!second.created_run);
    assert_eq!(first.context.session_id, second.context.session_id);
    assert_eq!(first.context.active_run_id, second.context.active_run_id);
    assert_eq!(second.context.runs.len(), 1);
}

#[test]
fn run_lineage_init_resume_creates_distinct_linked_run() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));

    let first = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let resumed = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(first.context.session_id.clone()),
            predecessor_run_id: Some(first.context.active_run_id.clone()),
            force_new_run: true,
        })
        .unwrap();

    assert_eq!(first.context.session_id, resumed.context.session_id);
    assert_ne!(first.context.active_run_id, resumed.context.active_run_id);
    assert_eq!(resumed.context.runs.len(), 2);
    assert_eq!(
        resumed.run.predecessor_run_id.as_deref(),
        Some(first.context.active_run_id.as_str())
    );
}

#[test]
fn context_pin_unpin_is_idempotent_and_persistent() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;
    let urn = "ce://default/tickets/effba966-f0a8-4d7d-b289-b7feba826cf8";

    let pinned_once = config
        .pin_runtime_entity(
            &workspace_id,
            urn,
            Some("primary-focus".to_string()),
            Some("epic context".to_string()),
        )
        .unwrap();
    let pinned_twice = config
        .pin_runtime_entity(&workspace_id, urn, None, None)
        .unwrap();

    assert_eq!(pinned_once.pinned_entities.len(), 1);
    assert_eq!(pinned_twice.pinned_entities.len(), 1);

    let loaded = config.read_runtime_context(&workspace_id).unwrap();
    assert_eq!(loaded.pinned_entities.len(), 1);

    let unpinned_once =
        config.unpin_runtime_entity(&workspace_id, urn).unwrap();
    let unpinned_twice =
        config.unpin_runtime_entity(&workspace_id, urn).unwrap();
    assert!(unpinned_once.pinned_entities.is_empty());
    assert!(unpinned_twice.pinned_entities.is_empty());
}

#[test]
fn context_pin_rejects_malformed_entity_urn_segments() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();

    let error = config
        .pin_runtime_entity(
            &init.context.session_id,
            "ce:///tickets/",
            None,
            None,
        )
        .unwrap_err();

    assert!(matches!(error, SessionError::InvalidEntityUrn(_)));
}

#[test]
fn context_view_returns_headers_only() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    config
        .pin_runtime_entity(
            &workspace_id,
            "ce://default/specs/709f067a-21b6-41b6-8879-3cacef4bacaf",
            Some("guard".to_string()),
            Some("runtime contract".to_string()),
        )
        .unwrap();

    let view = config.view_runtime_context(&workspace_id).unwrap();
    let json = serde_json::to_string(&view).unwrap();

    assert_eq!(view.pinned_count, 1);
    assert!(json.contains("pinned_headers"));
    assert!(!json.contains("body"));
    assert!(!json.contains("content"));
}
