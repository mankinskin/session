#[allow(unused_imports)]
use super::*;

pub(super) use std::{collections::BTreeMap, fs, path::PathBuf};

pub(super) use crate::{
    SessionLinks, SessionMetadata, SessionRecord, SessionTurn, SessionTurnEventMeta,
    SessionWorktreeAssignment,
};

/// Writes a session's `session.json`/`transcript.json` directly to disk,
/// bypassing `check_in_worktree` (which always pairs a worktree assignment
/// with a `ticket_id`). This reproduces the historical on-disk shape the
/// backfill exists to repair: a `worktree` assignment present with no
/// `ticket_id` yet written.
pub(super) fn write_raw_session(
    store_root: &std::path::Path,
    session_id: &str,
    ticket_id: Option<&str>,
    branch: Option<&str>,
    worktree_path: Option<PathBuf>,
) {
    write_raw_session_with_turns(
        store_root,
        session_id,
        ticket_id,
        branch,
        worktree_path,
        vec![],
    );
}

pub(super) fn write_raw_session_with_turns(
    store_root: &std::path::Path,
    session_id: &str,
    ticket_id: Option<&str>,
    branch: Option<&str>,
    worktree_path: Option<PathBuf>,
    turns: Vec<SessionTurn>,
) {
    let session_dir = store_root.join("sessions").join(session_id);
    fs::create_dir_all(&session_dir).unwrap();

    let worktree = if branch.is_some() || worktree_path.is_some() {
        Some(SessionWorktreeAssignment {
            path: worktree_path.unwrap_or_else(|| PathBuf::from("unused")),
            branch: branch.unwrap_or("unused").to_string(),
            allocation_mode: SessionWorktreeAllocationMode::New,
            status: SessionWorktreeStatus::Active,
            predecessor_session_id: None,
            predecessor_path: None,
        })
    } else {
        None
    };

    let record = SessionRecord {
        schema_version: SESSION_SCHEMA_VERSION,
        session_id: session_id.to_string(),
        source: "test-fixture".to_string(),
        started_at: sample_time(),
        captured_at: sample_time(),
        metadata: SessionMetadata {
            workspace_path: "context-engine".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: ticket_id.map(str::to_string),
            model: None,
            trigger: None,
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree,
        },
        turns,
        links: SessionLinks::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: vec![],
        picked_up_handoff_ids: vec![],
    };

    let manifest = PersistedSessionManifest::from(&record);
    let transcript = PersistedSessionTranscript::from(&record);
    fs::write(
        session_dir.join("session.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(
        session_dir.join("transcript.json"),
        serde_json::to_string_pretty(&transcript).unwrap(),
    )
    .unwrap();
}

pub(super) fn ticket_tool_turn(
    tool_name: &str,
    arguments: serde_json::Value,
    content: &str,
) -> SessionTurn {
    SessionTurn {
        sequence: 0,
        role: SessionRole::Assistant,
        content: content.to_string(),
        captured_at: sample_time(),
        tool_name: None,
        model: None,
        event_meta: Some(SessionTurnEventMeta {
            tool_requests_json: Some(serde_json::json!([{
                "name": tool_name,
                "arguments": arguments,
                "toolCallId": "fixture-call",
                "type": "function"
            }])),
            ..Default::default()
        }),
    }
}

/// Writes only legacy `context.json`, mirroring the two deliberate corrupt fixture
/// entries in the real store (`session.json`/`transcript.json` absent).
pub(super) fn write_corrupt_session(store_root: &std::path::Path, session_id: &str) {
    let session_dir = store_root.join("sessions").join(session_id);
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("context.json"),
        r#"{"schema_version":1,"session_id":"x"}"#,
    )
    .unwrap();
}

pub(super) fn seed_ticket(ticket_store_root: &std::path::Path, ticket_id: uuid::Uuid) {
    let store = ticket_api::storage::TicketStore::open_or_init(ticket_store_root).unwrap();
    store
        .create(
            Some(ticket_id),
            "task",
            Some("backfill fixture ticket"),
            None,
            BTreeMap::new(),
            None,
            None,
        )
        .unwrap();
}

