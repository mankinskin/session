#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn computes_tool_calls_from_captured_events_when_no_tool_turns_exist() {
    let events = vec![
        completion_event(
            "call-1",
            "grep_search",
            "tool.execution_start",
            json!({"toolName": "grep_search"}),
        ),
        completion_event(
            "call-1",
            "grep_search",
            "tool.execution_complete",
            json!({
                "result_code": "ok",
                "duration_ms": 322,
                "arguments": {"query": "abc"},
            }),
        ),
        completion_event(
            "call-2",
            "run_in_terminal",
            "tool.execution_complete",
            json!({"result_code": "error", "duration_ms": 12}),
        ),
    ];

    let summary = compute_session_summary_with_events(
        &empty_record(),
        &events,
        &CharsPerTokenEstimator::default(),
    );

    assert!(!summary.is_empty());
    let grep = &summary.tools["grep_search"];
    assert_eq!(grep.call_count, 1, "start event must not count as a call");
    assert_eq!(grep.success_count, 1);
    assert_eq!(grep.duration_ms_values, vec![322]);
    assert_eq!(grep.input_char_sizes.len(), 1);
    assert!(
        grep.output_char_sizes.is_empty(),
        "output size is unreported by the producer and must stay unmeasured"
    );

    let terminal = &summary.tools["run_in_terminal"];
    assert_eq!(terminal.call_count, 1);
    assert_eq!(terminal.fail_count, 1);
}

#[test]
pub(super) fn does_not_double_count_complete_and_result_for_the_same_tool_call() {
    let events = vec![
        completion_event(
            "call-1",
            "read_file",
            "tool.execution_complete",
            json!({"result_code": "ok"}),
        ),
        completion_event(
            "call-1",
            "read_file",
            "tool.execution_result",
            json!({"result_code": "ok"}),
        ),
    ];

    let summary = compute_session_summary_with_events(
        &empty_record(),
        &events,
        &CharsPerTokenEstimator::default(),
    );

    assert_eq!(summary.tools["read_file"].call_count, 1);
}

/// Ticket 44119807: a first persist without an output-size override (the
/// PostToolUse hook races the transcript flush) must not permanently
/// lose a later, higher-fidelity override for the same `tool_call_id`
/// (e.g. delivered by a subsequent `Stop`-triggered re-parse).
#[test]
pub(super) fn late_arriving_richer_output_copy_upgrades_the_dropped_record() {
    let events = vec![
        // First persist: terminal event with no output data reported yet.
        completion_event(
            "call-1",
            "run_in_terminal",
            "tool.execution_complete",
            json!({"result_code": "ok"}),
        ),
        // Later persist (e.g. Stop re-parse) carries the hook-payload
        // override with the highest-fidelity output_source.
        completion_event(
            "call-1",
            "run_in_terminal",
            "tool.execution_complete",
            json!({
                "result_code": "ok",
                "output_chars": 4096,
                "output_source": "hook_payload",
            }),
        ),
    ];

    let summary = compute_session_summary_with_events(
        &empty_record(),
        &events,
        &CharsPerTokenEstimator::default(),
    );

    let tool = &summary.tools["run_in_terminal"];
    assert_eq!(
        tool.call_count, 1,
        "the late copy must not recount the call"
    );
    assert_eq!(
        tool.output_char_sizes,
        vec![4096],
        "the richer late copy must land instead of being dropped"
    );
    assert_eq!(tool.output_source, vec!["hook_payload".to_string()]);
}

/// A poorer late copy (lower-fidelity source, or no output data at all)
/// must never downgrade or duplicate a richer value already recorded,
/// and replaying the identical event must not double-count anything.
#[test]
pub(super) fn poorer_or_duplicate_late_copy_never_downgrades_or_double_counts() {
    let events = vec![
        completion_event(
            "call-1",
            "run_in_terminal",
            "tool.execution_complete",
            json!({
                "result_code": "ok",
                "output_chars": 4096,
                "output_source": "hook_payload",
            }),
        ),
        // Later copy with a lower-fidelity source: must not overwrite.
        completion_event(
            "call-1",
            "run_in_terminal",
            "tool.execution_result",
            json!({
                "result_code": "ok",
                "output_chars": 12,
                "output_source": "spill_file",
            }),
        ),
        // Exact replay of the first event: must not double-count.
        completion_event(
            "call-1",
            "run_in_terminal",
            "tool.execution_complete",
            json!({
                "result_code": "ok",
                "output_chars": 4096,
                "output_source": "hook_payload",
            }),
        ),
    ];

    let summary = compute_session_summary_with_events(
        &empty_record(),
        &events,
        &CharsPerTokenEstimator::default(),
    );

    let tool = &summary.tools["run_in_terminal"];
    assert_eq!(tool.call_count, 1);
    assert_eq!(tool.success_count, 1);
    assert_eq!(
        tool.output_char_sizes,
        vec![4096],
        "a lower-fidelity late copy must not overwrite the richer value"
    );
    assert_eq!(tool.output_source, vec!["hook_payload".to_string()]);
}

#[test]
pub(super) fn summary_is_empty_when_no_tool_call_was_observed() {
    let summary = compute_session_summary_with_events(
        &empty_record(),
        &[completion_event(
            "call-1",
            "grep_search",
            "assistant.turn_end",
            json!({}),
        )],
        &CharsPerTokenEstimator::default(),
    );

    assert!(summary.is_empty());
}

/// Requirement R2: per-call output_source discriminant must survive
/// cross-session aggregation as a deterministic per-source breakdown.
#[test]
pub(super) fn aggregate_preserves_mixed_output_source_breakdown() {
    fn make_summary(sources: Vec<&str>) -> SessionToolMetricsSummary {
        let output_char_sizes: Vec<u64> = sources.iter().map(|s| s.len() as u64 * 10).collect();
        SessionToolMetricsSummary {
            schema_version: TOOL_METRICS_SCHEMA_VERSION,
            session_id: format!("session-{}", sources.join("-")),
            captured_at: sample_time(),
            tools: {
                let mut map = BTreeMap::new();
                map.insert(
                    "test_tool".to_string(),
                    ToolCallSummary {
                        call_count: sources.len() as u64,
                        success_count: sources.len() as u64,
                        fail_count: 0,
                        timeout_count: 0,
                        hang_count: 0,
                        output_char_sizes,
                        output_source: sources.iter().map(|s| s.to_string()).collect(),
                        input_char_sizes: Vec::new(),
                        duration_ms_values: Vec::new(),
                    },
                );
                map
            },
        }
    }

    let summaries = vec![
        make_summary(vec!["hook_payload", "spill_file", "transcript_turn"]),
        make_summary(vec!["hook_payload", "unspecified"]),
    ];

    let report = aggregate(
        summaries,
        ToolMetricsWindow {
            max_age_days: None,
            max_sessions: None,
        },
        &CharsPerTokenEstimator::default(),
    );

    let stats = report
        .tools
        .iter()
        .find(|t| t.tool_name == "test_tool")
        .expect("test_tool should be aggregated");
    let expected: BTreeMap<String, u64> = BTreeMap::from([
        ("hook_payload".to_string(), 2),
        ("spill_file".to_string(), 1),
        ("transcript_turn".to_string(), 1),
        ("unspecified".to_string(), 1),
    ]);
    assert_eq!(stats.output_source_counts, expected);
}
