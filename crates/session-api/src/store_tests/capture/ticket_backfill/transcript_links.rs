#[allow(unused_imports)]
use super::*;

#[test]
pub(in super::super) fn backfill_matches_ticket_tool_suffixes_across_server_prefixes() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let ticket_id = uuid::Uuid::parse_str("33333333-8888-4888-8888-888888888888").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);

    for (index, prefix) in ["mcp_ticket-mcp", "mcp_rmcp5", "mcp_rmcp6"]
        .iter()
        .enumerate()
    {
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
        let record = config
            .read_session(&format!("session-suffix-{index}"))
            .unwrap();
        assert_eq!(record.links.ticket_ids, vec![ticket_id.to_string()]);
    }
}

#[test]
pub(in super::super) fn backfill_ticket_tool_call_never_sets_strict_ticket_id() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let ticket_id = uuid::Uuid::parse_str("66666666-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);
    write_raw_session_with_turns(
        &store_root,
        "session-one-claim",
        None,
        None,
        None,
        vec![ticket_tool_turn(
            "mcp_ticket-mcp_board_check_in",
            serde_json::json!({"ticket_id": ticket_id}),
            "",
        )],
    );

    config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-one-claim").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert_eq!(record.links.ticket_ids, vec![ticket_id.to_string()]);
}

#[test]
pub(in super::super) fn backfill_discards_unresolvable_transcript_short_id() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let missing = "77777777";
    write_raw_session_with_turns(
        &store_root,
        "session-missing-ticket",
        None,
        None,
        None,
        vec![ticket_tool_turn(
            "mcp_rmcp6_board_check_in",
            serde_json::json!({"ticket_id": missing}),
            "",
        )],
    );

    config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-missing-ticket").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert!(record.links.ticket_ids.is_empty());
}

#[test]
pub(in super::super) fn backfill_resolves_ticket_tool_short_ids_without_mining_content() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let ticket_id = uuid::Uuid::parse_str("88888888-dddd-4ddd-8ddd-dddddddddddd").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);
    write_raw_session_with_turns(
        &store_root,
        "session-short-id",
        None,
        None,
        None,
        vec![ticket_tool_turn(
            "mcp_context-mcp_execute",
            serde_json::json!({"nested": [{"payload": "{\"id\":\"88888888\"}"}]}),
            "See ce://default/ticket/99999999-dddd-4ddd-8ddd-dddddddddddd.",
        )],
    );

    config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-short-id").unwrap();
    assert_eq!(record.links.ticket_ids, vec![ticket_id.to_string()]);
}

#[test]
pub(in super::super) fn backfill_transcript_dry_run_preserves_session_artifacts() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let ticket_id = uuid::Uuid::parse_str("99999999-eeee-4eee-8eee-eeeeeeeeeeee").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);
    write_raw_session_with_turns(
        &store_root,
        "session-dry-run",
        None,
        None,
        None,
        vec![ticket_tool_turn(
            "mcp_ticket-mcp_get_ticket",
            serde_json::json!({"id": ticket_id}),
            "",
        )],
    );

    let session_dir = store_root.join("sessions/session-dry-run");
    let manifest_path = session_dir.join("session.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["metadata"]["workspace_path"] =
        serde_json::Value::String(config.workspace_path.clone());
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let before_session = fs::read(session_dir.join("session.json")).unwrap();
    let before_transcript = fs::read(session_dir.join("transcript.json")).unwrap();

    let report = config.backfill_ticket_links(false).unwrap();
    assert_eq!(report.total_would_link, 1);
    assert_eq!(
        fs::read(session_dir.join("session.json")).unwrap(),
        before_session
    );
    assert_eq!(
        fs::read(session_dir.join("transcript.json")).unwrap(),
        before_transcript
    );
}

#[test]
pub(in super::super) fn backfill_transcript_ticket_links_are_idempotent() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let ticket_id = uuid::Uuid::parse_str("aaaaaaaa-ffff-4fff-8fff-ffffffffffff").unwrap();
    seed_ticket(&store_root.join(".ticket"), ticket_id);
    write_raw_session_with_turns(
        &store_root,
        "session-idempotent",
        None,
        None,
        None,
        vec![ticket_tool_turn(
            "mcp_ticket-mcp_update_ticket",
            serde_json::json!({"id": ticket_id}),
            "",
        )],
    );

    config.backfill_ticket_links(true).unwrap();
    let second = config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-idempotent").unwrap();
    assert_eq!(second.total_would_link, 0);
    assert_eq!(record.links.ticket_ids, vec![ticket_id.to_string()]);
}

#[test]
pub(in super::super) fn backfill_skips_malformed_or_missing_tool_request_payloads() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    write_raw_session_with_turns(
        &store_root,
        "session-malformed-payload",
        None,
        None,
        None,
        vec![
            ticket_tool_turn(
                "mcp_rmcp5_get_ticket",
                serde_json::json!("not an object"),
                "",
            ),
            ticket_tool_turn("mcp_rmcp6_get_ticket", serde_json::Value::Null, ""),
        ],
    );

    config.backfill_ticket_links(true).unwrap();
    let record = config.read_session("session-malformed-payload").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert!(record.links.ticket_ids.is_empty());
}
