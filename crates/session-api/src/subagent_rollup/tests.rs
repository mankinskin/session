use super::*;
use crate::{CopilotHookEvent, SessionLinks, SessionMetadata, SessionTurn, SessionTurnEventMeta};
use chrono::Utc;

#[test]
fn hook_lifecycle_events_produce_one_stopped_rollup_per_agent() {
    let started_at = DateTime::parse_from_rfc3339("2026-08-14T12:00:00Z")
        .expect("valid dispatch timestamp")
        .with_timezone(&Utc);
    let stopped_at = DateTime::parse_from_rfc3339("2026-08-14T12:01:00Z")
        .expect("valid stop timestamp")
        .with_timezone(&Utc);
    let record = SessionRecord {
        schema_version: 1,
        session_id: "parent-session".to_string(),
        source: "test".to_string(),
        started_at,
        captured_at: stopped_at,
        metadata: SessionMetadata {
            workspace_path: "test".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: None,
            trigger: None,
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree: None,
        },
        turns: Vec::new(),
        links: SessionLinks::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: Vec::new(),
        picked_up_handoff_ids: Vec::new(),
    };
    let lifecycle_event = |event_type: &str, timestamp| CopilotHookEvent {
        event_id: None,
        parent_event_id: None,
        event_type: Some(event_type.to_string()),
        captured_at: Some(timestamp),
        turn_id: None,
        message_id: None,
        tool_call_id: None,
        tool_name: None,
        tool_success: None,
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: None,
        data_json: Some(serde_json::json!({
            "agent_id": "agent-42",
            "agent_type": "Implement Agent",
        })),
        raw_event_json: None,
    };
    let events = PersistedSessionEvents {
        schema_version: 1,
        session_id: record.session_id.clone(),
        captured_at: stopped_at,
        events: vec![
            lifecycle_event("SubagentStart", started_at),
            lifecycle_event("SubagentStop", stopped_at),
        ],
    };

    let rollups = compute_subagent_rollups_with_events(&record, None, Some(&events));

    assert_eq!(rollups.len(), 1);
    let rollup = rollups.get("agent-42").expect("lifecycle rollup");
    assert_eq!(rollup.parent_session_id.as_deref(), Some("parent-session"));
    assert_eq!(rollup.agent_type.as_deref(), Some("Implement Agent"));
    assert_eq!(rollup.dispatched_at, Some(started_at));
    assert_eq!(rollup.stopped_at, Some(stopped_at));
    assert_eq!(rollup.outcome.as_deref(), Some("stopped"));
}

#[test]
fn compute_rollup_aggregates_token_counts() {
    let record = SessionRecord {
        schema_version: 1,
        session_id: "session-1".to_string(),
        source: "test".to_string(),
        started_at: Utc::now(),
        captured_at: Utc::now(),
        metadata: SessionMetadata {
            workspace_path: "test".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: Some("claude-3-5-sonnet".to_string()),
            trigger: None,
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree: None,
        },
        turns: vec![
            SessionTurn {
                sequence: 0,
                role: SessionRole::Assistant,
                content: "Hello".to_string(),
                captured_at: Utc::now(),
                tool_name: None,
                model: Some("claude-3-5-sonnet".to_string()),
                event_meta: Some(SessionTurnEventMeta {
                    event_id: None,
                    parent_event_id: None,
                    event_type: None,
                    turn_id: None,
                    message_id: None,
                    tool_call_id: None,
                    tool_success: None,
                    reasoning_text: None,
                    tool_requests_json: None,
                    tool_arguments_json: None,
                    input_tokens: Some(1000),
                    output_tokens: Some(500),
                    cache_read_tokens: Some(200),
                    cache_write_tokens: Some(100),
                    cost_usd: Some(0.05),
                    model_id: Some("claude-3-5-sonnet".to_string()),
                    request_bytes: None,
                    request_chars: None,
                    response_bytes: None,
                    response_chars: None,
                    tokens_estimated: None,
                    error_message: None,
                    exit_code: None,
                    result_code: None,
                    subagent_run_id: None,
                }),
            },
            SessionTurn {
                sequence: 1,
                role: SessionRole::Assistant,
                content: "World".to_string(),
                captured_at: Utc::now(),
                tool_name: None,
                model: Some("claude-3-5-sonnet".to_string()),
                event_meta: Some(SessionTurnEventMeta {
                    event_id: None,
                    parent_event_id: None,
                    event_type: None,
                    turn_id: None,
                    message_id: None,
                    tool_call_id: None,
                    tool_success: None,
                    reasoning_text: None,
                    tool_requests_json: None,
                    tool_arguments_json: None,
                    input_tokens: Some(2000),
                    output_tokens: Some(1000),
                    cache_read_tokens: Some(0),
                    cache_write_tokens: Some(0),
                    cost_usd: Some(0.10),
                    model_id: Some("claude-3-5-sonnet".to_string()),
                    request_bytes: None,
                    request_chars: None,
                    response_bytes: None,
                    response_chars: None,
                    tokens_estimated: None,
                    error_message: None,
                    exit_code: None,
                    result_code: None,
                    subagent_run_id: None,
                }),
            },
        ],
        links: SessionLinks::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: Vec::new(),
        picked_up_handoff_ids: Vec::new(),
    };

    let rollups = compute_subagent_rollups(&record, None);
    let rollup = rollups.get("session-1").expect("rollup should exist");

    assert_eq!(rollup.session_id, "session-1");
    assert_eq!(rollup.turn_count, 2);
    assert_eq!(rollup.input_tokens, 3000);
    assert_eq!(rollup.output_tokens, 1500);
    assert_eq!(rollup.cache_read_tokens, 200);
    assert_eq!(rollup.cache_write_tokens, 100);
    // Floating point comparison with tolerance for precision
    assert!((rollup.cost_usd.unwrap() - 0.15).abs() < 0.0001);
    assert_eq!(rollup.model.as_deref(), Some("claude-3-5-sonnet"));
}

