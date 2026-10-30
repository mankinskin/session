#[allow(unused_imports)]
use super::*;

/// Compute per-session tool metrics summary from a session record.
///
/// Prefer [`compute_session_summary_with_events`]: the Copilot capture path
/// never emits `role: tool` turns, so a turn-only computation is empty for
/// every real session.
pub fn compute_session_summary(
    record: &SessionRecord,
    estimator: &impl TokenEstimator,
) -> SessionToolMetricsSummary {
    compute_session_summary_with_events(record, &[], estimator)
}

/// Compute per-session tool metrics summary from a session record plus its
/// captured event stream.
///
/// Tool call telemetry lives in the captured events (`tool.execution_complete`
/// / `tool.execution_result`), not in transcript turns — the Copilot
/// transcript format has no `role: tool` message. Turn-derived calls are still
/// honoured (other producers may emit them) and are de-duplicated against the
/// event stream by `tool_call_id`.
pub fn compute_session_summary_with_events(
    record: &SessionRecord,
    events: &[CopilotHookEvent],
    _estimator: &impl TokenEstimator,
) -> SessionToolMetricsSummary {
    let mut tools = BTreeMap::<String, ToolCallSummary>::new();
    let mut tracked_calls = BTreeMap::<String, TrackedToolCall>::new();

    for turn in &record.turns {
        if turn.role != SessionRole::Tool {
            continue;
        }
        let Some(tool_name) = &turn.tool_name else {
            continue;
        };

        let tool_call_id = turn
            .event_meta
            .as_ref()
            .and_then(|meta| meta.tool_call_id.clone());

        let entry = tools
            .entry(tool_name.clone())
            .or_insert_with(ToolCallSummary::new);

        entry.call_count += 1;

        // Classify execution result from result_code or tool_success
        let result_code = turn
            .event_meta
            .as_ref()
            .and_then(|meta| meta.result_code.as_deref());

        let is_success = entry.classify(
            result_code,
            turn.event_meta.as_ref().and_then(|meta| meta.tool_success),
        );

        // Capture output chars only for successful calls (preserve existing behavior)
        let mut output_slot = None;
        let mut output_rank = OUTPUT_SOURCE_RANK_NONE;
        if is_success {
            let output_chars = turn.content.chars().count() as u64;
            entry.output_char_sizes.push(output_chars);
            entry.output_source.push("transcript_turn".to_string());
            output_slot = Some(entry.output_char_sizes.len() - 1);
            output_rank = output_source_rank("transcript_turn");
        }

        // Capture input size and duration from event_meta
        if let Some(event_meta) = &turn.event_meta {
            if let Some(args_json) = &event_meta.tool_arguments_json {
                let input_chars = serde_json::to_string(args_json).unwrap_or_default().len() as u64;
                entry.input_char_sizes.push(input_chars);
            }

            // Extract duration from data_json if present
            if let Some(data_json) = event_meta
                .tool_requests_json
                .as_ref()
                .or(event_meta.tool_arguments_json.as_ref())
            {
                if let Some(duration) = data_json
                    .get("duration_ms")
                    .or_else(|| data_json.get("durationMs"))
                    .and_then(|v| v.as_i64())
                {
                    entry.duration_ms_values.push(duration);
                }
            }
        }

        if let Some(tool_call_id) = tool_call_id {
            tracked_calls.insert(
                tool_call_id,
                TrackedToolCall {
                    tool_name: tool_name.clone(),
                    output_slot,
                    output_rank,
                },
            );
        }
    }

    for event in events {
        record_event_tool_call(event, &mut tools, &mut tracked_calls);
    }

    SessionToolMetricsSummary {
        schema_version: TOOL_METRICS_SCHEMA_VERSION,
        session_id: record.session_id.clone(),
        captured_at: record.captured_at,
        tools,
    }
}

pub(super) fn output_source_rank(source: &str) -> i8 {
    match source {
        "hook_payload" => 3,
        "spill_file" => 2,
        "transcript_turn" => 1,
        _ => 0,
    }
}

