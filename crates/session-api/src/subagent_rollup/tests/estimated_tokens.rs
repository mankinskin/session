#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn rollup_aggregates_estimated_tokens() {
    // AC1, AC2, AC4: Verify tokens_estimated aggregation and null vs zero distinction
    let record = SessionRecord {
        schema_version: 1,
        session_id: "session-with-estimates".to_string(),
        source: "test".to_string(),
        started_at: Utc::now(),
        captured_at: Utc::now(),
        metadata: SessionMetadata {
            workspace_path: "test".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: Some("claude-opus-4".to_string()),
            trigger: None,
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree: None,
        },
        turns: vec![
            // MCP tool call with estimated tokens
            SessionTurn {
                sequence: 0,
                role: SessionRole::Tool,
                content: "Tool result".to_string(),
                captured_at: Utc::now(),
                tool_name: Some("read_file".to_string()),
                model: None,
                event_meta: Some(SessionTurnEventMeta {
                    event_id: None,
                    parent_event_id: None,
                    event_type: None,
                    turn_id: None,
                    message_id: None,
                    tool_call_id: Some("call-1".to_string()),
                    tool_success: Some(true),
                    reasoning_text: None,
                    tool_requests_json: None,
                    tool_arguments_json: None,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    cost_usd: None,
                    model_id: None,
                    request_bytes: Some(256),
                    request_chars: Some(200),
                    response_bytes: Some(512),
                    response_chars: Some(400),
                    tokens_estimated: Some(150), // (200+400)/4
                    error_message: None,
                    exit_code: None,
                    result_code: None,
                    subagent_run_id: None,
                }),
            },
            // Another MCP tool call
            SessionTurn {
                sequence: 1,
                role: SessionRole::Tool,
                content: "Another tool result".to_string(),
                captured_at: Utc::now(),
                tool_name: Some("write_file".to_string()),
                model: None,
                event_meta: Some(SessionTurnEventMeta {
                    event_id: None,
                    parent_event_id: None,
                    event_type: None,
                    turn_id: None,
                    message_id: None,
                    tool_call_id: Some("call-2".to_string()),
                    tool_success: Some(true),
                    reasoning_text: None,
                    tool_requests_json: None,
                    tool_arguments_json: None,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    cost_usd: None,
                    model_id: None,
                    request_bytes: Some(128),
                    request_chars: Some(100),
                    response_bytes: Some(64),
                    response_chars: Some(50),
                    tokens_estimated: Some(37), // (100+50)/4, rounded down
                    error_message: None,
                    exit_code: None,
                    result_code: None,
                    subagent_run_id: None,
                }),
            },
            // Non-MCP turn with null telemetry (AC4: null vs zero)
            SessionTurn {
                sequence: 2,
                role: SessionRole::Assistant,
                content: "Response without tool call".to_string(),
                captured_at: Utc::now(),
                tool_name: None,
                model: Some("claude-opus-4".to_string()),
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
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    cost_usd: Some(0.05),
                    model_id: Some("claude-opus-4".to_string()),
                    request_bytes: None, // AC4: null, not zero
                    request_chars: None,
                    response_bytes: None,
                    response_chars: None,
                    tokens_estimated: None, // AC4: null for non-MCP traffic
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
    let rollup = rollups
        .get("session-with-estimates")
        .expect("rollup should exist");

    // AC1, AC2: Verify non-zero aggregation
    assert_eq!(
        rollup.tokens_estimated,
        Some(187),
        "tokens_estimated should aggregate: 150 + 37 = 187"
    );
    assert_eq!(rollup.tool_call_count, 2, "should count both tool calls");
    assert_eq!(rollup.turn_count, 1, "should count one assistant turn");

    // AC4: Verify the turn without telemetry contributed null, not zero
    // (the aggregate is Some(187), not Some(187+0), proving null was preserved)

    // AC5: cost_usd remains Some() because the assistant turn had it
    assert_eq!(rollup.cost_usd, Some(0.05));

    // Verify model attribution
    assert_eq!(rollup.model.as_deref(), Some("claude-opus-4"));
}

#[test]
pub(super) fn rollup_with_no_estimates_yields_none() {
    // AC4, AC6: Verify null is preserved when no MCP traffic exists
    let record = SessionRecord {
        schema_version: 1,
        session_id: "session-no-mcp".to_string(),
        source: "test".to_string(),
        started_at: Utc::now(),
        captured_at: Utc::now(),
        metadata: SessionMetadata {
            workspace_path: "test".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: Some("gpt-4".to_string()),
            trigger: None,
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree: None,
        },
        turns: vec![SessionTurn {
            sequence: 0,
            role: SessionRole::Assistant,
            content: "No tool calls here".to_string(),
            captured_at: Utc::now(),
            tool_name: None,
            model: Some("gpt-4".to_string()),
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
                input_tokens: Some(100),
                output_tokens: Some(50),
                cache_read_tokens: None,
                cache_write_tokens: None,
                cost_usd: Some(0.01),
                model_id: Some("gpt-4".to_string()),
                request_bytes: None,
                request_chars: None,
                response_bytes: None,
                response_chars: None,
                tokens_estimated: None, // AC4: null for non-MCP
                error_message: None,
                exit_code: None,
                result_code: None,
                subagent_run_id: None,
            }),
        }],
        links: SessionLinks::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: Vec::new(),
        picked_up_handoff_ids: Vec::new(),
    };

    let rollups = compute_subagent_rollups(&record, None);
    let rollup = rollups.get("session-no-mcp").expect("rollup should exist");

    // AC4, AC6: tokens_estimated should be None (not Some(0))
    assert_eq!(
        rollup.tokens_estimated, None,
        "tokens_estimated should be None when no MCP traffic exists"
    );
    assert_eq!(rollup.tool_call_count, 0);
    assert_eq!(rollup.input_tokens, 100);
    assert_eq!(rollup.output_tokens, 50);
}
