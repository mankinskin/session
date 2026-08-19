use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
};

use crate::{
    SessionLinks,
    SessionMetadata,
    SessionRecord,
    SessionTurn,
    SessionTurnEventMeta,
    SessionWorktreeAssignment,
};

/// Writes a session's `session.json`/`transcript.json` directly to disk,
/// bypassing `check_in_worktree` (which always pairs a worktree assignment
/// with a `ticket_id`). This reproduces the historical on-disk shape the
/// backfill exists to repair: a `worktree` assignment present with no
/// `ticket_id` yet written.
fn write_raw_session(
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

fn write_raw_session_with_turns(
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
            workspace_slug: "context-engine".to_string(),
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

fn ticket_tool_turn(
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
fn write_corrupt_session(store_root: &std::path::Path, session_id: &str) {
    let session_dir = store_root.join("sessions").join(session_id);
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("context.json"),
        r#"{"schema_version":1,"session_id":"x"}"#,
    )
    .unwrap();
}

fn seed_ticket(ticket_store_root: &std::path::Path, ticket_id: uuid::Uuid) {
    let store =
        ticket_api::storage::TicketStore::open_or_init(ticket_store_root)
            .unwrap();
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
fn backfill_links_via_agent_branch_shape() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let ticket_id =
        uuid::Uuid::parse_str("aaaaaaaa-1111-4111-8111-111111111111")
            .unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);

    write_raw_session(
        &store_root,
        "session-branch",
        None,
        Some("agent/aaaaaaaa-some-slug"),
        Some(PathBuf::from("/tmp/worktrees/aaaaaaaa-some-slug")),
    );

    let dry = config.backfill_ticket_links(false).unwrap();
    assert_eq!(dry.total_sessions, 1);
    assert_eq!(dry.linked_via_branch, 1);
    assert_eq!(dry.total_would_link, 1);
    assert_eq!(
        config.read_session("session-branch").unwrap().metadata.ticket_id,
        None,
        "dry run must not write"
    );

    let written = config.backfill_ticket_links(true).unwrap();
    assert_eq!(written.linked_via_branch, 1);
    let record = config.read_session("session-branch").unwrap();
    assert_eq!(record.metadata.ticket_id.as_deref(), Some(ticket_id.to_string().as_str()));

    let matches = config
        .sessions_for_ticket(&ticket_id.to_string(), RelationStrength::Strict)
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].session_id, "session-branch");
}

#[test]
fn backfill_falls_back_to_worktree_path_when_branch_absent() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let ticket_id =
        uuid::Uuid::parse_str("bbbbbbbb-2222-4222-8222-222222222222")
            .unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);

    write_raw_session(
        &store_root,
        "session-worktree-path",
        None,
        None,
        Some(PathBuf::from(
            "/repo/.worktrees/bbbbbbbb-some-other-slug",
        )),
    );

    let report = config.backfill_ticket_links(true).unwrap();
    assert_eq!(report.linked_via_branch, 0);
    assert_eq!(report.linked_via_worktree_path, 1);
    let record = config.read_session("session-worktree-path").unwrap();
    assert_eq!(
        record.metadata.ticket_id.as_deref(),
        Some(ticket_id.to_string().as_str())
    );
}

#[test]
fn backfill_branch_present_and_unmatched_does_not_fall_back() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let ticket_id =
        uuid::Uuid::parse_str("cccccccc-3333-4333-8333-333333333333")
            .unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);

    // Branch does not match `agent/<8hex>-slug`; worktree_path does encode a
    // valid short id, but branch presence must not be skipped in favor of it
    // unless the branch itself fails to parse as the agent shape.
    write_raw_session(
        &store_root,
        "session-plain-branch",
        None,
        Some("main"),
        Some(PathBuf::from("/repo/.worktrees/cccccccc-some-slug")),
    );

    let report = config.backfill_ticket_links(true).unwrap();
    // "main" does not match the agent/<8hex>-slug shape, so the worktree_path
    // fallback is exercised and the ticket resolves.
    assert_eq!(report.linked_via_worktree_path, 1);
    let record = config.read_session("session-plain-branch").unwrap();
    assert_eq!(
        record.metadata.ticket_id.as_deref(),
        Some(ticket_id.to_string().as_str())
    );
}

#[test]
fn backfill_handoff_links_multiple_target_tickets() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let ticket_one =
        uuid::Uuid::parse_str("dddddddd-4444-4444-8444-444444444444")
            .unwrap();
    let ticket_two =
        uuid::Uuid::parse_str("eeeeeeee-5555-4555-8555-555555555555")
            .unwrap();
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

    let record = config.read_session("f4444444-4444-4444-8444-444444444444").unwrap();
    assert_eq!(record.metadata.ticket_id, None, "handoff writes linked tier, not strict");
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
fn backfill_skips_unresolvable_short_id_without_writing() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

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
        config.read_session("session-unresolvable").unwrap().metadata.ticket_id,
        None
    );
}

#[test]
fn backfill_skips_corrupt_entry_and_continues() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let ticket_id =
        uuid::Uuid::parse_str("11111111-6666-4666-8666-666666666666")
            .unwrap();
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
    let record = config.read_session("f2222222-2222-4222-8222-222222222222").unwrap();
    assert_eq!(
        record.metadata.ticket_id.as_deref(),
        Some(ticket_id.to_string().as_str())
    );
}

