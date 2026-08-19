
/// High: finished workspaces are immutable — every workflow/pin mutation is
/// rejected after finish.
#[test]
fn finished_workspace_rejects_all_mutations() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root, "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("seed-node".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "seed".to_string(),
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
        .workflow_update_node_status(
            &workspace_id,
            "seed-node",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();

    let finished = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap();
    assert!(!finished.already_finished);

    // Adding a node after finish is rejected.
    let add_err = config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("post-finish-node".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Optional,
                title: "should be rejected".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap_err();
    assert!(matches!(add_err, SessionError::WorkspaceFinished { .. }));

    // Updating a node status after finish is rejected.
    let status_err = config
        .workflow_update_node_status(
            &workspace_id,
            "seed-node",
            SessionWorkflowNodeStatus::InProgress,
            None,
        )
        .unwrap_err();
    assert!(matches!(status_err, SessionError::WorkspaceFinished { .. }));

    // Pinning after finish is rejected.
    let pin_err = config
        .pin_runtime_entity(
            &workspace_id,
            "ce://context-engine/tickets/55555555-5555-4555-8555-555555555555",
            None,
            None,
        )
        .unwrap_err();
    assert!(matches!(pin_err, SessionError::WorkspaceFinished { .. }));
}

/// A live lock cannot be stolen solely because its metadata is older than the
/// former 30-second stale threshold, and releasing it preserves the stable lock
/// file used by successor owners.
#[test]
fn aged_live_lock_blocks_second_owner_and_releases_safely() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root, "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    let paths = config.runtime_paths_for_workspace(&workspace_id).unwrap();
    let lock_path = paths.workspace_dir.join(".context.lock");

    let formerly_stale =
        (chrono::Utc::now() - chrono::Duration::seconds(31)).to_rfc3339();
    std::fs::write(&lock_path, formerly_stale).unwrap();
    let first_owner = config.acquire_runtime_lock(&workspace_id).unwrap();

    let conflict = match config.acquire_runtime_lock(&workspace_id) {
        Ok(_) => panic!("a second owner acquired the aged live lock"),
        Err(error) => error,
    };
    assert!(matches!(
        conflict,
        SessionError::RuntimeMutationConflict { .. }
    ));

    drop(first_owner);
    let successor = config.acquire_runtime_lock(&workspace_id).unwrap();
    assert!(lock_path.exists());
    drop(successor);
    assert!(lock_path.exists());

    let final_owner = config.acquire_runtime_lock(&workspace_id).unwrap();
    drop(final_owner);
}

#[cfg(windows)]
#[test]
fn failed_windows_replacement_preserves_previous_bytes() {
    use std::os::windows::fs::OpenOptionsExt;

    let tempdir = TempDir::new().unwrap();
    let path = tempdir.path().join("durable.json");
    super::write_json(&path, &serde_json::json!({ "version": "old" })).unwrap();
    let previous = std::fs::read(&path).unwrap();

    let destination = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0x0000_0001 | 0x0000_0002)
        .open(&path)
        .unwrap();

    let error = super::write_json(
        &path,
        &serde_json::json!({ "version": "replacement" }),
    )
    .unwrap_err();
    assert!(matches!(error, SessionError::Io { .. }));
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    drop(destination);
}

#[test]
fn finish_excludes_mutation_init_and_resume_until_terminal_commit() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;
    let predecessor_run_id = init.run.run_id;
    let ticket_urn =
        "ce://context-engine/tickets/66666666-6666-4666-8666-666666666666";

    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("terminal-ticket".to_string()),
                kind: SessionWorkflowNodeKind::Ticket,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "terminal ticket".to_string(),
                ticket_urn: Some(ticket_urn.to_string()),
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let finish_config = config.clone();
    let finish_workspace_id = workspace_id.clone();
    let finish_thread = std::thread::spawn(move || {
        let resolver = BlockingTerminalResolver {
            urn: ticket_urn.to_string(),
            entered: entered_tx,
            release: std::sync::Mutex::new(release_rx),
        };
        finish_config.finish_workflow(
            &finish_workspace_id,
            vec![],
            vec![],
            Some(&resolver),
        )
    });

    entered_rx.recv().unwrap();

    let mutation_error = config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("racing-mutation".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Optional,
                title: "must not interleave".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap_err();
    assert!(matches!(
        mutation_error,
        SessionError::RuntimeMutationConflict { .. }
    ));

    let init_error = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(workspace_id.clone()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap_err();
    assert!(matches!(
        init_error,
        SessionError::RuntimeMutationConflict { .. }
    ));

    let resume_error = config
        .resume_workspace_context(&workspace_id, &predecessor_run_id)
        .unwrap_err();
    assert!(matches!(
        resume_error,
        SessionError::RuntimeMutationConflict { .. }
    ));

    release_tx.send(()).unwrap();
    let finished = finish_thread.join().unwrap().unwrap();
    assert!(!finished.already_finished);

    let context = config.read_runtime_context(&workspace_id).unwrap();
    assert_eq!(context.runs.len(), 1);
    assert!(
        context
            .workflow
            .nodes
            .iter()
            .all(|node| node.node_id != "racing-mutation")
    );
}

/// Helper: create a workspace with one required Task node marked done and
/// then finish it, returning the config and workspace id for immutability tests.
fn finished_workspace() -> (SessionStoreConfig, String, TempDir) {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root, "context-engine");
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest { session_id: Some(uuid::Uuid::new_v4().to_string()), ..Default::default() })
        .unwrap();
    let workspace_id = init.context.session_id;

    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("seed-node".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "seed".to_string(),
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
        .workflow_update_node_status(
            &workspace_id,
            "seed-node",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();
    let finished = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap();
    assert!(!finished.already_finished);
    (config, workspace_id, tempdir)
}

/// High: resume/init lineage updates are immutable after finish. Appending a
/// new run to a finished workspace must be rejected under the lock, not
/// silently drift the run lineage of a terminal workspace.
#[test]
fn finished_workspace_rejects_resume_run_creation() {
    let (config, workspace_id, _tempdir) = finished_workspace();

    let resume_err = config
        .resume_workspace_context(&workspace_id, "any-predecessor")
        .unwrap_err();
    assert!(matches!(resume_err, SessionError::WorkspaceFinished { .. }));

    let force_err = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(workspace_id.clone()),
            predecessor_run_id: None,
            force_new_run: true,
        })
        .unwrap_err();
    assert!(matches!(force_err, SessionError::WorkspaceFinished { .. }));
}
