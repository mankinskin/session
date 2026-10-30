use chrono::TimeZone;
use session_api::{
    SessionWorktreeAllocationMode, SessionWorktreeAssignment, SessionWorktreeStatus,
};

use super::*;

fn time(hour: u32, minute: u32, second: u32) -> DateTimeUtc {
    chrono::Utc
        .with_ymd_and_hms(2026, 8, 19, hour, minute, second)
        .single()
        .unwrap()
}

type DateTimeUtc = chrono::DateTime<chrono::Utc>;

fn stub_manifest(captured_at: DateTimeUtc) -> PersistedSessionManifest {
    PersistedSessionManifest {
        schema_version: 1,
        session_id: "s-1".to_string(),
        source: "session-worktree-inference".to_string(),
        started_at: captured_at,
        captured_at,
        metadata: SessionMetadata {
            workspace_path: "default".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: None,
            trigger: Some("session-worktree-inference".to_string()),
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree: Some(SessionWorktreeAssignment {
                path: "worktrees/s-1".into(),
                branch: "agent/s-1/session".to_string(),
                allocation_mode: SessionWorktreeAllocationMode::New,
                status: SessionWorktreeStatus::Active,
                predecessor_session_id: None,
                predecessor_path: None,
            }),
        },
        links: SessionLinks::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: Vec::new(),
        picked_up_handoff_ids: Vec::new(),
        active_run_id: String::new(),
        runs: Vec::new(),
        pinned_entities: Vec::new(),
        workflow: Default::default(),
    }
}

fn full_manifest(started_at: DateTimeUtc, captured_at: DateTimeUtc) -> PersistedSessionManifest {
    let mut record = stub_manifest(captured_at);
    record.source = "copilot-hook".to_string();
    record.started_at = started_at;
    record.metadata.agent_id = Some("copilot-agent".to_string());
    record.metadata.trigger = Some("Stop".to_string());
    record.metadata.copilot_version = Some("0.61.0".to_string());
    record.links.ticket_ids = vec!["ticket-a".to_string()];
    record
}

#[test]
fn stub_side_defers_to_fully_captured_side() {
    let stub = stub_manifest(time(1, 35, 14));
    let full = full_manifest(time(0, 12, 23), time(1, 20, 34));

    let merged = merge_manifest(stub, full);

    assert_eq!(merged.source, "copilot-hook");
    assert_eq!(merged.metadata.copilot_version.as_deref(), Some("0.61.0"));
    // Newer captured_at wins even though it came from the stub side.
    assert_eq!(merged.captured_at, time(1, 35, 14));
    // Worktree assignment present on both sides is preserved.
    assert!(merged.metadata.worktree.is_some());
    assert_eq!(merged.links.ticket_ids, vec!["ticket-a".to_string()]);
}

#[test]
fn merge_is_symmetric_regardless_of_argument_order() {
    let stub = stub_manifest(time(1, 35, 14));
    let full = full_manifest(time(0, 12, 23), time(1, 20, 34));

    let a = merge_manifest(stub.clone(), full.clone());
    let b = merge_manifest(full, stub);

    assert_eq!(a.source, b.source);
    assert_eq!(a.captured_at, b.captured_at);
    assert_eq!(a.metadata.worktree, b.metadata.worktree);
}

#[test]
fn transcript_turns_union_by_sequence_preferring_newer_capture() {
    let turn = |sequence, content: &str, captured_at| SessionTurn {
        sequence,
        role: session_api::SessionRole::User,
        content: content.to_string(),
        captured_at,
        tool_name: None,
        model: None,
        event_meta: None,
    };

    let a = PersistedSessionTranscript {
        schema_version: 1,
        session_id: "s-1".to_string(),
        captured_at: time(1, 20, 0),
        turns: vec![turn(0, "hello", time(1, 19, 0))],
    };
    let b = PersistedSessionTranscript {
        schema_version: 1,
        session_id: "s-1".to_string(),
        captured_at: time(1, 35, 0),
        turns: vec![
            turn(0, "hello (revised)", time(1, 20, 0)),
            turn(1, "follow-up", time(1, 35, 0)),
        ],
    };

    let merged = merge_transcript(a, b);

    assert_eq!(merged.turns.len(), 2);
    assert_eq!(merged.turns[0].content, "hello (revised)");
    assert_eq!(merged.turns[1].content, "follow-up");
    assert_eq!(merged.captured_at, time(1, 35, 0));
}

#[test]
fn manifest_json_roundtrip_rejects_mismatched_session_ids() {
    let ours = serde_json::to_string(&stub_manifest(time(1, 0, 0))).unwrap();
    let mut theirs_record = stub_manifest(time(1, 0, 0));
    theirs_record.session_id = "s-2".to_string();
    let theirs = serde_json::to_string(&theirs_record).unwrap();

    let result = merge_manifest_json(&ours, &theirs);
    assert!(result.is_err());
}

#[test]
fn events_union_by_id_and_reject_conflicting_reuse() {
    let event = |id: &str, captured_at| CopilotHookEvent {
        event_id: Some(id.to_owned()),
        parent_event_id: None,
        event_type: Some("UserPromptSubmit".to_owned()),
        captured_at: Some(captured_at),
        turn_id: None,
        message_id: None,
        tool_call_id: None,
        tool_name: None,
        tool_success: None,
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: None,
        data_json: None,
        raw_event_json: None,
    };
    let a = PersistedSessionEvents {
        schema_version: 1,
        session_id: "s-1".to_owned(),
        captured_at: time(1, 20, 0),
        events: vec![event("one", time(1, 10, 0))],
    };
    let b = PersistedSessionEvents {
        schema_version: 1,
        session_id: "s-1".to_owned(),
        captured_at: time(1, 30, 0),
        events: vec![event("one", time(1, 10, 0)), event("two", time(1, 20, 0))],
    };
    let merged = merge_events(a.clone(), b).unwrap();
    assert_eq!(merged.events.len(), 2);
    assert_eq!(merged.events[0].event_id.as_deref(), Some("one"));
    assert_eq!(merged.events[1].event_id.as_deref(), Some("two"));

    let mut conflicting = a;
    conflicting.events[0].event_type = Some("Stop".to_owned());
    assert!(
        merge_events(
            conflicting,
            PersistedSessionEvents {
                schema_version: 1,
                session_id: "s-1".to_owned(),
                captured_at: time(1, 20, 0),
                events: vec![event("one", time(1, 10, 0))],
            }
        )
        .is_err()
    );
}
