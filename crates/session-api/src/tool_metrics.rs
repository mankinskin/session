use chrono::{
    DateTime,
    Duration,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};
use std::{
    collections::BTreeMap,
    path::Path,
};

use crate::{
    CopilotHookEvent,
    SessionError,
    SessionRecord,
    SessionRole,
};

/// Trait for estimating token counts from character counts.
pub trait TokenEstimator {
    fn estimate_tokens(
        &self,
        chars: u64,
    ) -> f64;
}

/// Default token estimator using a fixed chars-per-token ratio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharsPerTokenEstimator {
    pub chars_per_token: f64,
}

impl Default for CharsPerTokenEstimator {
    fn default() -> Self {
        Self {
            chars_per_token: 4.0,
        }
    }
}

impl TokenEstimator for CharsPerTokenEstimator {
    fn estimate_tokens(
        &self,
        chars: u64,
    ) -> f64 {
        chars as f64 / self.chars_per_token
    }
}

/// Per-tool token statistics aggregated across sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolTokenStats {
    pub tool_name: String,
    pub call_count: u64,
    pub success_count: u64,
    pub fail_count: u64,
    pub timeout_count: u64,
    pub hang_count: u64,
    pub total_output_chars: u64,
    pub mean_output_chars: f64,
    pub p50_output_chars: u64,
    pub p90_output_chars: u64,
    pub p95_output_chars: u64,
    pub max_output_chars: u64,
    pub est_mean_output_tokens: f64,
    pub est_p90_output_tokens: f64,
    pub mean_input_chars: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50_duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p95_duration_ms: Option<i64>,
    /// Optional graded cost (1..=scale_max) for this tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<u32>,
    /// Per-source counts of recorded tool outputs (e.g. "hook_payload",
    /// "spill_file", "transcript_turn"). Empty maps are omitted for backward
    /// compatibility with rollups produced before source attribution.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub output_source_counts: BTreeMap<String, u64>,
}

/// Aggregated tool metrics report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolMetricsReport {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub session_count: usize,
    pub turn_count: usize,
    pub chars_per_token: f64,
    pub window: ToolMetricsWindowDescription,
    pub tools: Vec<ToolTokenStats>,
}

/// Description of the window used for aggregation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMetricsWindowDescription {
    pub max_age_days: Option<u32>,
    pub max_sessions: Option<usize>,
    pub actual_sessions: usize,
    pub oldest_session_date: Option<DateTime<Utc>>,
    pub newest_session_date: Option<DateTime<Utc>>,
}

/// Calibration for graded cost mapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradedCostCalibration {
    /// Maximum value of the cost scale (default 100).
    pub scale_max: u32,
    /// Token count that maps to scale_max (tunable anchor).
    /// Default 150.0 places typical heavy tools mid-high on the scale.
    pub tokens_at_max: f64,
}

impl Default for GradedCostCalibration {
    fn default() -> Self {
        Self {
            scale_max: 100,
            // TODO: provisional anchor; re-tune from the first empirical tool-metrics rollup
            tokens_at_max: 8000.0,
        }
    }
}

/// Compute graded cost from estimated tokens using linear mapping.
/// Maps [0, tokens_at_max] -> [1, scale_max], clamped.
pub fn graded_cost(
    est_tokens: f64,
    cal: &GradedCostCalibration,
) -> u32 {
    if est_tokens <= 0.0 {
        return 1;
    }
    let ratio = est_tokens / cal.tokens_at_max;
    let scaled = (ratio * cal.scale_max as f64).ceil() as u32;
    scaled.clamp(1, cal.scale_max)
}

/// Schema-versioned rollup containing the report plus per-tool graded costs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolMetricsRollup {
    pub schema_version: u32,
    pub report: ToolMetricsReport,
}

/// Window configuration for tool metrics aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolMetricsWindow {
    pub max_age_days: Option<u32>,
    pub max_sessions: Option<usize>,
}

impl Default for ToolMetricsWindow {
    fn default() -> Self {
        Self {
            max_age_days: Some(30),
            max_sessions: Some(100),
        }
    }
}

/// Per-session tool metrics summary (no transcript content, only sizes and counts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionToolMetricsSummary {
    pub schema_version: u32,
    pub session_id: String,
    pub captured_at: DateTime<Utc>,
    pub tools: BTreeMap<String, ToolCallSummary>,
}

