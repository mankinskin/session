use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{PersistedSessionEvents, SessionRecord, SessionRole, SessionRuntimeContext};

/// Per-sub-agent cost and usage rollup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubAgentRollup {
    pub run_id: String,
    pub session_id: String,
    /// The parent session id that spawned this delegated session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatched_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<DateTime<Utc>>,
    pub turn_count: usize,
    pub tool_call_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Estimated token load from payload sizes (MCP tool calls) — ticket 9d527ad1.
    /// Null means no estimation available; 0 means measured as zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_estimated: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// Compute sub-agent rollups from a session and its runtime context.
/// Returns a map keyed by run_id.
pub fn compute_subagent_rollups(
    record: &SessionRecord,
    context: Option<&SessionRuntimeContext>,
) -> HashMap<String, SubAgentRollup> {
    compute_subagent_rollups_with_events(record, context, None)
}

/// Compute sub-agent rollups, enriching transcript spans with lifecycle
/// records captured by the SubagentStart and SubagentStop hooks.
pub fn compute_subagent_rollups_with_events(
    record: &SessionRecord,
    context: Option<&SessionRuntimeContext>,
    events: Option<&PersistedSessionEvents>,
) -> HashMap<String, SubAgentRollup> {
    let mut rollups = compute_rollup_metrics(record, context);
    if let Some(events) = events {
        add_hook_lifecycle_rollups(&mut rollups, record, events);
    }
    rollups
}

fn compute_rollup_metrics(
    record: &SessionRecord,
    context: Option<&SessionRuntimeContext>,
) -> HashMap<String, SubAgentRollup> {
    let mut rollups: HashMap<String, SubAgentRollup> = HashMap::new();

    // If there's a runtime context, initialize rollups for all runs
    if let Some(ctx) = context {
        for run in &ctx.runs {
            if let Some(session_id) = &run.captured_session_id {
                rollups.insert(
                    run.run_id.clone(),
                    SubAgentRollup {
                        run_id: run.run_id.clone(),
                        session_id: session_id.clone(),
                        parent_session_id: Some(record.session_id.clone()),
                        model: None,
                        agent_type: None,
                        dispatched_at: None,
                        stopped_at: None,
                        turn_count: 0,
                        tool_call_count: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                        cost_usd: None,
                        tokens_estimated: None,
                        wall_time_secs: None,
                        outcome: None,
                    },
                );
            }
        }
    }

    // Aggregate token/cost data from turns with event_meta
    for turn in &record.turns {
        if let Some(meta) = &turn.event_meta {
            // Extract model_id from event_meta or turn.model
            let model_id = meta.model_id.as_ref().or(turn.model.as_ref());

            // Count this turn if it's an assistant turn
            let is_assistant = turn.role == SessionRole::Assistant;

            // Count tool calls (tool turns)
            let is_tool_call = turn.role == SessionRole::Tool;

            // Aggregate tokens
            let input = meta.input_tokens.unwrap_or(0);
            let output = meta.output_tokens.unwrap_or(0);
            let cache_read = meta.cache_read_tokens.unwrap_or(0);
            let cache_write = meta.cache_write_tokens.unwrap_or(0);

            // Real per-sub-agent attribution (ticket b7c61f0e): when the
            // capture hook has resolved this turn's owning `runSubagent`
            // span via `parent_event_id` ancestry, group into that span's
            // own rollup instead of lumping every turn into the parent
            // session's bucket. Turns without a resolved span (top-level
            // orchestrator turns, or transcripts captured before this
            // attribution existed) keep the prior fallback behavior of
            // aggregating into the parent session's own key.
            let rollup_key = meta
                .subagent_run_id
                .clone()
                .unwrap_or_else(|| record.session_id.clone());
            let is_inline_subagent_span = rollup_key != record.session_id;

            let rollup = rollups
                .entry(rollup_key.clone())
                .or_insert_with(|| SubAgentRollup {
                    run_id: rollup_key.clone(),
                    session_id: record.session_id.clone(),
                    parent_session_id: if is_inline_subagent_span {
                        Some(record.session_id.clone())
                    } else {
                        None
                    },
                    model: None,
                    agent_type: None,
                    dispatched_at: None,
                    stopped_at: None,
                    turn_count: 0,
                    tool_call_count: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    cost_usd: None,
                    tokens_estimated: None,
                    outcome: None,
                    wall_time_secs: None,
                });

            if is_assistant {
                rollup.turn_count += 1;
            }

            if is_tool_call {
                rollup.tool_call_count += 1;
            }

            rollup.input_tokens += input;
            rollup.output_tokens += output;
            rollup.cache_read_tokens += cache_read;
            rollup.cache_write_tokens += cache_write;

            // Set model if we have one
            if rollup.model.is_none() && model_id.is_some() {
                rollup.model = model_id.cloned();
            }

            // Aggregate cost
            if let Some(cost) = meta.cost_usd {
                rollup.cost_usd = Some(rollup.cost_usd.unwrap_or(0.0) + cost);
            }

            // Aggregate estimated tokens from payload sizes (ticket 9d527ad1)
            if let Some(est) = meta.tokens_estimated {
                rollup.tokens_estimated = Some(rollup.tokens_estimated.unwrap_or(0) + est);
            }
        }
    }

    // Compute wall time from context if available
    if let Some(ctx) = context {
        for run in &ctx.runs {
            if let Some(rollup) = rollups.get_mut(&run.run_id) {
                // Wall time computation would require end time in SessionRunLineage
                // For now, leave as None - this can be enhanced in a follow-up
                rollup.wall_time_secs = None;
            }
        }
    }

    rollups
}

fn add_hook_lifecycle_rollups(
    rollups: &mut HashMap<String, SubAgentRollup>,
    record: &SessionRecord,
    events: &PersistedSessionEvents,
) {
    for event in &events.events {
        let Some(event_type) = event.event_type.as_deref() else {
            continue;
        };
        if !matches!(event_type, "SubagentStart" | "SubagentStop") {
            continue;
        }
        let Some(data) = event.data_json.as_ref() else {
            continue;
        };
        let Some(agent_id) = data.get("agent_id").and_then(|value| value.as_str()) else {
            continue;
        };
        let timestamp = event.captured_at.clone().or_else(|| {
            data.get("timestamp")
                .and_then(|value| value.as_str())
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc))
        });
        let rollup = rollups
            .entry(agent_id.to_string())
            .or_insert_with(|| SubAgentRollup {
                run_id: agent_id.to_string(),
                session_id: record.session_id.clone(),
                parent_session_id: Some(record.session_id.clone()),
                model: None,
                agent_type: None,
                dispatched_at: None,
                stopped_at: None,
                turn_count: 0,
                tool_call_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: None,
                tokens_estimated: None,
                wall_time_secs: None,
                outcome: None,
            });
        if rollup.agent_type.is_none() {
            rollup.agent_type = data
                .get("agent_type")
                .and_then(|value| value.as_str())
                .map(ToString::to_string);
        }
        match event_type {
            "SubagentStart" => {
                rollup.dispatched_at = timestamp;
                rollup.outcome = Some("running".to_string());
            }
            "SubagentStop" => {
                rollup.stopped_at = timestamp;
                rollup.outcome = Some("stopped".to_string());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "subagent_rollup/tests.rs"]
mod tests;
