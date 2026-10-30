use chrono::Utc;
use serde_json::Value;
use session_api::{
    CopilotHookMessage, CopilotHookPayload, SessionCaptureRequest, SessionError, SessionRole,
    SessionStoreConfig,
};
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

use super::*;

#[path = "tests/router_parity.rs"]
mod router_parity;

fn seed(config: &SessionStoreConfig, session_id: &str, agent: &str) {
    let payload = CopilotHookPayload {
        session_id: session_id.to_string(),
        workspace_path: "default".to_string(),
        captured_at: Utc::now(),
        conversation_id: None,
        agent_id: Some(agent.to_string()),
        model: None,
        trigger: None,
        provisioning: None,
        messages: vec![CopilotHookMessage {
            role: SessionRole::User,
            content: "alpha body\nbeta".to_string(),
            tool_name: None,
            captured_at: None,
            event_meta: None,
        }],
        events: vec![],
        runtime: None,
    };
    config
        .persist_capture(SessionCaptureRequest::copilot(payload))
        .expect("seed");
}

fn init_checkout_with_worktree(repo_root: &Path, worktree: &Path, branch: &str) {
    std::fs::create_dir_all(repo_root).unwrap();
    assert!(
        std::process::Command::new("git")
            .current_dir(repo_root)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .current_dir(repo_root)
            .args(["config", "user.name", "tests"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .current_dir(repo_root)
            .args(["config", "user.email", "tests@example.invalid"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .current_dir(repo_root)
            .args(["commit", "--quiet", "--allow-empty", "-m", "fixture"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .current_dir(repo_root)
            .args(["worktree", "add", "--quiet", "-b", branch])
            .arg(worktree)
            .status()
            .unwrap()
            .success()
    );
}

fn extract_json(result: rmcp::model::CallToolResult) -> Value {
    let text = result
        .content
        .iter()
        .find_map(|content| {
            if let rmcp::model::RawContent::Text(text) = &content.raw {
                Some(text.text.clone())
            } else {
                None
            }
        })
        .expect("text content");
    serde_json::from_str(&text).expect("parse json")
}

#[tokio::test]
async fn terminal_observer_tools_read_output_without_input_operation() {
    let dir = tempdir().unwrap();
    let workspace_root = dir.path();
    let session_root = workspace_root.join(".workflow-tools/session");
    let workspace = workspace_root.display().to_string();
    let session_id = "88888888-8888-4888-8888-888888888888";
    let server = SessionServer::new(session_root.clone());
    let config = SessionStoreConfig::new(session_root);
    seed(&config, session_id, "agent-terminal");
    config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(session_id.to_string()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap();

    let created = server
        .session_terminal_create(Parameters(TerminalCreateInput {
            workspace: workspace.clone(),
            session_id: session_id.to_string(),
            label: "human terminal".to_string(),
            cwd: Some(dir.path().to_path_buf()),
        }))
        .await
        .expect("create observer");
    let created_payload = extract_json(created);
    let terminal_id = created_payload["terminal_id"]
        .as_str()
        .expect("terminal id")
        .to_string();
    config
        .append_terminal_output(session_id, &terminal_id, "human output\n".to_string())
        .unwrap();

    let peek = server
        .session_terminal_peek(Parameters(TerminalPeekInput {
            workspace: workspace.clone(),
            session_id: session_id.to_string(),
            terminal_id: terminal_id.clone(),
            offset: None,
            limit: None,
        }))
        .await
        .expect("peek observer");
    let peek_payload = extract_json(peek);
    assert_eq!(peek_payload["events"][0]["output"], "human output\n");

    let closed = server
        .session_terminal_close(Parameters(TerminalStatusInput {
            workspace,
            session_id: session_id.to_string(),
            terminal_id,
        }))
        .await
        .expect("close observer");
    let closed_payload = extract_json(closed);
    assert_eq!(closed_payload["status"], "closed");
}

#[tokio::test]
async fn handoff_validates_target_file_in_selected_worktree() {
    let dir = tempdir().unwrap();
    let workspace_root = dir.path();
    let main_store = workspace_root.join(".workflow-tools/session");
    let session_id = "99999999-9999-4999-8999-999999999999";
    let worktree = workspace_root
        .join(".worktrees")
        .join(session_id)
        .join("handoff");
    init_checkout_with_worktree(workspace_root, &worktree, "agent/handoff-worktree");
    let target = worktree.join("handoff-fixtures/assigned-only.txt");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "assigned worktree target").unwrap();
    let workspace = workspace_root.display().to_string();
    let server = SessionServer::new(main_store);

    server
        .session_check_in(Parameters(CheckInInput {
            workspace: workspace.clone(),
            session_id: session_id.to_string(),
            owner_id: "agent".to_string(),
            ticket_id: "ticket-handoff".to_string(),
            worktree_path: worktree.display().to_string(),
            branch: "agent/handoff-worktree".to_string(),
            predecessor_session_id: None,
        }))
        .await
        .expect("check in assigned worktree");
    server
        .session_runtime_init(Parameters(RuntimeInitInput {
            workspace: workspace.clone(),
            session_id: session_id.to_string(),
            predecessor_run_id: None,
            force_new_run: false,
        }))
        .await
        .expect("initialize worktree runtime");

    let handoff = server
        .session_handoff(Parameters(RuntimeHandoffInput {
            workspace,
            session_id: session_id.to_string(),
            validation: vec![],
            objective: "Validate session worktree paths".to_string(),
            target_tickets: vec![],
            higher_level_objective: String::new(),
            upward_context: vec![],
            target_files: vec!["handoff-fixtures/assigned-only.txt".to_string()],
            decisions: vec![],
            non_goals: vec![],
            context_anchors: vec![],
            open_escalations: vec!["Regression package".to_string()],
            risk_notes: None,
            predecessor_handoff: None,
        }))
        .await
        .expect("handoff path in selected worktree");
    let payload = extract_json(handoff);
    assert_eq!(
        payload["record"]["target_files"][0],
        "handoff-fixtures/assigned-only.txt"
    );
}

#[tokio::test]
async fn sessions_for_ticket_returns_matches_at_requested_tier() {
    let dir = tempdir().unwrap();
    let workspace_root = dir.path();
    let store_root = workspace_root.join(".workflow-tools/session");
    let session_id = "33333333-3333-4333-8333-333333333333";
    let worktree = workspace_root
        .join(".worktrees")
        .join(session_id)
        .join("ticket");
    init_checkout_with_worktree(workspace_root, &worktree, "feature/ticket-mcp");
    let server = SessionServer::new(store_root.clone());

    server
        .session_check_in(Parameters(CheckInInput {
            workspace: workspace_root.display().to_string(),
            session_id: session_id.to_string(),
            owner_id: "agent-ticket".to_string(),
            ticket_id: "ticket-mcp".to_string(),
            worktree_path: worktree.to_string_lossy().to_string(),
            branch: "feature/ticket-mcp".to_string(),
            predecessor_session_id: None,
        }))
        .await
        .expect("check-in");

    let result = server
        .session_sessions_for_ticket(Parameters(SessionsForTicketInput {
            ticket_id: "ticket-mcp".to_string(),
            strength: "strict".to_string(),
        }))
        .await
        .expect("sessions-for-ticket");
    assert!(!result.is_error.unwrap_or(false));
    let payload = extract_json(result);
    assert_eq!(payload["count"], 1);
    assert_eq!(payload["sessions"][0]["session_id"], session_id);
    assert_eq!(payload["sessions"][0]["matched_strength"], "strict");

    let unrelated = server
        .session_sessions_for_ticket(Parameters(SessionsForTicketInput {
            ticket_id: "ticket-other".to_string(),
            strength: "mentioned".to_string(),
        }))
        .await
        .expect("sessions-for-ticket unrelated");
    let unrelated_payload = extract_json(unrelated);
    assert_eq!(unrelated_payload["count"], 0);
}

#[tokio::test]
async fn runtime_render_instructions_returns_only_pinned_rules() {
    let dir = tempdir().unwrap();
    let workspace_root = dir.path();
    let session_root = workspace_root.join(".workflow-tools/session");
    let config = SessionStoreConfig::new(&session_root);
    seed(
        &config,
        "11111111-1111-4111-8111-111111111111",
        "agent-runtime",
    );
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .expect("init runtime");
    let mut rule_store =
        rule_api::RuleStore::open_or_init(&dir.path().join(".rule")).expect("rule store");
    let rule = rule_api::RuleManifest::new(
        "session/mcp/render",
        "MCP render",
        ".instructions",
        "mcp-render",
        "Pinned MCP guidance.",
    );
    let rule_id = rule_store.create(&rule, None).expect("create rule");
    config
        .pin_runtime_entity(
            &init.context.session_id,
            &format!("ce://default/rules/{rule_id}"),
            None,
            None,
        )
        .expect("pin rule");
    let server = SessionServer::new(session_root.clone());

    let result = server
        .session_runtime_render_instructions(Parameters(RuntimeViewInput {
            workspace: workspace_root.display().to_string(),
            session_id: init.context.session_id,
        }))
        .await
        .expect("render instructions");
    let payload = extract_json(result);
    assert!(
        payload["render"]
            .as_str()
            .unwrap()
            .contains("Pinned MCP guidance.")
    );
}

#[tokio::test]
async fn query_and_peek() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".session");
    let server = SessionServer::new(store_root.clone());
    let config = SessionStoreConfig::new(store_root.clone());
    seed(&config, "22222222-2222-4222-8222-222222222222", "agent-2");
    let unreadable_id = "f3333333-3333-4333-8333-333333333333";
    let unreadable_dir = store_root.join("sessions").join(unreadable_id);
    std::fs::create_dir_all(&unreadable_dir).unwrap();
    std::fs::write(
        unreadable_dir.join("session.json"),
        b"private malformed session payload",
    )
    .unwrap();

    let query = server
        .session_query(Parameters(QueryInput {
            session_id_prefix: None,
            conversation_id: None,
            agent_id: Some("agent-2".to_string()),
            text: None,
            limit: None,
        }))
        .await
        .expect("query");
    assert!(!query.is_error.unwrap_or(false));
    let payload = extract_json(query);
    assert_eq!(payload["count"], 1);
    assert_eq!(
        payload["sessions"][0]["session_id"],
        "22222222-2222-4222-8222-222222222222"
    );
    assert_eq!(payload["diagnostics"][0]["session_id"], unreadable_id);
    assert_eq!(
        payload["diagnostics"][0]["code"],
        "unreadable_session_record"
    );
    assert!(
        !payload
            .to_string()
            .contains("private malformed session payload")
    );
    assert!(
        !payload
            .to_string()
            .contains(store_root.to_string_lossy().as_ref())
    );

    let skeleton = server
        .session_peek_skeleton(Parameters(PeekSkeletonInput {
            session_id: "22222222-2222-4222-8222-222222222222".to_string(),
            preview_chars: None,
        }))
        .await
        .expect("skeleton");
    assert!(!skeleton.is_error.unwrap_or(false));
}

// ── T-HANDLE: init/resume + workflow tools echo session_id ──────

#[tokio::test]
async fn runtime_init_result_exposes_session_id_top_line() {
    let dir = tempdir().unwrap();
    let session_root = dir.path().join(".session");
    let server = SessionServer::new(session_root.clone());

    let result = server
        .session_runtime_init(Parameters(RuntimeInitInput {
            workspace: session_root.display().to_string(),
            session_id: "22222222-2222-4222-8222-222222222222".to_string(),
            predecessor_run_id: None,
            force_new_run: false,
        }))
        .await
        .expect("runtime init");
    let payload = extract_json(result);
    let handle = payload["session_id"].as_str().expect("top-line session_id");
    assert!(!handle.is_empty());
    // The handle matches the nested context handle (no drift).
    assert_eq!(payload["context"]["session_id"].as_str(), Some(handle));

    // A subsequent workflow call echoes the same handle in its result.
    let node = server
        .session_workflow_add_node(Parameters(WorkflowAddNodeInput {
            workspace: session_root.display().to_string(),
            session_id: handle.to_string(),
            node_id: Some("n1".to_string()),
            kind: "task".to_string(),
            requirement: "optional".to_string(),
            title: "descriptive".to_string(),
            ticket_urn: None,
            spec_urn: None,
            anchor_urn: None,
            category: Some("note".to_string()),
            cached_ticket_title: None,
            validation_spec_id: None,
        }))
        .await
        .expect("add node");
    let node_payload = extract_json(node);
    assert_eq!(node_payload["session_id"].as_str(), Some(handle));
}

#[path = "tests/workflow_schema.rs"]
mod workflow_schema;
#[allow(unused_imports)]
pub use workflow_schema::*;

#[path = "tests/workspace_routing.rs"]
mod workspace_routing;
#[allow(unused_imports)]
pub use workspace_routing::*;

#[path = "tests/moves.rs"]
mod moves;
#[allow(unused_imports)]
pub use moves::*;
