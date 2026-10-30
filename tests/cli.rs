use std::{path::PathBuf, process::Command};

use chrono::Utc;
use tempfile::tempdir;

use session::cli::{CliOutput, machine_output_format, parse_cli_from, run};
use session_api::{
    CopilotHookMessage, CopilotHookPayload, SessionCaptureRequest, SessionRole, SessionStoreConfig,
};

fn seed_session(config: &SessionStoreConfig, session_id: &str, agent: &str) {
    let payload = CopilotHookPayload {
        session_id: session_id.to_string(),
        workspace_path: "default".to_string(),
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

fn seed_compaction_session(config: &SessionStoreConfig, session_id: &str, agent: &str) {
    let payload = CopilotHookPayload {
        session_id: session_id.to_string(),
        workspace_path: "default".to_string(),
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
        }
        CliOutput::Text(text) => panic!("expected machine output, got text: {text}"),
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
fn unassign_deleted_checkout_roundtrip() {
    let dir = tempdir().unwrap();
    let session_id = "11111111-1111-4111-8111-111111111111";
    let worktree = managed_worktree(&dir, session_id, "unassign", "feature/unassign");
    let config = SessionStoreConfig::new(dir.path().join(".workflow-tools/session"));
    config
        .check_in_worktree(session_api::SessionWorktreeCheckInRequest {
            session_id: session_id.to_string(),
            owner_id: "agent".to_string(),
            ticket_id: "ticket".to_string(),
            worktree_path: worktree.clone(),
            branch: "feature/unassign".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();
    std::fs::remove_dir_all(&worktree).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_session"))
        .args(["--json", "--workspace"])
        .arg(dir.path())
        .args([
            "unassign",
            "--session-id",
            session_id,
            "--owner-id",
            "agent",
            "--ticket-id",
            "ticket",
            "--expected-worktree-path",
        ])
        .arg(&worktree)
        .args([
            "--expected-branch",
            "feature/unassign",
            "--reason",
            "Return to main",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["session_id"], session_id);
    assert_eq!(
        receipt["main_checkout"],
        dir.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert!(PathBuf::from(receipt["audit_path"].as_str().unwrap()).is_file());
    assert!(config.registered_worktree(session_id).unwrap().is_none());
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
fn init_with_dot_workspace_reads_back_from_canonical_store() {
    let dir = tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session_id = "11111111-1111-4111-8111-111111111111";

    let output = Command::new(env!("CARGO_BIN_EXE_session"))
        .current_dir(&workspace)
        .args([
            "--json",
            "--workspace",
            ".",
            "init",
            "--session-id",
            session_id,
        ])
        .output()
        .expect("run Session CLI init");
    assert!(
        output.status.success(),
        "Session CLI init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store_root = workspace.join(".workflow-tools").join("session");
    let read_back = SessionStoreConfig::new(store_root.clone())
        .view_runtime_context(session_id)
        .expect("read initialized runtime context");
    assert_eq!(read_back.session_id, session_id);
    assert!(store_root.is_dir());
    assert!(!workspace.join(".session").exists());
    assert!(!dir.path().join(".workflow-tools").join("session").exists());
}

#[test]
fn terminal_observer_cli_round_trip() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let store_root_str = store_root.to_string_lossy().to_string();
    let session_id = "77777777-7777-4777-8777-777777777777";
    let config = SessionStoreConfig::new(store_root.clone());
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

#[path = "cli/transcript_queries.rs"]
mod transcript_queries;
#[allow(unused_imports)]
pub use transcript_queries::*;
