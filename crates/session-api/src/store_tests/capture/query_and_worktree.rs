#[test]
fn persist_capture_keeps_distinct_id_less_events_by_data_json() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    let mut first = sample_payload(
        "session-events",
        Some("conversation-events"),
        sample_time(),
        &["first"],
    );
    first.events = vec![crate::CopilotHookEvent {
        event_id: None,
        parent_event_id: None,
        event_type: Some("tool.execution_complete".to_string()),
        captured_at: Some(sample_time()),
        turn_id: None,
        message_id: None,
        tool_call_id: Some("call-1".to_string()),
        tool_name: Some("read_file".to_string()),
        tool_success: Some(true),
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: Some(serde_json::json!({ "path": "A" })),
        data_json: Some(serde_json::json!({ "arguments": { "path": "A" } })),
        raw_event_json: Some(serde_json::json!({
            "type": "tool.execution_complete",
            "data": { "arguments": { "path": "A" } }
        })),
    }];
    config
        .persist_capture(SessionCaptureRequest::copilot(first))
        .unwrap();

    let mut second = sample_payload(
        "session-events",
        Some("conversation-events"),
        sample_time_later(),
        &["first", "second"],
    );
    second.events = vec![crate::CopilotHookEvent {
        event_id: None,
        parent_event_id: None,
        event_type: Some("tool.execution_complete".to_string()),
        captured_at: Some(sample_time()),
        turn_id: None,
        message_id: None,
        tool_call_id: Some("call-1".to_string()),
        tool_name: Some("read_file".to_string()),
        tool_success: Some(true),
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: Some(serde_json::json!({ "path": "B" })),
        data_json: Some(serde_json::json!({ "arguments": { "path": "B" } })),
        raw_event_json: Some(serde_json::json!({
            "type": "tool.execution_complete",
            "data": { "arguments": { "path": "B" } }
        })),
    }];
    let plan = config
        .persist_capture(SessionCaptureRequest::copilot(second))
        .unwrap();

    let events_text = std::fs::read_to_string(&plan.paths.events_path).unwrap();
    let events: PersistedSessionEvents =
        serde_json::from_str(&events_text).unwrap();
    assert_eq!(events.events.len(), 2);
    // data_json is the canonical payload and must carry the distinct values.
    assert!(events.events.iter().any(|event| {
        event
            .data_json
            .as_ref()
            .and_then(|json| json.pointer("/arguments/path"))
            .and_then(serde_json::Value::as_str)
            == Some("A")
    }));
    assert!(events.events.iter().any(|event| {
        event
            .data_json
            .as_ref()
            .and_then(|json| json.pointer("/arguments/path"))
            .and_then(serde_json::Value::as_str)
            == Some("B")
    }));
    // raw_event_json must not be written to the persisted file (AC1).
    assert!(
        events
            .events
            .iter()
            .all(|event| event.raw_event_json.is_none())
    );
}

#[test]
fn query_sessions_filters_by_text_and_metadata() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    config
        .capture_copilot_hook(sample_payload(
            "session-alpha",
            Some("conversation-alpha"),
            sample_time(),
            &["Investigate failing test"],
        ))
        .unwrap();
    config
        .capture_copilot_hook(sample_payload(
            "session-beta",
            Some("conversation-beta"),
            sample_time_later(),
            &["Document hook query behavior"],
        ))
        .unwrap();

    let by_text = config
        .query_sessions(&SessionQuery {
            text: Some("hook query".to_string()),
            ..SessionQuery::default()
        })
        .unwrap();
    let by_conversation = config
        .query_sessions(&SessionQuery {
            conversation_id: Some("conversation-alpha".to_string()),
            ..SessionQuery::default()
        })
        .unwrap();

    assert_eq!(by_text.len(), 1);
    assert_eq!(by_text[0].session_id, "session-beta");
    assert_eq!(by_conversation.len(), 1);
    assert_eq!(by_conversation[0].session_id, "session-alpha");
}

