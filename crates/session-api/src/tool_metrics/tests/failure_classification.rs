#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn failure_timeout_hang_classification_and_duration_tracking() {
    // AC1: Prove tool-metrics is non-empty and reflects failures + slow tools
    // AC2: Verify error reasons are retrievable
    // AC3: Verify timeout/hang outcomes are countable and distinct
    use serde_json::json;

    let record = SessionRecord {
        schema_version: SESSION_SCHEMA_VERSION,
        session_id: "test-session-with-failures".to_string(),
        source: "test".to_string(),
        started_at: sample_time(),
        captured_at: sample_time(),
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
        turns: vec![
            // Success case
            SessionTurn {
                sequence: 0,
                role: SessionRole::Tool,
                content: "ok result".to_string(),
                captured_at: sample_time(),
                tool_name: Some("test_tool".to_string()),
                model: None,
                event_meta: Some(SessionTurnEventMeta {
                    tool_success: Some(true),
                    tool_arguments_json: Some(json!({"duration_ms": 100})),
                    result_code: Some("ok".to_string()),
                    subagent_run_id: None,
                    request_bytes: None,
                    request_chars: None,
                    response_bytes: None,
                    response_chars: None,
                    tokens_estimated: None,
                    error_message: None,
                    exit_code: None,
                    event_id: None,
                    parent_event_id: None,
                    event_type: None,
                    turn_id: None,
                    message_id: None,
                    tool_call_id: None,
                    reasoning_text: None,
                    tool_requests_json: None,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    cost_usd: None,
                    model_id: None,
                }),
            },
            // Error case with exit code and message
            SessionTurn {
                sequence: 1,
                role: SessionRole::Tool,
                content: "error output".to_string(),
                captured_at: sample_time(),
                tool_name: Some("test_tool".to_string()),
                model: None,
                event_meta: Some(SessionTurnEventMeta {
                    tool_success: Some(false),
                    tool_arguments_json: Some(json!({"duration_ms": 200})),
                    result_code: Some("error".to_string()),
                    subagent_run_id: None,
                    error_message: Some("Command failed with non-zero exit".to_string()),
                    exit_code: Some(1),
                    event_id: None,
                    parent_event_id: None,
                    event_type: None,
                    turn_id: None,
                    message_id: None,
                    tool_call_id: None,
                    reasoning_text: None,
                    tool_requests_json: None,
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
                }),
            },
            // Timeout case (duration >= 300000ms)
            SessionTurn {
                sequence: 2,
                role: SessionRole::Tool,
                content: "timeout output".to_string(),
                captured_at: sample_time(),
                tool_name: Some("slow_tool".to_string()),
                model: None,
                event_meta: Some(SessionTurnEventMeta {
                    tool_success: Some(false),
                    tool_arguments_json: Some(json!({"duration_ms": 305000})),
                    result_code: Some("timeout".to_string()),
                    subagent_run_id: None,
                    error_message: Some("Execution exceeded timeout cap".to_string()),
                    exit_code: None,
                    event_id: None,
                    parent_event_id: None,
                    event_type: None,
                    turn_id: None,
                    message_id: None,
                    tool_call_id: None,
                    reasoning_text: None,
                    tool_requests_json: None,
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
                }),
            },
            // Hang case
            SessionTurn {
                sequence: 3,
                role: SessionRole::Tool,
                content: "hang output".to_string(),
                captured_at: sample_time(),
                tool_name: Some("ambiguous_tool".to_string()),
                model: None,
                event_meta: Some(SessionTurnEventMeta {
                    tool_success: None,
                    tool_arguments_json: Some(json!({"duration_ms": 50000})),
                    result_code: Some("hang".to_string()),
                    subagent_run_id: None,
                    error_message: Some("sync-terminal-state-ambiguous".to_string()),
                    exit_code: None,
                    event_id: None,
                    parent_event_id: None,
                    event_type: None,
                    turn_id: None,
                    message_id: None,
                    tool_call_id: None,
                    reasoning_text: None,
                    tool_requests_json: None,
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
                }),
            },
        ],
        links: Default::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: Vec::new(),
        picked_up_handoff_ids: Vec::new(),
    };

    let estimator = CharsPerTokenEstimator::default();
    let summary = compute_session_summary(&record, &estimator);

    // Verify test_tool summary
    let test_tool = &summary.tools["test_tool"];
    assert_eq!(test_tool.call_count, 2);
    assert_eq!(test_tool.success_count, 1);
    assert_eq!(test_tool.fail_count, 1);
    assert_eq!(test_tool.timeout_count, 0);
    assert_eq!(test_tool.hang_count, 0);
    assert_eq!(test_tool.duration_ms_values, vec![100, 200]);

    // Verify slow_tool summary
    let slow_tool = &summary.tools["slow_tool"];
    assert_eq!(slow_tool.call_count, 1);
    assert_eq!(slow_tool.success_count, 0);
    assert_eq!(slow_tool.fail_count, 0);
    assert_eq!(slow_tool.timeout_count, 1);
    assert_eq!(slow_tool.hang_count, 0);
    assert_eq!(slow_tool.duration_ms_values, vec![305000]);

    // Verify ambiguous_tool summary
    let ambiguous_tool = &summary.tools["ambiguous_tool"];
    assert_eq!(ambiguous_tool.call_count, 1);
    assert_eq!(ambiguous_tool.success_count, 0);
    assert_eq!(ambiguous_tool.fail_count, 0);
    assert_eq!(ambiguous_tool.timeout_count, 0);
    assert_eq!(ambiguous_tool.hang_count, 1);
    assert_eq!(ambiguous_tool.duration_ms_values, vec![50000]);

    // Aggregate and verify report
    let window = ToolMetricsWindow {
        max_age_days: None,
        max_sessions: None,
    };
    let report = aggregate(vec![summary], window, &estimator);

    assert_eq!(report.tools.len(), 3);
    assert_eq!(report.session_count, 1);
    assert_eq!(report.turn_count, 4);

    // Verify aggregated stats contain p50/p95 duration
    let test_tool_stats = report
        .tools
        .iter()
        .find(|t| t.tool_name == "test_tool")
        .expect("test_tool should be in report");
    assert_eq!(test_tool_stats.fail_count, 1);
    assert_eq!(test_tool_stats.p50_duration_ms, Some(100)); // nearest rank: ceil(0.5*2)=1, values[0]=100
    assert_eq!(test_tool_stats.p95_duration_ms, Some(200));

    let slow_tool_stats = report
        .tools
        .iter()
        .find(|t| t.tool_name == "slow_tool")
        .expect("slow_tool should be in report");
    assert_eq!(slow_tool_stats.timeout_count, 1);
    assert_eq!(slow_tool_stats.p50_duration_ms, Some(305000));
    assert_eq!(slow_tool_stats.p95_duration_ms, Some(305000));

    let ambiguous_tool_stats = report
        .tools
        .iter()
        .find(|t| t.tool_name == "ambiguous_tool")
        .expect("ambiguous_tool should be in report");
    assert_eq!(ambiguous_tool_stats.hang_count, 1);
    assert_eq!(ambiguous_tool_stats.p50_duration_ms, Some(50000));

    // AC2: Verify error_message is preserved in the turn
    assert_eq!(
        record.turns[1]
            .event_meta
            .as_ref()
            .unwrap()
            .error_message
            .as_deref(),
        Some("Command failed with non-zero exit")
    );
    assert_eq!(
        record.turns[1].event_meta.as_ref().unwrap().exit_code,
        Some(1)
    );

    // AC3: Verify result_code distinguishes outcomes
    assert_eq!(
        record.turns[0]
            .event_meta
            .as_ref()
            .unwrap()
            .result_code
            .as_deref(),
        Some("ok")
    );
    assert_eq!(
        record.turns[1]
            .event_meta
            .as_ref()
            .unwrap()
            .result_code
            .as_deref(),
        Some("error")
    );
    assert_eq!(
        record.turns[2]
            .event_meta
            .as_ref()
            .unwrap()
            .result_code
            .as_deref(),
        Some("timeout")
    );
    assert_eq!(
        record.turns[3]
            .event_meta
            .as_ref()
            .unwrap()
            .result_code
            .as_deref(),
        Some("hang")
    );

    // AC1: Verify rollup is non-empty and serializable
    let rollup = ToolMetricsRollup {
        schema_version: TOOL_METRICS_SCHEMA_VERSION,
        report,
    };
    let json = serde_json::to_string_pretty(&rollup).unwrap();
    assert!(json.contains("\"fail_count\":"));
    assert!(json.contains("\"timeout_count\":"));
    assert!(json.contains("\"hang_count\":"));
    assert!(json.contains("\"p50_duration_ms\":"));
    assert!(json.contains("\"p95_duration_ms\":"));
}
