use chrono::TimeZone;
use pretty_assertions::assert_eq;
use std::path::PathBuf;

use super::{
    SessionLinks, SessionMetadata, SessionRecord, SessionRole, SessionTurn, SessionTurnEventMeta,
    SessionWorktreeAllocationMode, SessionWorktreeAssignment, SessionWorktreeStatus,
};
use crate::SESSION_SCHEMA_VERSION;

fn sample_time() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc
        .with_ymd_and_hms(2026, 6, 2, 12, 0, 0)
        .single()
        .unwrap()
}

#[test]
fn session_record_round_trips_through_serde() {
    let record = SessionRecord {
        schema_version: SESSION_SCHEMA_VERSION,
        session_id: "session-123".to_string(),
        source: "copilot-hook".to_string(),
        started_at: sample_time(),
        captured_at: sample_time(),
        metadata: SessionMetadata {
            workspace_path: "context-engine".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            agent_id: Some("github-copilot-gpt-5.4".to_string()),
            ticket_id: Some("ticket-1".to_string()),
            model: Some("GPT-5.4".to_string()),
            trigger: Some("post-turn".to_string()),
            provisioning: None,
            producer: Some("copilot-agent".to_string()),
            copilot_version: Some("0.55.0".to_string()),
            vscode_version: Some("1.127.0".to_string()),
            protocol_version: Some(1),
            worktree: Some(SessionWorktreeAssignment {
                path: PathBuf::from("worktrees/session-123"),
                branch: "session/session-123".to_string(),
                allocation_mode: SessionWorktreeAllocationMode::New,
                status: SessionWorktreeStatus::Active,
                predecessor_session_id: None,
                predecessor_path: None,
            }),
        },
        turns: vec![SessionTurn {
            sequence: 0,
            role: SessionRole::User,
            content: "Summarize the test failures".to_string(),
            captured_at: sample_time(),
            tool_name: None,
            model: Some("GPT-5.4".to_string()),
            event_meta: Some(SessionTurnEventMeta {
                event_id: Some("evt-1".to_string()),
                parent_event_id: None,
                event_type: Some("user.message".to_string()),
                turn_id: Some("turn-1".to_string()),
                message_id: Some("msg-1".to_string()),
                tool_call_id: None,
                tool_success: None,
                reasoning_text: None,
                tool_requests_json: None,
                tool_arguments_json: None,
                // Token and cost attribution (ticket 6549b6a7)
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                cache_write_tokens: None,
                cost_usd: None,
                model_id: None,
                // Payload telemetry (ticket 9d527ad1)
                request_bytes: None,
                request_chars: None,
                response_bytes: None,
                response_chars: None,
                tokens_estimated: None,
                // Tool execution failure/timeout classification (ticket 84c7757d)
                error_message: None,
                exit_code: None,
                result_code: None,
                subagent_run_id: None,
            }),
        }],
        links: SessionLinks {
            ticket_ids: vec!["ticket-1".to_string()],
            spec_ids: vec!["spec-1".to_string()],
            doc_evidence_ids: vec!["doc-1".to_string()],
            log_ids: vec!["log-1".to_string()],
            runtime_session_id: Some("03baab6c-0fdb-4ffc-8159-b83066a6283f".to_string()),
            runtime_run_id: Some("8cf1255d-7969-4ac2-905a-cbd234dc3eac".to_string()),
        },
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: Vec::new(),
        picked_up_handoff_ids: Vec::new(),
    };

    let json = serde_json::to_string_pretty(&record).unwrap();
    let reparsed: SessionRecord = serde_json::from_str(&json).unwrap();

    assert_eq!(reparsed, record);
    assert_eq!(
        record.turns[0].model.as_deref(),
        Some("GPT-5.4"),
        "per-turn model should round-trip"
    );
    assert!(record.links.links_to_ticket("ticket-1"));
    assert!(record.links.links_to_spec("spec-1"));
}

#[test]
fn existing_sessions_deserialize_without_track_fields() {
    // Real on-disk session format from pre-track schema (AC2 verification)
    let legacy_json = r#"{
            "schema_version": 1,
            "session_id": "01246bdc-dd6b-4807-bda3-38cf6aa780de",
            "source": "copilot-hook",
            "started_at": "2026-07-25T18:57:59.736Z",
            "captured_at": "2026-07-26T01:23:11.736Z",
            "metadata": {
                "workspace_path": "default",
                "agent_id": "copilot-agent",
                "trigger": "Stop",
                "producer": "copilot-agent",
                "copilot_version": "0.58.0",
                "vscode_version": "1.130.0",
                "protocol_version": 1
            },
            "links": {}
        }"#;

    let record: SessionRecord = serde_json::from_str(legacy_json).unwrap();

    // All four track fields must deserialize to None
    assert_eq!(
        record.track_id, None,
        "track_id must be None for legacy sessions"
    );
    assert_eq!(
        record.anchor_ticket_id, None,
        "anchor_ticket_id must be None for legacy sessions"
    );
    assert_eq!(
        record.parent_session_id, None,
        "parent_session_id must be None for legacy sessions"
    );
    assert_eq!(
        record.spawned_session_id, None,
        "spawned_session_id must be None for legacy sessions"
    );

    // Round-trip must not emit the track fields
    let reserialized = serde_json::to_string(&record).unwrap();
    assert!(
        !reserialized.contains("track_id"),
        "track_id must not appear in JSON when None"
    );
    assert!(
        !reserialized.contains("anchor_ticket_id"),
        "anchor_ticket_id must not appear in JSON when None"
    );
    assert!(
        !reserialized.contains("parent_session_id"),
        "parent_session_id must not appear in JSON when None"
    );
    assert!(
        !reserialized.contains("spawned_session_id"),
        "spawned_session_id must not appear in JSON when None"
    );
}

#[test]
fn existing_sessions_deserialize_without_provisioning_metadata() {
    let legacy_json = r#"{"schema_version":1,"session_id":"93d9261e-93e8-4ae3-9f4c-02c17e7c8568","source":"copilot-hook","started_at":"2026-08-10T17:57:22.617Z","captured_at":"2026-08-10T18:53:14.528Z","metadata":{"workspace_path":"default","agent_id":"copilot-agent","trigger":"Stop","producer":"copilot-agent","copilot_version":"0.60.0","vscode_version":"1.132.0","protocol_version":1,"worktree":{"path":"C:/Users/linus/git/context-engine","branch":"main","allocation_mode":"new","status":"active"}},"links":{}}"#;

    let record: SessionRecord = serde_json::from_str(legacy_json).unwrap();

    assert_eq!(record.metadata.provisioning, None);
}
