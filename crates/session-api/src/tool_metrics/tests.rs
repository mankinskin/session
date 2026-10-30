use super::*;
use crate::{SESSION_SCHEMA_VERSION, SessionMetadata, SessionTurn, SessionTurnEventMeta};
use chrono::TimeZone;

fn sample_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0)
        .single()
        .unwrap()
}

fn sample_time_offset(days: i64) -> DateTime<Utc> {
    Utc::now() + Duration::days(days)
}

#[test]
fn percentile_computation_is_correct() {
    let values = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
    assert_eq!(percentile(&values, 50), 50);
    assert_eq!(percentile(&values, 90), 90);
    assert_eq!(percentile(&values, 95), 100);
}

#[test]
fn failure_exclusion_from_size_percentiles() {
    let record = SessionRecord {
        schema_version: SESSION_SCHEMA_VERSION,
        session_id: "test-session".to_string(),
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
            SessionTurn {
                sequence: 1,
                role: SessionRole::Tool,
                content: "success output".to_string(),
                captured_at: sample_time(),
                tool_name: Some("test_tool".to_string()),
                model: None,
                event_meta: Some(SessionTurnEventMeta {
                    tool_success: Some(true),
                    tool_arguments_json: None,
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
                    error_message: None,
                    exit_code: None,
                    result_code: None,
                    subagent_run_id: None,
                }),
            },
            SessionTurn {
                sequence: 2,
                role: SessionRole::Tool,
                content: "failure output".to_string(),
                captured_at: sample_time(),
                tool_name: Some("test_tool".to_string()),
                model: None,
                event_meta: Some(SessionTurnEventMeta {
                    tool_success: Some(false),
                    tool_arguments_json: None,
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
                    error_message: None,
                    exit_code: None,
                    result_code: None,
                    subagent_run_id: None,
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

    let tool_summary = &summary.tools["test_tool"];
    assert_eq!(tool_summary.call_count, 2);
    assert_eq!(tool_summary.success_count, 1);
    assert_eq!(tool_summary.output_char_sizes.len(), 1);
    assert_eq!(
        tool_summary.output_char_sizes[0],
        "success output".chars().count() as u64
    );
}

#[test]
fn window_cap_filters_correctly() {
    let summaries = vec![
        SessionToolMetricsSummary {
            schema_version: TOOL_METRICS_SCHEMA_VERSION,
            session_id: "s1".to_string(),
            captured_at: sample_time_offset(-35), // 35 days ago, outside 30-day window
            tools: BTreeMap::new(),
        },
        SessionToolMetricsSummary {
            schema_version: TOOL_METRICS_SCHEMA_VERSION,
            session_id: "s2".to_string(),
            captured_at: sample_time_offset(-10), // 10 days ago, inside window
            tools: BTreeMap::new(),
        },
        SessionToolMetricsSummary {
            schema_version: TOOL_METRICS_SCHEMA_VERSION,
            session_id: "s3".to_string(),
            captured_at: sample_time_offset(-5), // 5 days ago, inside window
            tools: BTreeMap::new(),
        },
    ];

    let window = ToolMetricsWindow {
        max_age_days: Some(30),
        max_sessions: Some(100),
    };
    let estimator = CharsPerTokenEstimator::default();

    let report = aggregate(summaries, window, &estimator);
    assert_eq!(report.session_count, 2); // Only s2 and s3
}

#[test]
fn window_cap_respects_session_limit() {
    let summaries: Vec<SessionToolMetricsSummary> = (0..150)
        .map(|i| SessionToolMetricsSummary {
            schema_version: TOOL_METRICS_SCHEMA_VERSION,
            session_id: format!("session-{}", i),
            captured_at: sample_time_offset(-(i as i64)),
            tools: BTreeMap::new(),
        })
        .collect();

    let window = ToolMetricsWindow {
        max_age_days: None, // No time filter, only session limit
        max_sessions: Some(100),
    };
    let estimator = CharsPerTokenEstimator::default();

    let report = aggregate(summaries, window, &estimator);
    assert_eq!(report.session_count, 100);
}

#[test]
fn privacy_no_transcript_content_in_summary() {
    let record = SessionRecord {
        schema_version: SESSION_SCHEMA_VERSION,
        session_id: "test-session".to_string(),
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
        turns: vec![SessionTurn {
            sequence: 1,
            role: SessionRole::Tool,
            content: "secret data".to_string(),
            captured_at: sample_time(),
            tool_name: Some("test_tool".to_string()),
            model: None,
            event_meta: None,
        }],
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
    let serialized = serde_json::to_string(&summary).unwrap();

    assert!(!serialized.contains("secret data"));
    assert!(serialized.contains("test_tool"));
}

#[test]
fn tokenizer_default_uses_4_chars_per_token() {
    let estimator = CharsPerTokenEstimator::default();
    assert_eq!(estimator.chars_per_token, 4.0);
    assert_eq!(estimator.estimate_tokens(100), 25.0);
}

#[test]
fn graded_cost_linear_mapping() {
    let cal = GradedCostCalibration {
        scale_max: 100,
        tokens_at_max: 8000.0,
    };

    // Zero maps to floor
    assert_eq!(graded_cost(0.0, &cal), 1);

    // Below zero maps to floor
    assert_eq!(graded_cost(-10.0, &cal), 1);

    // At anchor maps to ceiling
    assert_eq!(graded_cost(8000.0, &cal), 100);

    // Above anchor clamps to ceiling
    assert_eq!(graded_cost(20000.0, &cal), 100);

    // Mid-range linear mapping
    assert_eq!(graded_cost(4000.0, &cal), 50); // half of anchor -> half of scale
    assert_eq!(graded_cost(800.0, &cal), 10); // 10% of anchor -> 10% of scale
    assert_eq!(graded_cost(80.0, &cal), 1); // 1% of anchor -> 1% of scale (clamped to floor)
}

#[test]
fn graded_cost_clamped_to_range() {
    let cal = GradedCostCalibration {
        scale_max: 50,
        tokens_at_max: 100.0,
    };

    // Floor clamp
    assert_eq!(graded_cost(0.1, &cal), 1);

    // Ceiling clamp
    assert_eq!(graded_cost(200.0, &cal), 50);
}

#[test]
fn rollup_round_trips_through_serde() {
    let report = ToolMetricsReport {
        schema_version: TOOL_METRICS_SCHEMA_VERSION,
        generated_at: sample_time(),
        session_count: 10,
        turn_count: 100,
        chars_per_token: 4.0,
        window: ToolMetricsWindowDescription {
            max_age_days: Some(30),
            max_sessions: Some(100),
            actual_sessions: 10,
            oldest_session_date: Some(sample_time_offset(-10)),
            newest_session_date: Some(sample_time()),
        },
        tools: vec![ToolTokenStats {
            tool_name: "test_tool".to_string(),
            call_count: 10,
            success_count: 9,
            fail_count: 1,
            timeout_count: 0,
            hang_count: 0,
            total_output_chars: 1000,
            mean_output_chars: 100.0,
            p50_output_chars: 90,
            p90_output_chars: 150,
            p95_output_chars: 180,
            max_output_chars: 200,
            est_mean_output_tokens: 25.0,
            est_p90_output_tokens: 37.5,
            mean_input_chars: 50.0,
            p50_duration_ms: Some(100),
            p95_duration_ms: Some(500),
            cost: Some(25),
            output_source_counts: BTreeMap::new(),
        }],
    };

    let rollup = ToolMetricsRollup {
        schema_version: TOOL_METRICS_SCHEMA_VERSION,
        report: report.clone(),
    };

    let serialized = serde_json::to_string(&rollup).unwrap();
    let deserialized: ToolMetricsRollup = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized.report.tools[0].cost, Some(25));
    assert_eq!(deserialized.report.session_count, 10);
}

use serde_json::json;

fn empty_record() -> SessionRecord {
    SessionRecord {
        schema_version: SESSION_SCHEMA_VERSION,
        session_id: "events-session".to_string(),
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
        turns: vec![],
        links: Default::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: Vec::new(),
        picked_up_handoff_ids: Vec::new(),
    }
}

fn completion_event(
    tool_call_id: &str,
    tool_name: &str,
    event_type: &str,
    data: serde_json::Value,
) -> CopilotHookEvent {
    CopilotHookEvent {
        event_id: Some(format!("{tool_call_id}-{event_type}")),
        parent_event_id: None,
        event_type: Some(event_type.to_string()),
        captured_at: Some(sample_time()),
        turn_id: None,
        message_id: None,
        tool_call_id: Some(tool_call_id.to_string()),
        tool_name: Some(tool_name.to_string()),
        tool_success: None,
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: None,
        data_json: Some(data),
        raw_event_json: None,
    }
}

#[path = "tests/event_reconciliation.rs"]
mod event_reconciliation;
#[allow(unused_imports)]
pub use event_reconciliation::*;

#[path = "tests/failure_classification.rs"]
mod failure_classification;
#[allow(unused_imports)]
pub use failure_classification::*;