#[test]
fn backfill_is_idempotent_and_never_overwrites_real_check_in() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let ticket_id =
        uuid::Uuid::parse_str("22222222-7777-4777-8777-777777777777")
            .unwrap();
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
        config.read_session("session-branch").unwrap().metadata.ticket_id.as_deref(),
        Some(ticket_id.to_string().as_str())
    );
}

#[test]
fn backfill_matches_ticket_tool_suffixes_across_server_prefixes() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
    let ticket_id = uuid::Uuid::parse_str("33333333-8888-4888-8888-888888888888").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);

    for (index, prefix) in ["mcp_ticket-mcp", "mcp_rmcp5", "mcp_rmcp6"].iter().enumerate() {
        write_raw_session_with_turns(
            &store_root,
            &format!("session-suffix-{index}"),
            None,
            None,
            None,
            vec![ticket_tool_turn(
                &format!("{prefix}_update_ticket"),
                serde_json::json!({"id": ticket_id}),
                "",
            )],
        );
    }

    config.backfill_ticket_links(true).unwrap();
    for index in 0..3 {
        let record = config.read_session(&format!("session-suffix-{index}")).unwrap();
        assert_eq!(record.links.ticket_ids, vec![ticket_id.to_string()]);
    }
}

#[test]
fn backfill_keeps_ambiguous_claims_linked_without_a_primary_ticket() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
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
            ticket_tool_turn("mcp_rmcp5_board_check_in", serde_json::json!({"ticket_id": first}), ""),
            ticket_tool_turn("mcp_rmcp6_board_check_out", serde_json::json!({"ticket_id": second}), ""),
        ],
    );

    config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-ambiguous-claims").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert_eq!(record.links.ticket_ids, vec![first.to_string(), second.to_string()]);
}

#[test]
fn backfill_ticket_tool_call_never_sets_strict_ticket_id() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
    let ticket_id = uuid::Uuid::parse_str("66666666-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);
    write_raw_session_with_turns(
        &store_root,
        "session-one-claim",
        None,
        None,
        None,
        vec![ticket_tool_turn("mcp_ticket-mcp_board_check_in", serde_json::json!({"ticket_id": ticket_id}), "")],
    );

    config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-one-claim").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert_eq!(record.links.ticket_ids, vec![ticket_id.to_string()]);
}

#[test]
fn backfill_discards_unresolvable_transcript_short_id() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
    let missing = "77777777";
    write_raw_session_with_turns(
        &store_root,
        "session-missing-ticket",
        None,
        None,
        None,
        vec![ticket_tool_turn("mcp_rmcp6_board_check_in", serde_json::json!({"ticket_id": missing}), "")],
    );

    config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-missing-ticket").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert!(record.links.ticket_ids.is_empty());
}

#[test]
fn backfill_resolves_ticket_tool_short_ids_without_mining_content() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
    let ticket_id = uuid::Uuid::parse_str("88888888-dddd-4ddd-8ddd-dddddddddddd").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);
    write_raw_session_with_turns(
        &store_root,
        "session-short-id",
        None,
        None,
        None,
        vec![ticket_tool_turn("mcp_context-mcp_execute", serde_json::json!({"nested": [{"payload": "{\"id\":\"88888888\"}"}]}), "See ce://default/ticket/99999999-dddd-4ddd-8ddd-dddddddddddd.")],
    );

    config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-short-id").unwrap();
    assert_eq!(record.links.ticket_ids, vec![ticket_id.to_string()]);
}

#[test]
fn backfill_transcript_dry_run_preserves_session_artifacts() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
    let ticket_id = uuid::Uuid::parse_str("99999999-eeee-4eee-8eee-eeeeeeeeeeee").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);
    write_raw_session_with_turns(
        &store_root,
        "session-dry-run",
        None,
        None,
        None,
        vec![ticket_tool_turn("mcp_ticket-mcp_get_ticket", serde_json::json!({"id": ticket_id}), "")],
    );

    let session_dir = store_root.join("sessions/session-dry-run");
    let before_session = fs::read(session_dir.join("session.json")).unwrap();
    let before_transcript = fs::read(session_dir.join("transcript.json")).unwrap();

    let report = config.backfill_ticket_links(false).unwrap();
    assert_eq!(report.total_would_link, 1);
    assert_eq!(fs::read(session_dir.join("session.json")).unwrap(), before_session);
    assert_eq!(fs::read(session_dir.join("transcript.json")).unwrap(), before_transcript);
}

#[test]
fn backfill_transcript_ticket_links_are_idempotent() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
    let ticket_id = uuid::Uuid::parse_str("aaaaaaaa-ffff-4fff-8fff-ffffffffffff").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);
    write_raw_session_with_turns(
        &store_root,
        "session-idempotent",
        None,
        None,
        None,
        vec![ticket_tool_turn("mcp_ticket-mcp_update_ticket", serde_json::json!({"id": ticket_id}), "")],
    );

    config.backfill_ticket_links(true).unwrap();
    let second = config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-idempotent").unwrap();
    assert_eq!(second.total_would_link, 0);
    assert_eq!(record.links.ticket_ids, vec![ticket_id.to_string()]);
}

#[test]
fn backfill_skips_malformed_or_missing_tool_request_payloads() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");
    write_raw_session_with_turns(
        &store_root,
        "session-malformed-payload",
        None,
        None,
        None,
        vec![
            ticket_tool_turn("mcp_rmcp5_get_ticket", serde_json::json!("not an object"), ""),
            ticket_tool_turn("mcp_rmcp6_get_ticket", serde_json::Value::Null, ""),
        ],
    );

    config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-malformed-payload").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert!(record.links.ticket_ids.is_empty());
}