impl SessionToolMetricsSummary {
    /// `true` when no tool call was observed for this session. Callers use
    /// this to avoid persisting an empty `tool-metrics.json` sidecar.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// Per-tool call summary within a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub call_count: u64,
    pub success_count: u64,
    pub fail_count: u64,
    pub timeout_count: u64,
    pub hang_count: u64,
    pub output_char_sizes: Vec<u64>,
    /// Provenance for each entry in `output_char_sizes`, aligned by index
    /// (e.g. "hook_payload", "spill_file", "transcript_turn").
    pub output_source: Vec<String>,
    pub input_char_sizes: Vec<u64>,
    pub duration_ms_values: Vec<i64>,
}

impl ToolCallSummary {
    fn new() -> Self {
        Self {
            call_count: 0,
            success_count: 0,
            fail_count: 0,
            timeout_count: 0,
            hang_count: 0,
            output_char_sizes: Vec::new(),
            output_source: Vec::new(),
            input_char_sizes: Vec::new(),
            duration_ms_values: Vec::new(),
        }
    }

    /// Increment the outcome bucket implied by `result_code`, falling back to
    /// `tool_success` when the code is missing or unrecognized. Returns whether
    /// the call was classified as successful.
    fn classify(
        &mut self,
        result_code: Option<&str>,
        tool_success: Option<bool>,
    ) -> bool {
        match result_code {
            Some("ok") => {
                self.success_count += 1;
                true
            },
            Some("error") => {
                self.fail_count += 1;
                false
            },
            Some("timeout") => {
                self.timeout_count += 1;
                false
            },
            Some("hang") => {
                self.hang_count += 1;
                false
            },
            _ => {
                let success = tool_success != Some(false);
                if success {
                    self.success_count += 1;
                } else {
                    self.fail_count += 1;
                }
                success
            },
        }
    }
}

const TOOL_METRICS_SCHEMA_VERSION: u32 = 1;

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
                let input_chars = serde_json::to_string(args_json)
                    .unwrap_or_default()
                    .len() as u64;
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

/// Fidelity rank of an `output_source` value, matching the layering
/// documented on [`crate::hook::transcript::ToolResponseOverride`]:
/// `hook_payload` (highest fidelity) > `spill_file` > `transcript_turn` >
/// any unrecognized/`unspecified` source. Used by [`record_event_tool_call`]
/// to decide whether a later copy of an already-counted `tool_call_id` may
/// upgrade the recorded output size, never downgrade it.
const OUTPUT_SOURCE_RANK_NONE: i8 = -1;

fn output_source_rank(source: &str) -> i8 {
    match source {
        "hook_payload" => 3,
        "spill_file" => 2,
        "transcript_turn" => 1,
        _ => 0,
    }
}