/// Accumulate a single captured tool-completion event into `tools`.
///
/// Only terminal events (`tool.execution_complete` / `tool.execution_result`)
/// are counted, so a start/complete pair yields exactly one call. A second
/// (or later) terminal event for the same `tool_call_id` — e.g. a `Stop`-
/// triggered re-parse of the transcript carrying a higher-fidelity
/// `output_source` than the first persist saw — never recounts the call or
/// its input/duration, but may upgrade the previously recorded output size
/// in place when its `output_source` outranks the one already stored. A
/// poorer late copy (lower rank, or no output data at all) never downgrades
/// or overwrites a richer one already recorded, and replaying the identical
/// event again is a no-op (rank ties do not overwrite).
pub(super) fn record_event_tool_call(
    event: &CopilotHookEvent,
    tools: &mut BTreeMap<String, ToolCallSummary>,
    tracked_calls: &mut BTreeMap<String, TrackedToolCall>,
) {
    let is_terminal = matches!(
        event.event_type.as_deref(),
        Some("tool.execution_complete")
            | Some("tool_execution_complete")
            | Some("tool.execution_result")
            | Some("tool_execution_result")
    );
    if !is_terminal {
        return;
    }

    let data = event.data_json.as_ref();
    let Some(tool_name) = event
        .tool_name
        .clone()
        .or_else(|| json_str(data, &["tool_name", "toolName"]))
    else {
        return;
    };

    // Output size is only recorded when the producer reported it. The Copilot
    // transcript carries no tool result payload, so leaving it unrecorded keeps
    // the unmeasured-tool cost policy fail-open instead of inventing a size.
    let output_chars = data.and_then(|data| {
        [
            "output_chars",
            "response_chars",
            "outputChars",
            "responseChars",
        ]
        .iter()
        .find_map(|key| data.get(*key)?.as_u64())
    });
    let output_source = output_chars.map(|_| {
        json_str(data, &["output_source", "outputSource"])
            .unwrap_or_else(|| "unspecified".to_string())
    });
    let new_rank = output_source
        .as_deref()
        .map(output_source_rank)
        .unwrap_or(OUTPUT_SOURCE_RANK_NONE);

    if let Some(tool_call_id) = event.tool_call_id.clone() {
        if let Some(tracked) = tracked_calls.get_mut(&tool_call_id) {
            // Already counted this call: never recount call_count, input, or
            // duration. Only a strictly higher-fidelity output may land.
            if let (Some(output_chars), Some(output_source)) = (output_chars, output_source) {
                if new_rank > tracked.output_rank {
                    let entry = tools
                        .entry(tracked.tool_name.clone())
                        .or_insert_with(ToolCallSummary::new);
                    match tracked.output_slot {
                        Some(index) => {
                            entry.output_char_sizes[index] = output_chars;
                            entry.output_source[index] = output_source;
                        }
                        None => {
                            entry.output_char_sizes.push(output_chars);
                            entry.output_source.push(output_source);
                            tracked.output_slot = Some(entry.output_char_sizes.len() - 1);
                        }
                    }
                    tracked.output_rank = new_rank;
                }
            }
            return;
        }
    }

    let entry = tools
        .entry(tool_name.clone())
        .or_insert_with(ToolCallSummary::new);
    entry.call_count += 1;

    let result_code = json_str(data, &["result_code", "resultCode"]);
    let tool_success = event
        .tool_success
        .or_else(|| data.and_then(|data| data.get("success")?.as_bool()));
    entry.classify(result_code.as_deref(), tool_success);

    if let Some(arguments) = event
        .tool_arguments_json
        .as_ref()
        .or_else(|| data.and_then(|data| data.get("arguments")))
    {
        let input_chars = serde_json::to_string(arguments).unwrap_or_default().len() as u64;
        entry.input_char_sizes.push(input_chars);
    }

    let mut output_slot = None;
    if let (Some(output_chars), Some(output_source)) = (output_chars, output_source) {
        entry.output_char_sizes.push(output_chars);
        entry.output_source.push(output_source);
        output_slot = Some(entry.output_char_sizes.len() - 1);
    }

    if let Some(tool_call_id) = event.tool_call_id.clone() {
        tracked_calls.insert(
            tool_call_id,
            TrackedToolCall {
                tool_name,
                output_slot,
                output_rank: new_rank,
            },
        );
    }

    if let Some(duration) = data.and_then(|data| {
        data.get("duration_ms")
            .or_else(|| data.get("durationMs"))?
            .as_i64()
    }) {
        entry.duration_ms_values.push(duration);
    }
}

pub(super) fn json_str(value: Option<&serde_json::Value>, keys: &[&str]) -> Option<String> {
    let value = value?;
    keys.iter()
        .find_map(|key| value.get(*key)?.as_str())
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
}
