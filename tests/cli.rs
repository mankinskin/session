use std::{
    path::PathBuf,
    process::Command,
};

use chrono::Utc;
use tempfile::tempdir;

use session_api::{
    CopilotHookMessage,
    CopilotHookPayload,
    SessionCaptureRequest,
    SessionRole,
    SessionStoreConfig,
};
use session::cli::{
    CliOutput,
    machine_output_format,
    parse_cli_from,
    run,
};

fn seed_session(
    config: &SessionStoreConfig,
    session_id: &str,
    agent: &str,
) {
    let payload = CopilotHookPayload {
        session_id: session_id.to_string(),
        workspace_slug: "default".to_string(),
        captured_at: Utc::now(),
        conversation_id: Some("conv-1".to_string()),
        agent_id: Some(agent.to_string()),
        model: None,
        trigger: None,
        provisioning: None,
        messages: vec![
            CopilotHookMessage {
                role: SessionRole::User,
                content: "first turn body\nsecond line".to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: None,
            },
            CopilotHookMessage {
                role: SessionRole::Assistant,
                content: "second turn body".to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: None,
            },
        ],
        events: vec![],
        runtime: None,
    };
    config
        .persist_capture(SessionCaptureRequest::copilot(payload))
        .expect("seed session");
}

fn seed_compaction_session(
    config: &SessionStoreConfig,
    session_id: &str,
    agent: &str,
) {
    let payload = CopilotHookPayload {
        session_id: session_id.to_string(),
        workspace_slug: "default".to_string(),
        captured_at: Utc::now(),
        conversation_id: Some("conv-compact".to_string()),
        agent_id: Some(agent.to_string()),
        model: None,
        trigger: None,
        provisioning: None,
        messages: vec![
            CopilotHookMessage {
                role: SessionRole::Tool,
                content: "unchanged status".to_string(),
                tool_name: Some("run_in_terminal".to_string()),
                captured_at: None,
                event_meta: None,
            },
            CopilotHookMessage {
                role: SessionRole::Tool,
                content: "unchanged status".to_string(),
                tool_name: Some("run_in_terminal".to_string()),
                captured_at: None,
                event_meta: None,
            },
            CopilotHookMessage {
                role: SessionRole::Tool,
                content: "Large tool result written to file. Use the read_file tool to access the content at: /tmp/spill.txt".to_string(),
                tool_name: Some("run_in_terminal".to_string()),
                captured_at: None,
                event_meta: None,
            },
            CopilotHookMessage {
                role: SessionRole::Assistant,
                content: "I will retry the same command and check again.".to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: None,
            },
            CopilotHookMessage {
                role: SessionRole::Tool,
                content: format!("inline payload: {}", "x".repeat(800)),
                tool_name: Some("run_in_terminal".to_string()),
                captured_at: None,
                event_meta: None,
            },
        ],
        events: vec![],
        runtime: None,
    };
    config
        .persist_capture(SessionCaptureRequest::copilot(payload))
        .expect("seed compaction session");
}

fn run_machine(args: &[&str]) -> serde_json::Value {
    let cli = parse_cli_from(args).expect("parse cli");
    match run(cli).expect("run command") {
        CliOutput::Machine(value, format) => {
            assert_eq!(format, machine_output_format(true, false).unwrap());
            value
        },
        CliOutput::Text(text) =>
            panic!("expected machine output, got text: {text}"),
    }
}

