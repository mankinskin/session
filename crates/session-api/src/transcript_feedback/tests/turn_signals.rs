use chrono::Utc;

use super::super::super::*;

fn tool_turn(
    sequence: usize,
    tool_success: Option<bool>,
) -> SessionTurn {
    SessionTurn {
        sequence,
        role: SessionRole::Tool,
        content: String::new(),
        captured_at: Utc::now(),
        tool_name: Some("get_ticket".to_string()),
        model: None,
        event_meta: Some(SessionTurnEventMeta {
            event_id: Some(format!("evt-{sequence}")),
            parent_event_id: None,
            event_type: Some("tool.result".to_string()),
            turn_id: None,
            message_id: None,
            tool_call_id: Some(format!("call-{sequence}")),
            tool_success,
            reasoning_text: None,
            tool_requests_json: None,
            tool_arguments_json: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            cost_usd: None,
            model_id: None,
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
    }
}

#[test]
fn detects_failed_tool_calls_from_structured_metadata() {
    let turns = vec![
        tool_turn(0, Some(true)),
        tool_turn(1, Some(false)),
        tool_turn(2, None),
    ];

    let signals = mine_structured_feedback_signals(&turns);

    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].kind, FeedbackSignalKind::FailedToolCall);
    assert_eq!(signals[0].sequence, Some(1));
    assert_eq!(signals[0].tool_call_id.as_deref(), Some("call-1"));
}

#[test]
fn ignores_message_text_and_non_tool_roles() {
    let mut assistant = tool_turn(0, Some(false));
    assistant.role = SessionRole::Assistant;
    assistant.content =
        "This failed with a conflict and the wrong error".to_string();

    let signals = mine_structured_feedback_signals(&[assistant]);

    assert!(signals.is_empty());
}