/// Per-`tool_call_id` bookkeeping so a later, higher-fidelity copy of an
/// already-counted call's output can overwrite the earlier value in place
/// instead of being discarded (ticket 44119807) or double-counted.
struct TrackedToolCall {
    tool_name: String,
    /// Index into `tools[tool_name].output_char_sizes` / `.output_source`
    /// holding this call's recorded output, if any was recorded yet.
    output_slot: Option<usize>,
    output_rank: i8,
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
fn record_event_tool_call(
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
            if let (Some(output_chars), Some(output_source)) =
                (output_chars, output_source)
            {
                if new_rank > tracked.output_rank {
                    let entry = tools
                        .entry(tracked.tool_name.clone())
                        .or_insert_with(ToolCallSummary::new);
                    match tracked.output_slot {
                        Some(index) => {
                            entry.output_char_sizes[index] = output_chars;
                            entry.output_source[index] = output_source;
                        },
                        None => {
                            entry.output_char_sizes.push(output_chars);
                            entry.output_source.push(output_source);
                            tracked.output_slot =
                                Some(entry.output_char_sizes.len() - 1);
                        },
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
        let input_chars =
            serde_json::to_string(arguments).unwrap_or_default().len() as u64;
        entry.input_char_sizes.push(input_chars);
    }

    let mut output_slot = None;
    if let (Some(output_chars), Some(output_source)) =
        (output_chars, output_source)
    {
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

fn json_str(
    value: Option<&serde_json::Value>,
    keys: &[&str],
) -> Option<String> {
    let value = value?;
    keys.iter()
        .find_map(|key| value.get(*key)?.as_str())
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
}

/// Aggregate tool metrics from multiple session summaries with window filtering.
pub fn aggregate(
    summaries: Vec<SessionToolMetricsSummary>,
    window: ToolMetricsWindow,
    estimator: &impl TokenEstimator,
) -> ToolMetricsReport {
    aggregate_with_cost(summaries, window, estimator, None)
}

/// Aggregate tool metrics with optional cost calibration.
pub fn aggregate_with_cost(
    summaries: Vec<SessionToolMetricsSummary>,
    window: ToolMetricsWindow,
    estimator: &impl TokenEstimator,
    cost_cal: Option<GradedCostCalibration>,
) -> ToolMetricsReport {
    let mut filtered_summaries = summaries;

    // Apply time window filter
    if let Some(max_age_days) = window.max_age_days {
        let cutoff = Utc::now() - Duration::days(max_age_days as i64);
        filtered_summaries.retain(|s| s.captured_at >= cutoff);
    }

    // Sort by captured_at descending (most recent first)
    filtered_summaries.sort_by(|a, b| b.captured_at.cmp(&a.captured_at));

    // Apply session count limit
    if let Some(max_sessions) = window.max_sessions {
        filtered_summaries.truncate(max_sessions);
    }

    let session_count = filtered_summaries.len();
    let oldest_session_date = filtered_summaries.last().map(|s| s.captured_at);
    let newest_session_date = filtered_summaries.first().map(|s| s.captured_at);

    // Aggregate tool stats
    let mut tool_data = BTreeMap::<String, ToolAggregation>::new();
    let mut total_turn_count = 0usize;

    for summary in &filtered_summaries {
        for (tool_name, call_summary) in &summary.tools {
            let entry =
                tool_data.entry(tool_name.clone()).or_insert_with(|| {
                    ToolAggregation {
                        call_count: 0,
                        success_count: 0,
                        fail_count: 0,
                        timeout_count: 0,
                        hang_count: 0,
                        output_chars: Vec::new(),
                        input_chars: Vec::new(),
                        durations: Vec::new(),
                        output_source_counts: BTreeMap::new(),
                    }
                });

            entry.call_count += call_summary.call_count;
            entry.success_count += call_summary.success_count;
            entry.fail_count += call_summary.fail_count;
            entry.timeout_count += call_summary.timeout_count;
            entry.hang_count += call_summary.hang_count;
            entry.output_chars.extend(&call_summary.output_char_sizes);
            entry.input_chars.extend(&call_summary.input_char_sizes);
            entry.durations.extend(&call_summary.duration_ms_values);
            for source in &call_summary.output_source {
                *entry
                    .output_source_counts
                    .entry(source.clone())
                    .or_insert(0) += 1;
            }

            total_turn_count += call_summary.call_count as usize;
        }
    }

    // Compute per-tool statistics
    let mut tools = Vec::new();
    let cal = cost_cal.unwrap_or_default();
    for (tool_name, data) in tool_data {
        let mut sorted_output = data.output_chars.clone();
        sorted_output.sort_unstable();

        let total_output_chars: u64 = sorted_output.iter().sum();
        let mean_output_chars = if sorted_output.is_empty() {
            0.0
        } else {
            total_output_chars as f64 / sorted_output.len() as f64
        };

        let p50_output_chars = percentile(&sorted_output, 50);
        let p90_output_chars = percentile(&sorted_output, 90);
        let p95_output_chars = percentile(&sorted_output, 95);
        let max_output_chars = sorted_output.last().copied().unwrap_or(0);

        let est_mean_output_tokens =
            estimator.estimate_tokens(mean_output_chars as u64);
        let est_p90_output_tokens = estimator.estimate_tokens(p90_output_chars);

        let mean_input_chars = if data.input_chars.is_empty() {
            0.0
        } else {
            let total_input: u64 = data.input_chars.iter().sum();
            total_input as f64 / data.input_chars.len() as f64
        };

        let cost = if cost_cal.is_some() {
            Some(graded_cost(est_p90_output_tokens, &cal))
        } else {
            None
        };

        // Compute duration percentiles
        let mut sorted_durations = data.durations.clone();
        sorted_durations.sort_unstable();
        let p50_duration_ms = if sorted_durations.is_empty() {
            None
        } else {
            Some(percentile_i64(&sorted_durations, 50))
        };
        let p95_duration_ms = if sorted_durations.is_empty() {
            None
        } else {
            Some(percentile_i64(&sorted_durations, 95))
        };

        tools.push(ToolTokenStats {
            tool_name,
            call_count: data.call_count,
            success_count: data.success_count,
            fail_count: data.fail_count,
            timeout_count: data.timeout_count,
            hang_count: data.hang_count,
            total_output_chars,
            mean_output_chars,
            p50_output_chars,
            p90_output_chars,
            p95_output_chars,
            max_output_chars,
            est_mean_output_tokens,
            est_p90_output_tokens,
            mean_input_chars,
            p50_duration_ms,
            p95_duration_ms,
            cost,
            output_source_counts: data.output_source_counts,
        });
    }

    // Sort tools by est_p90_output_tokens descending
    tools.sort_by(|a, b| {
        b.est_p90_output_tokens
            .partial_cmp(&a.est_p90_output_tokens)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.tool_name.cmp(&b.tool_name))
    });

    ToolMetricsReport {
        schema_version: TOOL_METRICS_SCHEMA_VERSION,
        generated_at: Utc::now(),
        session_count,
        turn_count: total_turn_count,
        chars_per_token: match estimator as &dyn TokenEstimator {
            e if std::ptr::eq(
                e as *const _,
                &CharsPerTokenEstimator::default() as *const _,
            ) =>
                CharsPerTokenEstimator::default().chars_per_token,
            _ => 4.0, // fallback
        },
        window: ToolMetricsWindowDescription {
            max_age_days: window.max_age_days,
            max_sessions: window.max_sessions,
            actual_sessions: session_count,
            oldest_session_date,
            newest_session_date,
        },
        tools,
    }
}

#[derive(Debug, Clone)]
struct ToolAggregation {
    call_count: u64,
    success_count: u64,
    fail_count: u64,
    timeout_count: u64,
    hang_count: u64,
    output_chars: Vec<u64>,
    input_chars: Vec<u64>,
    durations: Vec<i64>,
    output_source_counts: BTreeMap<String, u64>,
}

fn percentile(
    sorted_values: &[u64],
    p: u8,
) -> u64 {
    if sorted_values.is_empty() {
        return 0;
    }
    if p >= 100 {
        return *sorted_values.last().unwrap();
    }
    // Nearest rank method: rank = ceil(p/100 * n)
    let rank = (p as f64 / 100.0 * sorted_values.len() as f64).ceil() as usize;
    sorted_values[rank.saturating_sub(1)]
}

fn percentile_i64(
    sorted_values: &[i64],
    p: u8,
) -> i64 {
    if sorted_values.is_empty() {
        return 0;
    }
    if p >= 100 {
        return *sorted_values.last().unwrap();
    }
    // Nearest rank method: rank = ceil(p/100 * n)
    let rank = (p as f64 / 100.0 * sorted_values.len() as f64).ceil() as usize;
    sorted_values[rank.saturating_sub(1)]
}

/// Write a tool metrics rollup to a file.
pub fn write_rollup(
    path: &Path,
    report: ToolMetricsReport,
) -> Result<(), SessionError> {
    use std::{
        fs,
        io::Write,
    };

    let rollup = ToolMetricsRollup {
        schema_version: TOOL_METRICS_SCHEMA_VERSION,
        report,
    };

    let json = serde_json::to_string_pretty(&rollup).map_err(|source| {
        SessionError::Serialize {
            path: path.to_path_buf(),
            source,
        }
    })?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| SessionError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut file =
        fs::File::create(path).map_err(|source| SessionError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    file.write_all(json.as_bytes())
        .map_err(|source| SessionError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    Ok(())
}

/// Aggregate tool metrics across multiple store roots.
/// Each store root is expected to contain a `sessions/` subdirectory.
/// This function is a convenience wrapper around store-level aggregation logic.
pub fn aggregate_multi_store(
    store_roots: &[std::path::PathBuf],
    window: ToolMetricsWindow,
) -> Result<ToolMetricsReport, SessionError> {
    // Import here to avoid circular dependencies
    use crate::SessionStoreConfig;

    let mut all_summaries = Vec::new();

    for store_root in store_roots {
        let config = SessionStoreConfig::new(store_root, "default");

        // Get summaries from this store using the same logic as tool_metrics()
        // but don't aggregate yet - just collect
        let sessions_root = store_root.join("sessions");

        if !sessions_root.exists() {
            continue;
        }

        for entry in std::fs::read_dir(&sessions_root).map_err(|source| {
            SessionError::Io {
                path: sessions_root.clone(),
                source,
            }
        })? {
            let entry = entry.map_err(|source| SessionError::Io {
                path: sessions_root.clone(),
                source,
            })?;

            let file_type =
                entry.file_type().map_err(|source| SessionError::Io {
                    path: entry.path(),
                    source,
                })?;

            if !file_type.is_dir() {
                continue;
            }

            let session_id = entry.file_name().to_string_lossy().into_owned();

            // Try to load a cached summary that actually holds tool data.
            let tool_metrics_path = entry.path().join("tool-metrics.json");

            if tool_metrics_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&tool_metrics_path)
                {
                    if let Ok(summary) = serde_json::from_str::<
                        SessionToolMetricsSummary,
                    >(&content)
                    {
                        if !summary.is_empty() {
                            all_summaries.push(summary);
                            continue;
                        }
                    }
                }
            }

            // Otherwise recompute from the transcript plus the event stream.
            if let Ok(record) = config.read_session(&session_id) {
                let events = std::fs::read_to_string(
                    entry.path().join("events.json"),
                )
                .ok()
                .and_then(|content| {
                    serde_json::from_str::<crate::PersistedSessionEvents>(
                        &content,
                    )
                    .ok()
                });
                let estimator = CharsPerTokenEstimator::default();
                let summary = compute_session_summary_with_events(
                    &record,
                    events
                        .as_ref()
                        .map(|events| events.events.as_slice())
                        .unwrap_or_default(),
                    &estimator,
                );
                all_summaries.push(summary);
            }
        }
    }

    let estimator = CharsPerTokenEstimator::default();
    let cal = GradedCostCalibration::default();
    Ok(aggregate_with_cost(
        all_summaries,
        window,
        &estimator,
        Some(cal),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SESSION_SCHEMA_VERSION,
        SessionMetadata,
        SessionTurn,
        SessionTurnEventMeta,
    };
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
                workspace_slug: "test".to_string(),
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
                workspace_slug: "test".to_string(),
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
        let deserialized: ToolMetricsRollup =
            serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.report.tools[0].cost, Some(25));
        assert_eq!(deserialized.report.session_count, 10);
    }

    #[test]
    fn failure_timeout_hang_classification_and_duration_tracking() {
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
                workspace_slug: "test".to_string(),
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
                        error_message: Some(
                            "Command failed with non-zero exit".to_string(),
                        ),
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
                        tool_arguments_json: Some(
                            json!({"duration_ms": 305000}),
                        ),
                        result_code: Some("timeout".to_string()),
                        subagent_run_id: None,
                        error_message: Some(
                            "Execution exceeded timeout cap".to_string(),
                        ),
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
                        tool_arguments_json: Some(
                            json!({"duration_ms": 50000}),
                        ),
                        result_code: Some("hang".to_string()),
                        subagent_run_id: None,
                        error_message: Some(
                            "sync-terminal-state-ambiguous".to_string(),
                        ),
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
        let window = ToolMetricsWindow::default();
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

    use serde_json::json;

    fn empty_record() -> SessionRecord {
        SessionRecord {
            schema_version: SESSION_SCHEMA_VERSION,
            session_id: "events-session".to_string(),
            source: "test".to_string(),
            started_at: sample_time(),
            captured_at: sample_time(),
            metadata: SessionMetadata {
                workspace_slug: "test".to_string(),
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

    #[test]
    fn computes_tool_calls_from_captured_events_when_no_tool_turns_exist() {
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
    fn does_not_double_count_complete_and_result_for_the_same_tool_call() {
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
    fn late_arriving_richer_output_copy_upgrades_the_dropped_record() {
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
    fn poorer_or_duplicate_late_copy_never_downgrades_or_double_counts() {
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
    fn summary_is_empty_when_no_tool_call_was_observed() {
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
    fn aggregate_preserves_mixed_output_source_breakdown() {
        fn make_summary(sources: Vec<&str>) -> SessionToolMetricsSummary {
            let output_char_sizes: Vec<u64> =
                sources.iter().map(|s| s.len() as u64 * 10).collect();
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
                            output_source: sources
                                .iter()
                                .map(|s| s.to_string())
                                .collect(),
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
            ToolMetricsWindow::default(),
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
}
