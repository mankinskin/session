#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn persist_capture_keeps_distinct_id_less_events_by_data_json() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));

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
    let events: PersistedSessionEvents = serde_json::from_str(&events_text).unwrap();
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
pub(super) fn query_sessions_filters_by_text_and_metadata() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));

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

    assert_eq!(by_text.sessions.len(), 1);
    assert_eq!(by_text.sessions[0].session_id, "session-beta");
    assert_eq!(by_conversation.sessions.len(), 1);
    assert_eq!(by_conversation.sessions[0].session_id, "session-alpha");
}

#[test]
pub(super) fn query_sessions_skips_unreadable_records_and_reports_safe_diagnostics() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(&store_root);
    config
        .capture_copilot_hook(sample_payload(
            "session-readable",
            Some("conversation-readable"),
            sample_time(),
            &["Readable session"],
        ))
        .unwrap();

    let unreadable_id = "f3333333-3333-4333-8333-333333333333";
    let unreadable_dir = store_root.join("sessions").join(unreadable_id);
    std::fs::create_dir_all(&unreadable_dir).unwrap();
    std::fs::write(
        unreadable_dir.join("session.json"),
        b"private malformed session payload",
    )
    .unwrap();

    let result = config.query_sessions(&SessionQuery::default()).unwrap();

    assert_eq!(result.sessions.len(), 1);
    assert_eq!(result.sessions[0].session_id, "session-readable");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].session_id, unreadable_id);
    assert_eq!(result.diagnostics[0].code, "unreadable_session_record");

    let diagnostics_json = serde_json::to_string(&result.diagnostics).unwrap();
    assert!(!diagnostics_json.contains("private malformed session payload"));
    assert!(!diagnostics_json.contains(store_root.to_string_lossy().as_ref()));
}

#[test]
pub(super) fn capture_copilot_transcript_persists_visible_transcript_messages() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
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
pub(super) fn capture_copilot_transcript_allows_divergent_newer_snapshot() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
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
pub(super) fn capture_copilot_transcript_with_tool_response_never_blocks_on_missing_override_match()
{
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
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
pub(super) fn new_events_file_omits_raw_event_json() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));

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
        tool_arguments_json: Some(serde_json::json!({ "path": "size-test.rs" })),
        data_json: Some(serde_json::json!({ "arguments": { "path": "size-test.rs" } })),
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

#[path = "query_and_worktree/worktree_assignments.rs"]
mod worktree_assignments;
#[allow(unused_imports)]
pub use worktree_assignments::*;
