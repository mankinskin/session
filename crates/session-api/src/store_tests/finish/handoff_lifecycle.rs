use crate::HandoffBacklogFilter;

fn init_workspace(config: &SessionStoreConfig, session_id: &str) -> String {
    config
        .init_runtime_context(crate::SessionRuntimeInitRequest {
            session_id: Some(session_id.to_string()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap()
        .context
        .session_id
}

fn capture_session(config: &SessionStoreConfig, session_id: &str) {
    config
        .capture_copilot_hook(crate::CopilotHookPayload {
            session_id: session_id.to_string(),
            workspace_slug: "context-engine".to_string(),
            captured_at: chrono::Utc::now(),
            conversation_id: None,
            agent_id: None,
            model: None,
            trigger: None,
            provisioning: None,
            messages: vec![crate::CopilotHookMessage {
                role: crate::SessionRole::User,
                content: "hello".to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: None,
            }],
            events: vec![],
            runtime: None,
        })
        .unwrap();
}

/// Slice 1 AC3: `SessionRecord` round-trips `emitted_handoff_ids` and
/// `picked_up_handoff_ids` through the real store persist+load path, not
/// just serde.
#[test]
fn session_record_persists_and_loads_handoff_id_lists() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    let session_id = "session-with-handoffs";
    capture_session(&config, session_id);

    let mut record = config.read_session(session_id).unwrap();
    assert!(record.emitted_handoff_ids.is_empty());
    assert!(record.picked_up_handoff_ids.is_empty());

    record.emitted_handoff_ids = vec!["handoff-a".to_string(), "handoff-b".to_string()];
    record.picked_up_handoff_ids = vec!["handoff-c".to_string()];

    let paths = config.paths_for(&record).unwrap();
    crate::SessionStorePlan {
        record: record.clone(),
        paths,
        events: None,
    }
    .persist()
    .unwrap();

    let reloaded = config.read_session(session_id).unwrap();
    assert_eq!(
        reloaded.emitted_handoff_ids,
        vec!["handoff-a".to_string(), "handoff-b".to_string()]
    );
    assert_eq!(reloaded.picked_up_handoff_ids, vec!["handoff-c".to_string()]);
}

/// AC1/AC2: pickup binds `target_session_id` and distinguishes claimed vs
/// open handoffs; also verifies the target session's `picked_up_handoff_ids`
/// is updated (AC4).
#[test]
fn pickup_binds_target_session_and_updates_target_record() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    let source_session = init_workspace(&config, "33333333-3333-4333-8333-333333333333");
    let package = crate::SessionHandoffPackage {
        objective: "Fix the bug".to_string(),
        target_tickets: vec![crate::SessionHandoffTargetTicket {
            id: "ticket-1".to_string(),
            why: "Fix the ticket".to_string(),
            state: "ready".to_string(),
            acceptance_criteria: vec![],
        }],
        higher_level_objective: "Complete the program work".to_string(),
        upward_context: vec![crate::SessionHandoffUpwardContextEntry {
            entity_urn: "ce://default/ticket/program".to_string(),
            title: "Program".to_string(),
            role: crate::SessionHandoffUpwardContextRole::Epic,
        }],
        target_files: vec![],
        decisions: vec!["decision".to_string()],
        non_goals: vec!["non-goal".to_string()],
        context_anchors: vec!["anchor".to_string()],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };
    let record = config
        .create_handoff_record(&source_session, Some(package), vec![], None)
        .unwrap();

    // Freshly created handoff is unclaimed/open.
    assert_eq!(record.target_session_id, None);
    let backlog = config
        .list_unclaimed_handoffs(&HandoffBacklogFilter::default())
        .unwrap();
    assert!(backlog.iter().any(|h| h.handoff_id == record.handoff_id));

    let target_session_id = "44444444-4444-4444-8444-444444444444";
    capture_session(&config, target_session_id);

    let picked_up = config
        .pickup_handoff(&record.handoff_id, target_session_id)
        .unwrap();
    assert_eq!(picked_up.target_session_id.as_deref(), Some(target_session_id));

    // Claimed handoffs no longer appear in the backlog.
    let backlog_after = config
        .list_unclaimed_handoffs(&HandoffBacklogFilter::default())
        .unwrap();
    assert!(!backlog_after.iter().any(|h| h.handoff_id == record.handoff_id));

    // AC4: the target session's picked_up_handoff_ids is updated.
    let target_record = config.read_session(target_session_id).unwrap();
    assert_eq!(target_record.picked_up_handoff_ids, vec![record.handoff_id.clone()]);

    // Picking up an already-claimed handoff is rejected.
    let second_attempt = config.pickup_handoff(&record.handoff_id, "55555555-5555-4555-8555-555555555555");
    assert!(matches!(
        second_attempt,
        Err(crate::SessionError::HandoffAlreadyClaimed { .. })
    ));
}

/// AC5: the backlog query is filterable by source session id and by track.
#[test]
fn backlog_query_filters_by_source_session_and_track() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    let session_a = init_workspace(&config, "11111111-1111-4111-8111-111111111111");
    let session_b = init_workspace(&config, "22222222-2222-4222-8222-222222222222");

    let package = |objective: &str| crate::SessionHandoffPackage {
        objective: objective.to_string(),
        target_tickets: vec![crate::SessionHandoffTargetTicket {
            id: "ticket-1".to_string(),
            why: String::new(),
            state: String::new(),
            acceptance_criteria: Vec::new(),
        }],
        higher_level_objective: "broader program objective".to_string(),
        upward_context: vec![crate::SessionHandoffUpwardContextEntry {
            entity_urn: "ce://default/epic/example".to_string(),
            title: "Example epic".to_string(),
            role: crate::SessionHandoffUpwardContextRole::Epic,
        }],
        target_files: vec![],
        decisions: vec!["decision".to_string()],
        non_goals: vec!["non-goal".to_string()],
        context_anchors: vec!["anchor".to_string()],
        open_escalations: vec![],
        risk_notes: None,
        predecessor_handoff: None,
    };

    let handoff_a = config
        .create_handoff_record(&session_a, Some(package("from a")), vec![], None)
        .unwrap();
    let handoff_b = config
        .create_handoff_record(&session_b, Some(package("from b")), vec![], None)
        .unwrap();

    // Filter by source session: only handoff_a comes back.
    let filtered = config
        .list_unclaimed_handoffs(&HandoffBacklogFilter {
            track_id: None,
            source_session_id: Some(session_a.clone()),
        })
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].handoff_id, handoff_a.handoff_id);

    // Filter by track: give session_b a track_id, then filter on it.
    capture_session(&config, &session_b);
    let mut record_b = config.read_session(&session_b).unwrap();
    record_b.track_id = Some("track-xyz".to_string());
    let paths_b = config.paths_for(&record_b).unwrap();
    crate::SessionStorePlan {
        record: record_b,
        paths: paths_b,
        events: None,
    }
    .persist()
    .unwrap();

    let by_track = config
        .list_unclaimed_handoffs(&HandoffBacklogFilter {
            track_id: Some("track-xyz".to_string()),
            source_session_id: None,
        })
        .unwrap();
    assert_eq!(by_track.len(), 1);
    assert_eq!(by_track[0].handoff_id, handoff_b.handoff_id);
}