#[test]
pub(super) fn backfill_handoff_links_multiple_target_tickets() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());

    let ticket_one = uuid::Uuid::parse_str("dddddddd-4444-4444-8444-444444444444").unwrap();
    let ticket_two = uuid::Uuid::parse_str("eeeeeeee-5555-4555-8555-555555555555").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_one);
    seed_ticket(&store_root.join(".ticket"), ticket_two);

    config
        .capture_copilot_hook(sample_payload(
            "f4444444-4444-4444-8444-444444444444",
            Some("conversation-handoff"),
            sample_time(),
            &["Handed off for follow-up"],
        ))
        .unwrap();
    config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some("f4444444-4444-4444-8444-444444444444".to_string()),
            ..Default::default()
        })
        .unwrap();
    config
        .create_handoff_record(
            "f4444444-4444-4444-8444-444444444444",
            Some(SessionHandoffPackage {
                objective: "Follow up on two tickets".to_string(),
                target_tickets: vec![
                    crate::SessionHandoffTargetTicket {
                        id: ticket_one.to_string(),
                        why: "Follow-up ownership".to_string(),
                        state: "ready".to_string(),
                        acceptance_criteria: vec![],
                    },
                    crate::SessionHandoffTargetTicket {
                        id: ticket_two.to_string(),
                        why: "Follow-up ownership".to_string(),
                        state: "ready".to_string(),
                        acceptance_criteria: vec![],
                    },
                ],
                higher_level_objective: "Complete the program work".to_string(),
                upward_context: vec![crate::SessionHandoffUpwardContextEntry {
                    entity_urn: "ce://default/ticket/program".to_string(),
                    title: "Program".to_string(),
                    role: crate::SessionHandoffUpwardContextRole::Epic,
                }],
                ..Default::default()
            }),
            vec![],
            None,
        )
        .unwrap();

    let report = config.backfill_ticket_links(true).unwrap();
    assert_eq!(report.linked_via_handoff, 2);
    assert!(report.handoff_already_at_mentioned);

    let record = config
        .read_session("f4444444-4444-4444-8444-444444444444")
        .unwrap();
    assert_eq!(
        record.metadata.ticket_id, None,
        "handoff writes linked tier, not strict"
    );
    assert!(record.links.links_to_ticket(&ticket_one.to_string()));
    assert!(record.links.links_to_ticket(&ticket_two.to_string()));

    for ticket in [&ticket_one, &ticket_two] {
        let strict = config
            .sessions_for_ticket(&ticket.to_string(), RelationStrength::Strict)
            .unwrap();
        assert!(strict.is_empty());
        let linked = config
            .sessions_for_ticket(&ticket.to_string(), RelationStrength::Linked)
            .unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].session_id, "f4444444-4444-4444-8444-444444444444");
    }
}

#[test]
pub(super) fn backfill_skips_unresolvable_short_id_without_writing() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());

    // No ticket store at all: every short id is unresolvable.
    write_raw_session(
        &store_root,
        "session-unresolvable",
        None,
        Some("agent/ffffffff-no-such-ticket"),
        None,
    );

    let report = config.backfill_ticket_links(true).unwrap();
    assert_eq!(report.skipped_unresolvable_shortid, 1);
    assert_eq!(report.linked_via_branch, 0);
    assert_eq!(report.total_would_link, 0);
    assert_eq!(
        config
            .read_session("session-unresolvable")
            .unwrap()
            .metadata
            .ticket_id,
        None
    );
}

