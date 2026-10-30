#[allow(unused_imports)]
use super::*;

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
            let entry = tool_data
                .entry(tool_name.clone())
                .or_insert_with(|| ToolAggregation {
                    call_count: 0,
                    success_count: 0,
                    fail_count: 0,
                    timeout_count: 0,
                    hang_count: 0,
                    output_chars: Vec::new(),
                    input_chars: Vec::new(),
                    durations: Vec::new(),
                    output_source_counts: BTreeMap::new(),
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

        let est_mean_output_tokens = estimator.estimate_tokens(mean_output_chars as u64);
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
            {
                CharsPerTokenEstimator::default().chars_per_token
            }
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

pub(super) fn percentile(sorted_values: &[u64], p: u8) -> u64 {
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

pub(super) fn percentile_i64(sorted_values: &[i64], p: u8) -> i64 {
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
pub fn write_rollup(path: &Path, report: ToolMetricsReport) -> Result<(), SessionError> {
    use std::{fs, io::Write};

    let rollup = ToolMetricsRollup {
        schema_version: TOOL_METRICS_SCHEMA_VERSION,
        report,
    };

    let json = serde_json::to_string_pretty(&rollup).map_err(|source| SessionError::Serialize {
        path: path.to_path_buf(),
        source,
    })?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| SessionError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut file = fs::File::create(path).map_err(|source| SessionError::Io {
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
        let config = SessionStoreConfig::new(store_root);

        // Get summaries from this store using the same logic as tool_metrics()
        // but don't aggregate yet - just collect
        let sessions_root = store_root.join("sessions");

        if !sessions_root.exists() {
            continue;
        }

        for entry in std::fs::read_dir(&sessions_root).map_err(|source| SessionError::Io {
            path: sessions_root.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| SessionError::Io {
                path: sessions_root.clone(),
                source,
            })?;

            let file_type = entry.file_type().map_err(|source| SessionError::Io {
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
                if let Ok(content) = std::fs::read_to_string(&tool_metrics_path) {
                    if let Ok(summary) = serde_json::from_str::<SessionToolMetricsSummary>(&content)
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
                let events = std::fs::read_to_string(entry.path().join("events.json"))
                    .ok()
                    .and_then(|content| {
                        serde_json::from_str::<crate::PersistedSessionEvents>(&content).ok()
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