#[test]
fn query_surface_returns_rollups_for_session_with_context() {
    use crate::SessionStoreConfig;
    use chrono::Utc;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(temp_dir.path().join(".session"));

    // Create a simple session using the capture API
    let session_id = "f5555555-5555-4555-8555-555555555555";
    use crate::hook::{CopilotHookMessage, CopilotHookPayload};

    let payload = CopilotHookPayload {
        session_id: session_id.to_string(),
        workspace_path: "test-workspace".to_string(),
        captured_at: Utc::now(),
        conversation_id: None,
        agent_id: None,
        model: Some("claude-opus-4".to_string()),
        trigger: Some("test".to_string()),
        provisioning: None,
        messages: vec![
            CopilotHookMessage {
                role: SessionRole::User,
                content: "Hello".to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: None,
            },
            CopilotHookMessage {
                role: SessionRole::Assistant,
                content: "Hi there".to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: Some(SessionTurnEventMeta {
                    event_id: None,
                    parent_event_id: None,
                    event_type: None,
                    turn_id: None,
                    message_id: None,
                    tool_call_id: None,
                    tool_success: None,
                    reasoning_text: None,
                    tool_requests_json: None,
                    tool_arguments_json: None,
                    input_tokens: Some(500),
                    output_tokens: Some(250),
                    cache_read_tokens: Some(0),
                    cache_write_tokens: Some(0),
                    cost_usd: Some(0.025),
                    model_id: Some("claude-opus-4".to_string()),
                    request_bytes: None,
                    request_chars: None,
                    response_bytes: None,
                    response_chars: None,
                    tokens_estimated: None,
                    error_message: None,
                    exit_code: None,
                    result_code: None,
                    subagent_run_id: None,
                }),
            },
        ],
        events: vec![],
        runtime: None,
    };

    config.capture_copilot_hook(payload).unwrap();

    // Query the rollups
    let rollups = config.subagent_rollups(session_id).unwrap();

    // Verify we got rollups
    assert!(!rollups.is_empty(), "Should have at least one rollup");

    // Check the main session rollup
    let rollup = rollups.get(session_id).expect("Should have session rollup");
    assert_eq!(rollup.session_id, session_id);
    assert_eq!(rollup.turn_count, 1); // One assistant turn
    assert_eq!(rollup.input_tokens, 500);
    assert_eq!(rollup.output_tokens, 250);
    assert_eq!(rollup.model.as_deref(), Some("claude-opus-4"));
    assert!((rollup.cost_usd.unwrap() - 0.025).abs() < 0.0001);
}

#[path = "tests/estimated_tokens.rs"]
mod estimated_tokens;
#[allow(unused_imports)]
pub use estimated_tokens::*;