#[test]
fn capture_copilot_transcript_persists_visible_transcript_messages() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let transcript_path = tempdir.path().join("copilot.jsonl");

    std::fs::write(
            &transcript_path,
            concat!(
                "{\"type\":\"session.start\",\"timestamp\":\"2026-06-02T23:06:54.049Z\",\"data\":{\"sessionId\":\"session-transcript\",\"producer\":\"copilot-agent\",\"startTime\":\"2026-06-02T23:06:54.049Z\"}}\n",
                "{\"type\":\"user.message\",\"timestamp\":\"2026-06-02T23:07:00.000Z\",\"data\":{\"content\":\"Persist this transcript\"}}\n",
                "{\"type\":\"assistant.message\",\"timestamp\":\"2026-06-02T23:07:05.000Z\",\"data\":{\"content\":\"Transcript persisted.\"}}\n",
                "{\"type\":\"assistant.message\",\"timestamp\":\"2026-06-02T23:07:06.000Z\",\"data\":{\"content\":\"\"}}\n"
            ),
        )
        .unwrap();

    let plan = config
        .capture_copilot_transcript(&transcript_path, "stop")
        .unwrap();
    let record = config.read_session("session-transcript").unwrap();

    assert!(plan.paths.manifest_path.exists());
    assert_eq!(record.session_id, "session-transcript");
    assert_eq!(record.metadata.trigger.as_deref(), Some("stop"));
    assert_eq!(record.turns.len(), 2);
    assert_eq!(record.turns[0].content, "Persist this transcript");
    assert_eq!(record.turns[1].content, "Transcript persisted.");
}

#[test]
fn capture_copilot_transcript_allows_divergent_newer_snapshot() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let transcript_path = tempdir.path().join("copilot.jsonl");

    std::fs::write(
            &transcript_path,
            concat!(
                "{\"type\":\"session.start\",\"timestamp\":\"2026-06-02T23:06:54.049Z\",\"data\":{\"sessionId\":\"session-sync\",\"producer\":\"copilot-agent\",\"startTime\":\"2026-06-02T23:06:54.049Z\"}}\n",
                "{\"type\":\"user.message\",\"timestamp\":\"2026-06-02T23:07:00.000Z\",\"data\":{\"content\":\"Original prompt\"}}\n",
                "{\"type\":\"assistant.message\",\"timestamp\":\"2026-06-02T23:07:05.000Z\",\"data\":{\"content\":\"Original response\"}}\n"
            ),
        )
        .unwrap();

    config
        .capture_copilot_transcript(&transcript_path, "PostToolUse")
        .unwrap();

    std::fs::write(
            &transcript_path,
            concat!(
                "{\"type\":\"session.start\",\"timestamp\":\"2026-06-02T23:06:54.049Z\",\"data\":{\"sessionId\":\"session-sync\",\"producer\":\"copilot-agent\",\"startTime\":\"2026-06-02T23:06:54.049Z\"}}\n",
                "{\"type\":\"user.message\",\"timestamp\":\"2026-06-02T23:07:00.000Z\",\"data\":{\"content\":\"Edited prompt\"}}\n",
                "{\"type\":\"assistant.message\",\"timestamp\":\"2026-06-02T23:07:05.000Z\",\"data\":{\"content\":\"Edited response\"}}\n",
                "{\"type\":\"assistant.message\",\"timestamp\":\"2026-06-02T23:07:07.000Z\",\"data\":{\"content\":\"Additional message\"}}\n"
            ),
        )
        .unwrap();

    config
        .capture_copilot_transcript(&transcript_path, "PostToolUse")
        .unwrap();

    let record = config.read_session("session-sync").unwrap();
    assert_eq!(record.turns.len(), 3);
    assert_eq!(record.turns[0].content, "Edited prompt");
    assert_eq!(record.turns[2].content, "Additional message");
}

