use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

use crate::{CopilotHookEvent, SessionError, SessionRecord, SessionRole};

/// Trait for estimating token counts from character counts.
pub trait TokenEstimator {
    fn estimate_tokens(&self, chars: u64) -> f64;
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
    fn estimate_tokens(&self, chars: u64) -> f64 {
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
pub fn graded_cost(est_tokens: f64, cal: &GradedCostCalibration) -> u32 {
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
    fn classify(&mut self, result_code: Option<&str>, tool_success: Option<bool>) -> bool {
        match result_code {
            Some("ok") => {
                self.success_count += 1;
                true
            }
            Some("error") => {
                self.fail_count += 1;
                false
            }
            Some("timeout") => {
                self.timeout_count += 1;
                false
            }
            Some("hang") => {
                self.hang_count += 1;
                false
            }
            _ => {
                let success = tool_success != Some(false);
                if success {
                    self.success_count += 1;
                } else {
                    self.fail_count += 1;
                }
                success
            }
        }
    }
}

const TOOL_METRICS_SCHEMA_VERSION: u32 = 1;

/// Fidelity rank of an `output_source` value, matching the layering
/// documented on [`crate::hook::transcript::ToolResponseOverride`]:
/// `hook_payload` (highest fidelity) > `spill_file` > `transcript_turn` >
/// any unrecognized/`unspecified` source. Used by [`record_event_tool_call`]
/// to decide whether a later copy of an already-counted `tool_call_id` may
/// upgrade the recorded output size, never downgrade it.
const OUTPUT_SOURCE_RANK_NONE: i8 = -1;

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

#[cfg(test)]
#[path = "tool_metrics/tests.rs"]
mod tests;

#[path = "tool_metrics/summary.rs"]
mod summary;
#[allow(unused_imports)]
pub use summary::*;

#[path = "tool_metrics/aggregation.rs"]
mod aggregation;
#[allow(unused_imports)]
pub use aggregation::*;
