use crate::{
    RelationStrength,
    SessionHandoffPackage,
};

const TARGET_TICKET: &str = "ticket-target";

fn seed_strict_session(
    config: &SessionStoreConfig,
    tempdir: &TempDir,
) {
    let worktree_path = managed_worktree(
        tempdir,
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "strict",
        "agent/strict-branch",
    );
    config
        .check_in_worktree(SessionWorktreeCheckInRequest {
            session_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            owner_id: "agent-strict".to_string(),
            ticket_id: TARGET_TICKET.to_string(),
            worktree_path,
            branch: "agent/strict-branch".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();
}

fn seed_linked_session(config: &SessionStoreConfig) {
    let mut request = SessionCaptureRequest::copilot(sample_payload(
        "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        Some("conversation-linked"),
        sample_time(),
        &["Working on something else"],
    ));
    request.links.ticket_ids.push(TARGET_TICKET.to_string());
    config.persist_capture(request).unwrap();
}

fn seed_mentioned_session(config: &SessionStoreConfig) {
    config
        .capture_copilot_hook(sample_payload(
            "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
            Some("conversation-mentioned"),
            sample_time(),
            &["Handed off for follow-up"],
        ))
        .unwrap();
    config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(
                "cccccccc-cccc-4ccc-8ccc-cccccccccccc".to_string(),
            ),
            ..Default::default()
        })
        .unwrap();
    config
        .create_handoff_record(
            "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
            Some(SessionHandoffPackage {
                objective: "Follow up on target ticket".to_string(),
                target_tickets: vec![crate::SessionHandoffTargetTicket {
                    id: TARGET_TICKET.to_string(),
                    why: "Follow-up ownership".to_string(),
                    state: "ready".to_string(),
                    acceptance_criteria: vec![],
                }],
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
}

/// Mentions the ticket id only in transcript text, never in metadata, links,
/// or a handoff package. Must be excluded at every tier (AC2: no tier scans
/// `transcript.json` text).
fn seed_transcript_only_session(config: &SessionStoreConfig) {
    config
        .capture_copilot_hook(sample_payload(
            "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
            Some("conversation-transcript-only"),
            sample_time(),
            &[&format!("Discussing {TARGET_TICKET} in passing")],
        ))
        .unwrap();
}

fn seed_unrelated_session(config: &SessionStoreConfig) {
    config
        .capture_copilot_hook(sample_payload(
            "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
            Some("conversation-unrelated"),
            sample_time(),
            &["Nothing to do with the target ticket"],
        ))
        .unwrap();
}

fn seeded_config(tempdir: &TempDir) -> SessionStoreConfig {
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    seed_strict_session(&config, tempdir);
    seed_linked_session(&config);
    seed_mentioned_session(&config);
    seed_transcript_only_session(&config);
    seed_unrelated_session(&config);
    config
}

#[test]
fn sessions_for_ticket_strict_matches_only_metadata_ticket_id() {
    let tempdir = TempDir::new().unwrap();
    let config = seeded_config(&tempdir);

    let matches = config
        .sessions_for_ticket(TARGET_TICKET, RelationStrength::Strict)
        .unwrap();

    assert_eq!(matches.len(), 1);
    let row = &matches[0];
    assert_eq!(row.session_id, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    assert_eq!(row.agent_id.as_deref(), Some("agent-strict"));
    assert_eq!(row.branch.as_deref(), Some("agent/strict-branch"));
    assert_eq!(
        row.worktree_path.as_deref(),
        Some(
            tempdir
                .path()
                .join(".worktrees")
                .join("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
                .join("strict")
                .canonicalize()
                .unwrap()
                .as_path(),
        )
    );
    let manifest = config
        .read_session("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
        .unwrap();
    assert_eq!(manifest.metadata.agent_id, None);
    assert_eq!(manifest.metadata.ticket_id, None);
    assert_eq!(
        manifest.metadata.worktree.unwrap().branch,
        "agent/strict-branch"
    );
    assert!(row.started_at <= row.ended_at);
    assert_eq!(row.matched_strength, RelationStrength::Strict);
}

#[test]
fn sessions_for_ticket_linked_includes_strict_and_links_but_not_mentioned() {
    let tempdir = TempDir::new().unwrap();
    let config = seeded_config(&tempdir);

    let mut matches = config
        .sessions_for_ticket(TARGET_TICKET, RelationStrength::Linked)
        .unwrap();
    matches.sort_by(|left, right| left.session_id.cmp(&right.session_id));

    let ids: Vec<&str> =
        matches.iter().map(|row| row.session_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
        ]
    );

    let strict = matches
        .iter()
        .find(|row| row.session_id == "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
        .unwrap();
    assert_eq!(strict.matched_strength, RelationStrength::Strict);

    let linked = matches
        .iter()
        .find(|row| row.session_id == "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")
        .unwrap();
    assert_eq!(linked.matched_strength, RelationStrength::Linked);
}

#[test]
fn sessions_for_ticket_mentioned_includes_all_structured_tiers_only() {
    let tempdir = TempDir::new().unwrap();
    let config = seeded_config(&tempdir);

    let mut matches = config
        .sessions_for_ticket(TARGET_TICKET, RelationStrength::Mentioned)
        .unwrap();
    matches.sort_by(|left, right| left.session_id.cmp(&right.session_id));

    let ids: Vec<&str> =
        matches.iter().map(|row| row.session_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
        ]
    );

    let mentioned = matches
        .iter()
        .find(|row| row.session_id == "cccccccc-cccc-4ccc-8ccc-cccccccccccc")
        .unwrap();
    assert_eq!(mentioned.matched_strength, RelationStrength::Mentioned);
}

/// Ticket 2b75bac2: a session checked in against a ticket must be
/// discoverable at the strict tier immediately afterward, with no
/// backfill or transcript reading involved.
#[test]
fn check_in_worktree_forward_captures_ticket_linkage_for_immediate_discovery() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let worktree_path = managed_worktree(
        &tempdir,
        "f1111111-1111-4111-8111-111111111111",
        "forward-capture",
        "agent/forward-capture-branch",
    );

    config
        .check_in_worktree(SessionWorktreeCheckInRequest {
            session_id: "f1111111-1111-4111-8111-111111111111".to_string(),
            owner_id: "agent-forward-capture".to_string(),
            ticket_id: TARGET_TICKET.to_string(),
            worktree_path,
            branch: "agent/forward-capture-branch".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();

    let matches = config
        .sessions_for_ticket(TARGET_TICKET, RelationStrength::Strict)
        .unwrap();

    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0].session_id,
        "f1111111-1111-4111-8111-111111111111"
    );
    assert_eq!(matches[0].matched_strength, RelationStrength::Strict);
}

/// Ticket e4d4c667: a single corrupt/malformed session entry must not
/// abort the whole scan; skip it and keep returning the readable ones.
#[test]
fn sessions_for_ticket_skips_unreadable_session_and_returns_the_rest() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let worktree_path = managed_worktree(
        &tempdir,
        "f2222222-2222-4222-8222-222222222222",
        "good",
        "agent/good-branch",
    );

    config
        .check_in_worktree(SessionWorktreeCheckInRequest {
            session_id: "f2222222-2222-4222-8222-222222222222".to_string(),
            owner_id: "agent-good".to_string(),
            ticket_id: TARGET_TICKET.to_string(),
            worktree_path,
            branch: "agent/good-branch".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();

    let corrupt_dir = tempdir
        .path()
        .join("store")
        .join("sessions")
        .join("f3333333-3333-4333-8333-333333333333");
    std::fs::create_dir_all(&corrupt_dir).unwrap();
    std::fs::write(corrupt_dir.join("session.json"), b"{ not valid json")
        .unwrap();
    std::fs::write(corrupt_dir.join("transcript.json"), b"{}").unwrap();

    let matches = config
        .sessions_for_ticket(TARGET_TICKET, RelationStrength::Strict)
        .unwrap();

    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0].session_id,
        "f2222222-2222-4222-8222-222222222222"
    );
}