/// Ticket 44119807 Step B: the synchronous capture path must never sleep or
/// retry waiting for a tool-response override to land in the transcript.
/// Even when the override's `tool_call_id` never appears in the parsed
/// transcript (the worst case that used to drive up to 12 * 200ms of
/// blocking retries here), the call must return promptly and persist
/// whatever was parsed on the single attempt.
#[test]
fn capture_copilot_transcript_with_tool_response_never_blocks_on_missing_override_match()
 {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let transcript_path = tempdir.path().join("copilot.jsonl");

    std::fs::write(
            &transcript_path,
            concat!(
                "{\"type\":\"session.start\",\"timestamp\":\"2026-06-02T23:06:54.049Z\",\"data\":{\"sessionId\":\"session-no-retry\",\"producer\":\"copilot-agent\",\"startTime\":\"2026-06-02T23:06:54.049Z\"}}\n",
                "{\"type\":\"user.message\",\"timestamp\":\"2026-06-02T23:07:00.000Z\",\"data\":{\"content\":\"Run a tool\"}}\n"
            ),
        )
        .unwrap();

    let override_value = crate::ToolResponseOverride {
        tool_call_id: "call-never-appears".to_string(),
        output_chars: 4096,
        output_source: "hook_payload".to_string(),
    };

    let started = std::time::Instant::now();
    config
        .capture_copilot_transcript_with_tool_response(
            &transcript_path,
            "PostToolUse",
            Some(override_value),
        )
        .unwrap();
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "sync capture path must not sleep/retry waiting for the override to \
         land; took {elapsed:?} (old blocking loop took up to 2.4s)"
    );

    let record = config.read_session("session-no-retry").unwrap();
    assert_eq!(record.turns.len(), 1);
    assert_eq!(record.turns[0].content, "Run a tool");
}

#[test]
fn check_in_worktree_creates_and_returns_new_assignment() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
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
fn check_in_worktree_reuses_existing_assignment_for_same_session() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
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
fn check_in_worktree_claims_unclaimed_hook_record() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let worktree_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a",
        "agent/initial",
    );

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
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap())
            .unwrap();
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
fn check_in_worktree_rejects_mismatched_claimed_owner() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let worktree_path = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session-a",
        "agent/claimed",
    );

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
fn check_in_worktree_rotates_for_handoff_and_supersedes_predecessor() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
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
fn new_events_file_omits_raw_event_json() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    let mut payload = sample_payload(
        "session-size-check",
        Some("conversation-size-check"),
        sample_time(),
        &["size regression check"],
    );
    payload.events = vec![crate::CopilotHookEvent {
        event_id: None,
        parent_event_id: None,
        event_type: Some("tool.execution_complete".to_string()),
        captured_at: Some(sample_time()),
        turn_id: None,
        message_id: None,
        tool_call_id: Some("call-size-1".to_string()),
        tool_name: Some("read_file".to_string()),
        tool_success: Some(true),
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: Some(
            serde_json::json!({ "path": "size-test.rs" }),
        ),
        data_json: Some(
            serde_json::json!({ "arguments": { "path": "size-test.rs" } }),
        ),
        raw_event_json: Some(serde_json::json!({
            "type": "tool.execution_complete",
            "data": { "arguments": { "path": "size-test.rs" } }
        })),
    }];
    let plan = config
        .persist_capture(SessionCaptureRequest::copilot(payload))
        .unwrap();

    let events_text = std::fs::read_to_string(&plan.paths.events_path).unwrap();
    assert!(
        !events_text.contains("raw_event_json"),
        "newly written events.json must not contain the key 'raw_event_json', \
         but found it in: {events_text}"
    );
}

#[test]
fn lookup_reads_branch_manifest_without_a_transcript() {
    let tempdir = TempDir::new().unwrap();
    let main_checkout = tempdir.path();
    let worktree = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session",
        "agent/session-a",
    );
    let config = SessionStoreConfig::new(
        main_checkout.join(".session"),
        "context-engine",
    );

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
    assert!(matches!(
        config.peek_range(WORKTREE_SESSION_A, 0, None),
        Err(SessionError::NotFound { .. })
    ));
}

#[test]
fn check_in_writes_untracked_main_registry_and_branch_only_manifests() {
    let tempdir = TempDir::new().unwrap();
    let main_checkout = tempdir.path();
    let worktree = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session",
        "agent/session-a",
    );
    let worktree_config =
        SessionStoreConfig::new(worktree.join(".session"), "context-engine");
    let main_config = SessionStoreConfig::new(
        main_checkout.join(".session"),
        "context-engine",
    );

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
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(worktree_manifest).unwrap(),
    )
    .unwrap();
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
fn lookup_rejects_registry_entry_for_missing_worktree() {
    let tempdir = TempDir::new().unwrap();
    let main_checkout = tempdir.path();
    let worktree = managed_worktree(
        &tempdir,
        WORKTREE_SESSION_A,
        "session",
        "agent/session-a",
    );
    let config = SessionStoreConfig::new(
        main_checkout.join(".session"),
        "context-engine",
    );
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
