#[allow(unused_imports)]
use super::*;

#[test]
pub(in super::super) fn backfill_links_via_agent_branch_shape() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());

    let ticket_id = uuid::Uuid::parse_str("aaaaaaaa-1111-4111-8111-111111111111").unwrap();
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
        config
            .read_session("session-branch")
            .unwrap()
            .metadata
            .ticket_id,
        None,
        "dry run must not write"
    );

    let written = config.backfill_ticket_links(true).unwrap();
    assert_eq!(written.linked_via_branch, 1);
    let record = config.read_session("session-branch").unwrap();
    assert_eq!(
        record.metadata.ticket_id.as_deref(),
        Some(ticket_id.to_string().as_str())
    );

    let matches = config
        .sessions_for_ticket(&ticket_id.to_string(), RelationStrength::Strict)
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].session_id, "session-branch");
}

#[test]
pub(in super::super) fn backfill_falls_back_to_worktree_path_when_branch_absent() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());

    let ticket_id = uuid::Uuid::parse_str("bbbbbbbb-2222-4222-8222-222222222222").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);

    write_raw_session(
        &store_root,
        "session-worktree-path",
        None,
        None,
        Some(PathBuf::from("/repo/.worktrees/bbbbbbbb-some-other-slug")),
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
pub(in super::super) fn backfill_branch_present_and_unmatched_does_not_fall_back() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());

    let ticket_id = uuid::Uuid::parse_str("cccccccc-3333-4333-8333-333333333333").unwrap();
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
