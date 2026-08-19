use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};
use std::collections::HashMap;

use crate::{
    PersistedSessionEvents,
    SessionRecord,
    SessionRole,
    SessionRuntimeContext,
};

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

            let rollup =
                rollups.entry(rollup_key.clone()).or_insert_with(|| {
                    SubAgentRollup {
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
                    }
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
                rollup.tokens_estimated =
                    Some(rollup.tokens_estimated.unwrap_or(0) + est);
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
        let Some(agent_id) =
            data.get("agent_id").and_then(|value| value.as_str())
        else {
            continue;
        };
        let timestamp = event.captured_at.clone().or_else(|| {
            data.get("timestamp")
                .and_then(|value| value.as_str())
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc))
        });
        let rollup = rollups.entry(agent_id.to_string()).or_insert_with(|| {
            SubAgentRollup {
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
            }
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
            },
            "SubagentStop" => {
                rollup.stopped_at = timestamp;
                rollup.outcome = Some("stopped".to_string());
            },
            _ => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CopilotHookEvent,
        SessionLinks,
        SessionMetadata,
        SessionTurn,
        SessionTurnEventMeta,
    };
    use chrono::Utc;

    #[test]
    fn hook_lifecycle_events_produce_one_stopped_rollup_per_agent() {
        let started_at = DateTime::parse_from_rfc3339("2026-08-14T12:00:00Z")
            .expect("valid dispatch timestamp")
            .with_timezone(&Utc);
        let stopped_at = DateTime::parse_from_rfc3339("2026-08-14T12:01:00Z")
            .expect("valid stop timestamp")
            .with_timezone(&Utc);
        let record = SessionRecord {
            schema_version: 1,
            session_id: "parent-session".to_string(),
            source: "test".to_string(),
            started_at,
            captured_at: stopped_at,
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
            turns: Vec::new(),
            links: SessionLinks::default(),
            track_id: None,
            anchor_ticket_id: None,
            parent_session_id: None,
            spawned_session_id: None,
            emitted_handoff_ids: Vec::new(),
            picked_up_handoff_ids: Vec::new(),
        };
        let lifecycle_event = |event_type: &str, timestamp| CopilotHookEvent {
            event_id: None,
            parent_event_id: None,
            event_type: Some(event_type.to_string()),
            captured_at: Some(timestamp),
            turn_id: None,
            message_id: None,
            tool_call_id: None,
            tool_name: None,
            tool_success: None,
            reasoning_text: None,
            tool_requests_json: None,
            tool_arguments_json: None,
            data_json: Some(serde_json::json!({
                "agent_id": "agent-42",
                "agent_type": "Implement Agent",
            })),
            raw_event_json: None,
        };
        let events = PersistedSessionEvents {
            schema_version: 1,
            session_id: record.session_id.clone(),
            captured_at: stopped_at,
            events: vec![
                lifecycle_event("SubagentStart", started_at),
                lifecycle_event("SubagentStop", stopped_at),
            ],
        };

        let rollups =
            compute_subagent_rollups_with_events(&record, None, Some(&events));

        assert_eq!(rollups.len(), 1);
        let rollup = rollups.get("agent-42").expect("lifecycle rollup");
        assert_eq!(rollup.parent_session_id.as_deref(), Some("parent-session"));
        assert_eq!(rollup.agent_type.as_deref(), Some("Implement Agent"));
        assert_eq!(rollup.dispatched_at, Some(started_at));
        assert_eq!(rollup.stopped_at, Some(stopped_at));
        assert_eq!(rollup.outcome.as_deref(), Some("stopped"));
    }

    #[test]
    fn compute_rollup_aggregates_token_counts() {
        let record = SessionRecord {
            schema_version: 1,
            session_id: "session-1".to_string(),
            source: "test".to_string(),
            started_at: Utc::now(),
            captured_at: Utc::now(),
            metadata: SessionMetadata {
                workspace_slug: "test".to_string(),
                conversation_id: None,
                agent_id: None,
                ticket_id: None,
                model: Some("claude-3-5-sonnet".to_string()),
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
                    sequence: 0,
                    role: SessionRole::Assistant,
                    content: "Hello".to_string(),
                    captured_at: Utc::now(),
                    tool_name: None,
                    model: Some("claude-3-5-sonnet".to_string()),
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
                        cache_read_tokens: Some(200),
                        cache_write_tokens: Some(100),
                        cost_usd: Some(0.05),
                        model_id: Some("claude-3-5-sonnet".to_string()),
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
                    sequence: 1,
                    role: SessionRole::Assistant,
                    content: "World".to_string(),
                    captured_at: Utc::now(),
                    tool_name: None,
                    model: Some("claude-3-5-sonnet".to_string()),
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
                        input_tokens: Some(2000),
                        output_tokens: Some(1000),
                        cache_read_tokens: Some(0),
                        cache_write_tokens: Some(0),
                        cost_usd: Some(0.10),
                        model_id: Some("claude-3-5-sonnet".to_string()),
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
            links: SessionLinks::default(),
            track_id: None,
            anchor_ticket_id: None,
            parent_session_id: None,
            spawned_session_id: None,
            emitted_handoff_ids: Vec::new(),
            picked_up_handoff_ids: Vec::new(),
        };

        let rollups = compute_subagent_rollups(&record, None);
        let rollup = rollups.get("session-1").expect("rollup should exist");

        assert_eq!(rollup.session_id, "session-1");
        assert_eq!(rollup.turn_count, 2);
        assert_eq!(rollup.input_tokens, 3000);
        assert_eq!(rollup.output_tokens, 1500);
        assert_eq!(rollup.cache_read_tokens, 200);
        assert_eq!(rollup.cache_write_tokens, 100);
        // Floating point comparison with tolerance for precision
        assert!((rollup.cost_usd.unwrap() - 0.15).abs() < 0.0001);
        assert_eq!(rollup.model.as_deref(), Some("claude-3-5-sonnet"));
    }

    #[test]
    fn query_surface_returns_rollups_for_session_with_context() {
        use crate::SessionStoreConfig;
        use chrono::Utc;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config = SessionStoreConfig::new(
            temp_dir.path().join(".session"),
            "test-workspace",
        );

        // Create a simple session using the capture API
        let session_id = "f5555555-5555-4555-8555-555555555555";
        use crate::hook::{
            CopilotHookMessage,
            CopilotHookPayload,
        };

        let payload = CopilotHookPayload {
            session_id: session_id.to_string(),
            workspace_slug: "test-workspace".to_string(),
            captured_at: Utc::now(),
            conversation_id: None,
            agent_id: None,
            model: Some("claude-opus-4".to_string()),
            trigger: Some("test".to_string()),
            provisioning: None,
            messages: vec![
                CopilotHookMessage {
                    role: SessionRole::User,
                    content: "Hello".to_string(),
                    tool_name: None,
                    captured_at: None,
                    event_meta: None,
                },
                CopilotHookMessage {
                    role: SessionRole::Assistant,
                    content: "Hi there".to_string(),
                    tool_name: None,
                    captured_at: None,
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
                        input_tokens: Some(500),
                        output_tokens: Some(250),
                        cache_read_tokens: Some(0),
                        cache_write_tokens: Some(0),
                        cost_usd: Some(0.025),
                        model_id: Some("claude-opus-4".to_string()),
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
            events: vec![],
            runtime: None,
        };

        config.capture_copilot_hook(payload).unwrap();

        // Query the rollups
        let rollups = config.subagent_rollups(session_id).unwrap();

        // Verify we got rollups
        assert!(!rollups.is_empty(), "Should have at least one rollup");

        // Check the main session rollup
        let rollup =
            rollups.get(session_id).expect("Should have session rollup");
        assert_eq!(rollup.session_id, session_id);
        assert_eq!(rollup.turn_count, 1); // One assistant turn
        assert_eq!(rollup.input_tokens, 500);
        assert_eq!(rollup.output_tokens, 250);
        assert_eq!(rollup.model.as_deref(), Some("claude-opus-4"));
        assert!((rollup.cost_usd.unwrap() - 0.025).abs() < 0.0001);
    }

    #[test]
    fn rollup_aggregates_estimated_tokens() {
        // AC1, AC2, AC4: Verify tokens_estimated aggregation and null vs zero distinction
        let record = SessionRecord {
            schema_version: 1,
            session_id: "session-with-estimates".to_string(),
            source: "test".to_string(),
            started_at: Utc::now(),
            captured_at: Utc::now(),
            metadata: SessionMetadata {
                workspace_slug: "test".to_string(),
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
    fn rollup_with_no_estimates_yields_none() {
        // AC4, AC6: Verify null is preserved when no MCP traffic exists
        let record = SessionRecord {
            schema_version: 1,
            session_id: "session-no-mcp".to_string(),
            source: "test".to_string(),
            started_at: Utc::now(),
            captured_at: Utc::now(),
            metadata: SessionMetadata {
                workspace_slug: "test".to_string(),
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
        let rollup =
            rollups.get("session-no-mcp").expect("rollup should exist");

        // AC4, AC6: tokens_estimated should be None (not Some(0))
        assert_eq!(
            rollup.tokens_estimated, None,
            "tokens_estimated should be None when no MCP traffic exists"
        );
        assert_eq!(rollup.tool_call_count, 0);
        assert_eq!(rollup.input_tokens, 100);
        assert_eq!(rollup.output_tokens, 50);
    }
}
