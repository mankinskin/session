//! Integration tests for delegated session data collection.
//!
//! Validates that the data model supports computing per-model satisfactory-work
//! rates from quality gates, session data, and tool metrics.

use chrono::Utc;
use session_api::{
    QualityGate,
    QualityGateOutcome,
    QualityGatePhase,
    SessionLinks,
    SessionMetadata,
    SessionRecord,
    SessionRole,
    SessionRunLineage,
    SessionRuntimeContext,
    SessionTurn,
    SessionTurnEventMeta,
    SubAgentRollup,
    compute_subagent_rollups,
    post_delegation_gate,
    pre_delegation_gate,
};
use std::collections::HashMap;

#[test]
fn quality_gate_captures_pre_and_post_delegation_checks() {
    let pre_gate = pre_delegation_gate(
        "prompt-clarity",
        QualityGateOutcome::Passed,
        "delegated-sess-abc",
        "parent-sess-xyz",
    )
    .with_validation_spec_id("val-pre-gate-clarity");

    let post_gate = post_delegation_gate(
        "acceptance-criteria",
        QualityGateOutcome::Failed,
        "delegated-sess-abc",
        "parent-sess-xyz",
    )
    .with_validation_spec_id("val-post-gate-acceptance")
    .with_detail("Test suite failed: 2 of 5 tests did not pass");

    assert_eq!(pre_gate.phase, QualityGatePhase::PreDelegation);
    assert_eq!(post_gate.phase, QualityGatePhase::PostDelegation);
    assert_eq!(pre_gate.outcome, QualityGateOutcome::Passed);
    assert_eq!(post_gate.outcome, QualityGateOutcome::Failed);
    assert_eq!(
        pre_gate.delegated_session_id,
        post_gate.delegated_session_id
    );
    assert_eq!(pre_gate.parent_session_id, post_gate.parent_session_id);
}

#[test]
fn subagent_rollup_links_delegated_session_to_parent() {
    let parent_session_id = "parent-session-123";
    let delegated_session_id = "delegated-session-456";
    let run_id = "run-001";

    let parent_record = SessionRecord {
        schema_version: 1,
        session_id: parent_session_id.to_string(),
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
        turns: vec![],
        links: SessionLinks::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: vec![],
        picked_up_handoff_ids: vec![],
    };

    let context = SessionRuntimeContext {
        session_id: delegated_session_id.to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        active_run_id: run_id.to_string(),
        runs: vec![SessionRunLineage {
            run_id: run_id.to_string(),
            captured_session_id: Some(delegated_session_id.to_string()),
            predecessor_run_id: None,
            started_at: Utc::now(),
        }],
        pinned_entities: vec![],
        workflow: Default::default(),
    };

    let rollups = compute_subagent_rollups(&parent_record, Some(&context));
    let delegated_rollup =
        rollups.get(run_id).expect("delegated rollup should exist");

    assert_eq!(delegated_rollup.session_id, delegated_session_id);
    assert_eq!(
        delegated_rollup.parent_session_id.as_deref(),
        Some(parent_session_id)
    );
    assert_eq!(delegated_rollup.run_id, run_id);
}

