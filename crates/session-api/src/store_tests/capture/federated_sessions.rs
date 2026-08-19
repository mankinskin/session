fn federated_fixture(
    tempdir: &TempDir
) -> (SessionStoreConfig, SessionStoreConfig, SessionStoreConfig) {
    let main = SessionStoreConfig::new(
        tempdir.path().join(".session"),
        "context-engine",
    );
    let nested = SessionStoreConfig::new(
        tempdir
            .path()
            .join(".worktrees")
            .join("session-nested")
            .join("nested-slug")
            .join(".session"),
        "context-engine",
    );
    let legacy = SessionStoreConfig::new(
        tempdir
            .path()
            .join(".worktrees")
            .join("12345678-legacy-slug")
            .join(".session"),
        "context-engine",
    );
    (main, nested, legacy)
}

#[test]
fn federated_query_unions_main_nested_and_legacy_with_worktree_duplicate_winner(
) {
    let tempdir = TempDir::new().unwrap();
    let (main, nested, legacy) = federated_fixture(&tempdir);
    main.capture_copilot_hook(sample_payload(
        "session-main",
        None,
        sample_time(),
        &["main"],
    ))
    .unwrap();
    nested
        .capture_copilot_hook(sample_payload(
            "session-nested",
            None,
            sample_time_later(),
            &["nested"],
        ))
        .unwrap();
    legacy
        .capture_copilot_hook(sample_payload(
            "12345678-legacy",
            None,
            sample_time(),
            &["legacy"],
        ))
        .unwrap();
    main.capture_copilot_hook(sample_payload(
        "session-duplicate",
        None,
        sample_time(),
        &["main copy"],
    ))
    .unwrap();
    nested
        .capture_copilot_hook(sample_payload(
            "session-duplicate",
            None,
            sample_time_later(),
            &["worktree copy"],
        ))
        .unwrap();

    let records = main.query_sessions(&SessionQuery::default()).unwrap();
    let ids = records
        .iter()
        .map(|record| record.session_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            "session-duplicate",
            "session-nested",
            "12345678-legacy",
            "session-main"
        ]
    );
    assert_eq!(records[0].turns[0].content, "worktree copy");
}

#[test]
fn federated_tool_metrics_and_backfill_scan_main_nested_and_legacy_sessions() {
    let tempdir = TempDir::new().unwrap();
    let (main, nested, legacy) = federated_fixture(&tempdir);
    for (store, session_id) in [
        (&main, "session-main-metrics"),
        (&nested, "session-nested-metrics"),
        (&legacy, "12345678-legacy-metrics"),
    ] {
        store
            .capture_copilot_hook(sample_payload(
                session_id,
                None,
                sample_time(),
                &["federated reader fixture"],
            ))
            .unwrap();
    }

    let metrics = main
        .tool_metrics(crate::ToolMetricsWindow {
            max_age_days: None,
            max_sessions: None,
        })
        .unwrap();
    assert_eq!(metrics.session_count, 3);
    assert_eq!(main.backfill_ticket_links(false).unwrap().total_sessions, 3);
}

#[test]
fn federated_handoff_pickup_and_backlog_find_worktree_only_nested_and_legacy_handoffs(
) {
    let tempdir = TempDir::new().unwrap();
    let (main, nested, legacy) = federated_fixture(&tempdir);
    let nested_session = nested
        .init_runtime_context(crate::SessionRuntimeInitRequest {
            session_id: Some("66666666-6666-4666-8666-666666666666".to_string()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap()
        .context
        .session_id;
    let legacy_session = legacy
        .init_runtime_context(crate::SessionRuntimeInitRequest {
            session_id: Some("77777777-7777-4777-8777-777777777777".to_string()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap()
        .context
        .session_id;
    let nested_handoff = nested
        .create_handoff_record(&nested_session, None, vec![], None)
        .unwrap();
    let legacy_handoff = legacy
        .create_handoff_record(&legacy_session, None, vec![], None)
        .unwrap();
    main.capture_copilot_hook(sample_payload(
        "88888888-8888-4888-8888-888888888888",
        None,
        sample_time(),
        &["target"],
    ))
    .unwrap();
    main.capture_copilot_hook(sample_payload(
        "99999999-9999-4999-8999-999999999999",
        None,
        sample_time(),
        &["target"],
    ))
    .unwrap();

    assert!(main.read_session(&nested_session).is_err());
    assert!(main.read_session(&legacy_session).is_err());

    let backlog = main
        .list_unclaimed_handoffs(&crate::HandoffBacklogFilter::default())
        .unwrap();
    let backlog_ids = backlog
        .iter()
        .map(|handoff| handoff.handoff_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(backlog_ids.len(), 2);
    assert!(backlog_ids.contains(&nested_handoff.handoff_id.as_str()));
    assert!(backlog_ids.contains(&legacy_handoff.handoff_id.as_str()));

    let picked_nested = main
        .pickup_handoff(&nested_handoff.handoff_id, "88888888-8888-4888-8888-888888888888")
        .unwrap();
    let picked_legacy = main
        .pickup_handoff(&legacy_handoff.handoff_id, "99999999-9999-4999-8999-999999999999")
        .unwrap();
    assert_eq!(
        picked_nested.target_session_id.as_deref(),
        Some("88888888-8888-4888-8888-888888888888")
    );
    assert_eq!(
        picked_legacy.target_session_id.as_deref(),
        Some("99999999-9999-4999-8999-999999999999")
    );
}

#[test]
fn federated_ticket_relation_finds_worktree_only_session_and_skips_malformed_source(
) {
    let tempdir = TempDir::new().unwrap();
    let (main, nested, _) = federated_fixture(&tempdir);
    nested
        .check_in_worktree(sample_worktree_request(
            "55555555-5555-4555-8555-555555555555",
            "agent-worktree",
            "ticket-federated",
            managed_worktree(
                &tempdir,
                "55555555-5555-4555-8555-555555555555",
                "worktree",
                "agent/worktree",
            ),
            "agent/worktree",
        ))
        .unwrap();
    let malformed = tempdir
        .path()
        .join(".worktrees")
        .join("badbadba-malformed")
        .join(".session")
        .join("sessions")
        .join("broken");
    std::fs::create_dir_all(malformed).unwrap();

    let matches = main
        .sessions_for_ticket("ticket-federated", RelationStrength::Strict)
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].session_id, "55555555-5555-4555-8555-555555555555");
}