fn managed_worktree(
    dir: &tempfile::TempDir,
    session_id: &str,
    slug: &str,
    branch: &str,
) -> PathBuf {
    let main_checkout = dir.path();
    assert!(
        Command::new("git")
            .current_dir(main_checkout)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(main_checkout)
            .args(["config", "user.email", "tests@example.invalid"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(main_checkout)
            .args(["config", "user.name", "session-cli tests"])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(main_checkout.join("README.md"), "fixture\n").unwrap();
    assert!(
        Command::new("git")
            .current_dir(main_checkout)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(main_checkout)
            .args(["commit", "--quiet", "-m", "fixture"])
            .status()
            .unwrap()
            .success()
    );

    let path = main_checkout.join(".worktrees").join(session_id).join(slug);
    assert!(
        Command::new("git")
            .current_dir(main_checkout)
            .args(["worktree", "add", "--quiet", "-b", branch])
            .arg(&path)
            .arg("HEAD")
            .status()
            .unwrap()
            .success()
    );
    std::fs::canonicalize(path).unwrap()
}

#[test]
fn check_in_and_lookup_roundtrip() {
    let dir = tempdir().unwrap();
    let store_root: PathBuf = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let worktree = managed_worktree(
        &dir,
        "11111111-1111-4111-8111-111111111111",
        "wt-1",
        "feature/x",
    );
    let worktree_str = worktree.to_string_lossy().to_string();

    let receipt = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "check-in",
        "--session-id",
        "11111111-1111-4111-8111-111111111111",
        "--owner-id",
        "agent-1",
        "--ticket-id",
        "ticket-1",
        "--worktree-path",
        &worktree_str,
        "--branch",
        "feature/x",
    ]);
    assert_eq!(
        receipt["session_id"],
        "11111111-1111-4111-8111-111111111111"
    );
    assert_eq!(receipt["branch"], "feature/x");

    let lookup = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "lookup",
        "--session-id",
        "11111111-1111-4111-8111-111111111111",
    ]);
    assert_eq!(lookup["ticket_id"], "ticket-1");
    assert_eq!(lookup["owner_id"], "agent-1");
}

#[test]
fn query_returns_seeded_session() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let config =
        SessionStoreConfig::new(store_root.clone(), "default".to_string());
    seed_session(&config, "22222222-2222-4222-8222-222222222222", "agent-q");

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
}

#[test]
fn sessions_for_ticket_returns_seeded_session_at_strict_tier() {
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
fn peek_range_and_skeleton() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let config =
        SessionStoreConfig::new(store_root.clone(), "default".to_string());
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
fn terminal_observer_cli_round_trip() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let session_id = "77777777-7777-4777-8777-777777777777";
    let config =
        SessionStoreConfig::new(store_root.clone(), "default".to_string());
    config
        .init_runtime_context(session_api::SessionRuntimeInitRequest {
            session_id: Some(session_id.to_string()),
            ..Default::default()
        })
        .unwrap();

    let created = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "terminal-create",
        "--session-id",
        session_id,
        "--label",
        "human terminal",
    ]);
    let terminal_id = created["terminal_id"].as_str().unwrap().to_string();

    let appended = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "terminal-append-output",
        "--session-id",
        session_id,
        "--terminal-id",
        &terminal_id,
        "--output",
        "human output",
    ]);
    assert_eq!(appended["output"], "human output");

    let peek = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "terminal-peek",
        "--session-id",
        session_id,
        "--terminal-id",
        &terminal_id,
    ]);
    assert_eq!(peek["events"][0]["output"], "human output");

    let closed = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "terminal-close",
        "--session-id",
        session_id,
        "--terminal-id",
        &terminal_id,
    ]);
    assert_eq!(closed["status"], "closed");
}

#[test]
fn peek_prompt_pack_reports_guarded_entries() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let config =
        SessionStoreConfig::new(store_root.clone(), "default".to_string());
    seed_compaction_session(
        &config,
        "55555555-5555-4555-8555-555555555555",
        "agent-c",
    );

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
fn peek_prompt_pack_meets_quantitative_compactness_gate() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let config =
        SessionStoreConfig::new(store_root.clone(), "default".to_string());
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

#[test]
fn workflow_nested_and_flat_forms_are_equivalent() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let workspace_id = "77777777-7777-4777-8777-777777777777";

    run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "init",
        "--session-id",
        workspace_id,
    ]);

    // Canonical nested form.
    run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "workflow",
        "add-node",
        "--session-id",
        workspace_id,
        "--node-id",
        "nested-node",
        "--kind",
        "action",
        "--requirement",
        "optional",
        "--title",
        "added via nested form",
    ]);

    // Flat compatibility alias.
    let after_flat = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "workflow-add-node",
        "--session-id",
        workspace_id,
        "--node-id",
        "flat-node",
        "--kind",
        "action",
        "--requirement",
        "optional",
        "--title",
        "added via flat alias",
    ]);

    let node_ids = after_flat["workflow"]["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .map(|node| node["node_id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(node_ids.contains(&"nested-node".to_string()));
    assert!(node_ids.contains(&"flat-node".to_string()));

    // Both render subcommands resolve through the shared handler.
    let rendered = run_machine(&[
        "session",
        "--json",
        "--store-root",
        &store_root_str,
        "workflow",
        "render-terminal",
        "--session-id",
        workspace_id,
    ]);
    assert!(rendered["render"].as_str().unwrap().contains("nested-node"));
}
