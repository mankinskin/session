#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn query_returns_seeded_session() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let config = SessionStoreConfig::new(store_root.clone());
    seed_session(&config, "22222222-2222-4222-8222-222222222222", "agent-q");
    let unreadable_id = "f3333333-3333-4333-8333-333333333333";
    let unreadable_dir = store_root.join("sessions").join(unreadable_id);
    std::fs::create_dir_all(&unreadable_dir).unwrap();
    std::fs::write(
        unreadable_dir.join("session.json"),
        b"private malformed session payload",
    )
    .unwrap();

    let result = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "query",
        "--agent-id",
        "agent-q",
    ]);
    assert_eq!(result["count"], 1);
    assert_eq!(
        result["sessions"][0]["session_id"],
        "22222222-2222-4222-8222-222222222222"
    );
    assert_eq!(result["diagnostics"][0]["session_id"], unreadable_id);
    assert_eq!(
        result["diagnostics"][0]["code"],
        "unreadable_session_record"
    );
    assert!(
        !result
            .to_string()
            .contains("private malformed session payload")
    );
    assert!(!result.to_string().contains(&store_root_str));
}

#[test]
pub(super) fn sessions_for_ticket_returns_seeded_session_at_strict_tier() {
    let dir = tempdir().unwrap();
    let store_root: PathBuf = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let worktree = managed_worktree(
        &dir,
        "33333333-3333-4333-8333-333333333333",
        "wt-ticket",
        "feature/ticket-abc",
    );
    let worktree_str = worktree.to_string_lossy().to_string();

    run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "check-in",
        "--session-id",
        "33333333-3333-4333-8333-333333333333",
        "--owner-id",
        "agent-ticket",
        "--ticket-id",
        "ticket-abc",
        "--worktree-path",
        &worktree_str,
        "--branch",
        "feature/ticket-abc",
    ]);

    let result = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "sessions-for-ticket",
        "ticket-abc",
        "--strength",
        "strict",
    ]);
    assert_eq!(result["count"], 1);
    assert_eq!(
        result["sessions"][0]["session_id"],
        "33333333-3333-4333-8333-333333333333"
    );
    assert_eq!(result["sessions"][0]["branch"], "feature/ticket-abc");
    assert_eq!(result["sessions"][0]["matched_strength"], "strict");

    let unrelated = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "sessions-for-ticket",
        "ticket-other",
        "--strength",
        "mentioned",
    ]);
    assert_eq!(unrelated["count"], 0);
}

#[test]
pub(super) fn peek_range_and_skeleton() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let config = SessionStoreConfig::new(store_root.clone());
    seed_session(&config, "44444444-4444-4444-8444-444444444444", "agent-p");

    let range = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "peek-range",
        "--session-id",
        "44444444-4444-4444-8444-444444444444",
        "--start",
        "1",
    ]);
    assert_eq!(range["total_turns"], 2);
    assert_eq!(range["start"], 1);
    assert_eq!(range["turns"].as_array().unwrap().len(), 1);

    let skeleton = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "peek-skeleton",
        "--session-id",
        "44444444-4444-4444-8444-444444444444",
    ]);
    assert_eq!(skeleton["total_turns"], 2);
    assert_eq!(skeleton["entries"][0]["preview"], "first turn body");
}

#[test]
pub(super) fn peek_prompt_pack_reports_guarded_entries() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let config = SessionStoreConfig::new(store_root.clone());
    seed_compaction_session(&config, "55555555-5555-4555-8555-555555555555", "agent-c");

    let pack = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "peek-prompt-pack",
        "--session-id",
        "55555555-5555-4555-8555-555555555555",
        "--summarize-threshold-chars",
        "120",
    ]);

    assert_eq!(pack["total_turns"], 5);
    assert_eq!(pack["dropped_turns"], 2);
    assert_eq!(pack["reference_only_turns"], 1);
    assert_eq!(pack["summarized_turns"], 1);
    assert_eq!(pack["entries"].as_array().unwrap().len(), 3);

    let entries = pack["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry["reason"] == "artifact-pointer-detected")
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["reason"] == "oversized-content")
    );
}

#[test]
pub(super) fn peek_prompt_pack_meets_quantitative_compactness_gate() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let config = SessionStoreConfig::new(store_root.clone());
    seed_compaction_session(
        &config,
        "66666666-6666-4666-8666-666666666666",
        "agent-gate",
    );

    let pack = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "peek-prompt-pack",
        "--session-id",
        "66666666-6666-4666-8666-666666666666",
        "--summarize-threshold-chars",
        "120",
    ]);

    let total = pack["total_turns"].as_u64().unwrap();
    let dropped = pack["dropped_turns"].as_u64().unwrap();
    let included = pack["entries"].as_array().unwrap().len() as u64;

    assert_eq!(total, 5);
    assert!(dropped >= 2);
    assert!(included <= 3);
}