#[test]
pub(super) fn backfill_skips_corrupt_entry_and_continues() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());

    let ticket_id = uuid::Uuid::parse_str("11111111-6666-4666-8666-666666666666").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);

    write_corrupt_session(&store_root, "f3333333-3333-4333-8333-333333333333");
    write_raw_session(
        &store_root,
        "f2222222-2222-4222-8222-222222222222",
        None,
        Some("agent/11111111-good-slug"),
        None,
    );

    let report = config.backfill_ticket_links(true).unwrap();
    assert_eq!(report.total_sessions, 2);
    assert_eq!(report.skipped_corrupt, 1);
    assert_eq!(report.linked_via_branch, 1);
    let record = config
        .read_session("f2222222-2222-4222-8222-222222222222")
        .unwrap();
    assert_eq!(
        record.metadata.ticket_id.as_deref(),
        Some(ticket_id.to_string().as_str())
    );
}

#[test]
pub(super) fn backfill_is_idempotent_and_never_overwrites_real_check_in() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());

    let ticket_id = uuid::Uuid::parse_str("22222222-7777-4777-8777-777777777777").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);

    // A real check-in already has ticket_id + worktree populated together;
    // its ticket_id must never be touched by the backfill.
    config
        .check_in_worktree(crate::SessionWorktreeCheckInRequest {
            session_id: "44444444-4444-4444-8444-444444444444".to_string(),
            owner_id: "agent-real".to_string(),
            ticket_id: "manually-assigned-ticket".to_string(),
            worktree_path: managed_worktree(
                &tempdir,
                "44444444-4444-4444-8444-444444444444",
                "real",
                "agent/22222222-different-slug",
            ),
            branch: "agent/22222222-different-slug".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();

    write_raw_session(
        &store_root,
        "session-branch",
        None,
        Some("agent/22222222-some-slug"),
        None,
    );

    let first = config.backfill_ticket_links(true).unwrap();
    assert_eq!(first.linked_via_branch, 1);
    assert_eq!(first.already_linked_untouched, 1);

    let second = config.backfill_ticket_links(true).unwrap();
    assert_eq!(second.linked_via_branch, 0);
    assert_eq!(second.total_would_link, 0);
    assert_eq!(second.already_linked_untouched, 2);

    assert_eq!(
        config
            .read_session("44444444-4444-4444-8444-444444444444")
            .unwrap()
            .metadata
            .ticket_id,
        None,
        "D1 keeps check-in ticket ownership out of the manifest"
    );
    assert_eq!(
        config
            .worktree_registry_entry("44444444-4444-4444-8444-444444444444")
            .unwrap()
            .unwrap()
            .ticket_id,
        "manually-assigned-ticket",
        "real check-in ticket ownership must never be overwritten"
    );
    assert_eq!(
        config
            .read_session("session-branch")
            .unwrap()
            .metadata
            .ticket_id
            .as_deref(),
        Some(ticket_id.to_string().as_str())
    );
}

#[test]
pub(super) fn backfill_keeps_ambiguous_claims_linked_without_a_primary_ticket() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let first = uuid::Uuid::parse_str("44444444-9999-4999-8999-999999999999").unwrap();
    let second = uuid::Uuid::parse_str("55555555-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
    seed_ticket(&store_root.join(".ticket"), first);
    seed_ticket(&store_root.join(".ticket"), second);
    write_raw_session_with_turns(
        &store_root,
        "session-ambiguous-claims",
        None,
        None,
        None,
        vec![
            ticket_tool_turn(
                "mcp_rmcp5_board_check_in",
                serde_json::json!({"ticket_id": first}),
                "",
            ),
            ticket_tool_turn(
                "mcp_rmcp6_board_check_out",
                serde_json::json!({"ticket_id": second}),
                "",
            ),
        ],
    );

    config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-ambiguous-claims").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert_eq!(
        record.links.ticket_ids,
        vec![first.to_string(), second.to_string()]
    );
}

#[path = "ticket_backfill/transcript_links.rs"]
mod transcript_links;
#[allow(unused_imports)]
pub use transcript_links::*;

#[path = "ticket_backfill/branch_links.rs"]
mod branch_links;
#[allow(unused_imports)]
pub use branch_links::*;