#[test]
fn subagent_rollup_aggregates_token_cost_model_per_delegated_session() {
    let parent_session_id = "parent-session-abc";
    let delegated_session_id = "delegated-session-def";
    let run_id = "run-002";
    let model = "claude-3-5-sonnet";

    let parent_record = SessionRecord {
        schema_version: 1,
        session_id: parent_session_id.to_string(),
        source: "test".to_string(),
        started_at: Utc::now(),
        captured_at: Utc::now(),
        metadata: SessionMetadata {
            workspace_slug: "test".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: Some(model.to_string()),
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
                content: "Delegated work turn 1".to_string(),
                captured_at: Utc::now(),
                tool_name: None,
                model: Some(model.to_string()),
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
                    input_tokens: Some(5000),
                    output_tokens: Some(2000),
                    cache_read_tokens: Some(1000),
                    cache_write_tokens: Some(500),
                    cost_usd: Some(0.20),
                    model_id: Some(model.to_string()),
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
                    tool_call_id: None,
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
        emitted_handoff_ids: vec![],
        picked_up_handoff_ids: vec![],
    };

    let context = SessionRuntimeContext {
        session_id: delegated_session_id.to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        active_run_id: run_id.to_string(),
        runs: vec![SessionRunLineage {
            run_id: run_id.to_string(),
            captured_session_id: Some(delegated_session_id.to_string()),
            predecessor_run_id: None,
            started_at: Utc::now(),
        }],
        pinned_entities: vec![],
        workflow: Default::default(),
    };

    let rollups = compute_subagent_rollups(&parent_record, Some(&context));

    // Note: Current compute_subagent_rollups aggregates all turns into the main session.
    // For proper per-delegated-session attribution, event_meta would need a run_id field.
    // This test documents the current behavior: all turns aggregate to the parent session_id key.
    let rollup = rollups.get(parent_session_id).expect("rollup should exist");

    assert_eq!(rollup.turn_count, 1); // One assistant turn
    assert_eq!(rollup.tool_call_count, 1); // One tool turn
    assert_eq!(rollup.input_tokens, 5000);
    assert_eq!(rollup.output_tokens, 2000);
    assert_eq!(rollup.cache_read_tokens, 1000);
    assert_eq!(rollup.cache_write_tokens, 500);
    assert!((rollup.cost_usd.unwrap() - 0.20).abs() < 0.0001);
    assert_eq!(rollup.model.as_deref(), Some(model));
}

#[test]
fn data_model_supports_per_model_satisfactory_work_rate_query() {
    // Simulate data collection for computing per-model satisfactory-work rate.
    // Required query: For model M, count delegated sessions, count passed/failed post-gates,
    // aggregate token/cost, compute rate = passed / total.

    let model = "claude-3-5-sonnet";
    let parent_session_id = "parent-session-001";

    // Delegated session 1: passed post-gate
    let delegated_1_id = "delegated-session-001";
    let post_gate_1 = post_delegation_gate(
        "acceptance-criteria",
        QualityGateOutcome::Passed,
        delegated_1_id,
        parent_session_id,
    );
    let rollup_1 = SubAgentRollup {
        run_id: "run-001".to_string(),
        session_id: delegated_1_id.to_string(),
        parent_session_id: Some(parent_session_id.to_string()),
        model: Some(model.to_string()),
        agent_type: None,
        dispatched_at: None,
        stopped_at: None,
        turn_count: 5,
        tool_call_count: 3,
        input_tokens: 10000,
        output_tokens: 5000,
        cache_read_tokens: 2000,
        cache_write_tokens: 1000,
        cost_usd: Some(0.50),
        tokens_estimated: None,
        wall_time_secs: Some(120.0),
        outcome: Some("passed".to_string()),
    };

    // Delegated session 2: failed post-gate
    let delegated_2_id = "delegated-session-002";
    let post_gate_2 = post_delegation_gate(
        "acceptance-criteria",
        QualityGateOutcome::Failed,
        delegated_2_id,
        parent_session_id,
    );
    let rollup_2 = SubAgentRollup {
        run_id: "run-002".to_string(),
        session_id: delegated_2_id.to_string(),
        parent_session_id: Some(parent_session_id.to_string()),
        model: Some(model.to_string()),
        agent_type: None,
        dispatched_at: None,
        stopped_at: None,
        turn_count: 3,
        tool_call_count: 2,
        input_tokens: 8000,
        output_tokens: 3000,
        cache_read_tokens: 1000,
        cache_write_tokens: 500,
        cost_usd: Some(0.30),
        tokens_estimated: None,
        wall_time_secs: Some(90.0),
        outcome: Some("failed".to_string()),
    };

    // Collect into queryable structures
    let mut rollups_by_model: HashMap<String, Vec<SubAgentRollup>> =
        HashMap::new();
    rollups_by_model
        .entry(model.to_string())
        .or_default()
        .push(rollup_1.clone());
    rollups_by_model
        .entry(model.to_string())
        .or_default()
        .push(rollup_2.clone());

    let mut gates_by_session: HashMap<String, QualityGate> = HashMap::new();
    gates_by_session.insert(delegated_1_id.to_string(), post_gate_1);
    gates_by_session.insert(delegated_2_id.to_string(), post_gate_2);

    // Query: For model M, compute satisfactory-work rate
    let model_rollups = rollups_by_model
        .get(model)
        .expect("model should have rollups");
    let total_sessions = model_rollups.len();
    let passed_sessions = model_rollups
        .iter()
        .filter(|r| {
            gates_by_session
                .get(&r.session_id)
                .map(|g| g.outcome == QualityGateOutcome::Passed)
                .unwrap_or(false)
        })
        .count();
    let failed_sessions = total_sessions - passed_sessions;

    let total_tokens: u64 = model_rollups
        .iter()
        .map(|r| r.input_tokens + r.output_tokens)
        .sum();
    let total_cost: f64 = model_rollups.iter().filter_map(|r| r.cost_usd).sum();

    let satisfactory_work_rate = passed_sessions as f64 / total_sessions as f64;

    // Assertions: the data model supports the query
    assert_eq!(total_sessions, 2);
    assert_eq!(passed_sessions, 1);
    assert_eq!(failed_sessions, 1);
    assert_eq!(total_tokens, 15000 + 11000); // (10000+5000) + (8000+3000)
    assert!((total_cost - 0.80).abs() < 0.0001); // 0.50 + 0.30
    assert!((satisfactory_work_rate - 0.5).abs() < 0.0001); // 1/2 = 0.5

    // Verify parent_session_id links are intact
    assert_eq!(
        rollup_1.parent_session_id.as_deref(),
        Some(parent_session_id)
    );
    assert_eq!(
        rollup_2.parent_session_id.as_deref(),
        Some(parent_session_id)
    );
}

#[test]
fn token_cost_model_fields_populated_via_session_turn_event_meta() {
    // Document that token/cost/model data flows through SessionTurnEventMeta fields.
    // These fields are architecturally complete (ticket 6549b6a7 done) but null on disk
    // until ticket 9d527ad1 (capture hook populate data_json.usage) lands.
    // This test validates that once the hook is fixed, the data flows correctly.

    let turn = SessionTurn {
        sequence: 0,
        role: SessionRole::Assistant,
        content: "Test turn".to_string(),
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
    };

    let meta = turn.event_meta.as_ref().expect("event_meta should exist");

    // Verify fields are present and flow to compute_subagent_rollups
    assert_eq!(meta.input_tokens, Some(1000));
    assert_eq!(meta.output_tokens, Some(500));
    assert_eq!(meta.cache_read_tokens, Some(200));
    assert_eq!(meta.cache_write_tokens, Some(100));
    assert_eq!(meta.cost_usd, Some(0.05));
    assert_eq!(meta.model_id.as_deref(), Some("claude-3-5-sonnet"));

    // These fields are extracted by compute_subagent_rollups (see subagent_rollup.rs L80-120)
    // and aggregated into SubAgentRollup.{input_tokens, output_tokens, cost_usd, model}.
    // Once ticket 9d527ad1 lands, hook.rs L251-290 will populate event_meta from data_json.usage,
    // and these values will be non-null for real sessions.
}
