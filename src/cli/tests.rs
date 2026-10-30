use super::*;
use chrono::Utc;
use session_api::{
    CopilotHookMessage, CopilotHookPayload, SessionCaptureRequest, SessionRole,
    SessionRuntimeInitRequest,
};
use std::process::Command;
use tempfile::tempdir;

#[test]
fn unassign_requires_explicit_workspace_and_all_preconditions() {
    let args = [
        "session",
        "unassign",
        "--session-id",
        "11111111-1111-4111-8111-111111111111",
        "--owner-id",
        "agent",
        "--ticket-id",
        "ticket",
        "--expected-worktree-path",
        "/repo/.worktrees/session/deleted",
        "--expected-branch",
        "feature/x",
        "--reason",
        "Return to main",
    ];
    let cli = parse_cli_from(args).unwrap();
    assert!(matches!(cli.command, SessionCommand::Unassign(_)));
    assert!(matches!(run(cli), Err(CliRunError::BadRequest(_))));
    assert!(parse_cli_from(&args[..args.len() - 2]).is_err());
}

#[test]
fn parses_check_in_command() {
    let cli = parse_cli_from([
        "session",
        "check-in",
        "--session-id",
        "11111111-1111-4111-8111-111111111111",
        "--owner-id",
        "agent-1",
        "--ticket-id",
        "ticket-1",
        "--worktree-path",
        "/repo/wt",
        "--branch",
        "feature/x",
    ])
    .expect("parse check-in");

    assert!(cli.workspace_root.is_none());
    match cli.command {
        SessionCommand::CheckIn(args) => {
            assert_eq!(args.session_id, "11111111-1111-4111-8111-111111111111");
            assert_eq!(args.branch, "feature/x");
            assert!(args.predecessor_session_id.is_none());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn check_in_rejects_ambient_workspace_aliases() {
    for selector in ["", "  ", "default", ".."] {
        let mut cli = parse_cli_from([
            "session",
            "--workspace",
            ".",
            "check-in",
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "--owner-id",
            "agent-1",
            "--ticket-id",
            "ticket-1",
            "--worktree-path",
            "/repo/wt",
            "--branch",
            "feature/x",
        ])
        .expect("parse check-in");
        cli.workspace_root = Some(PathBuf::from(selector));

        match run(cli) {
            Err(CliRunError::BadRequest(message)) => {
                assert!(message.contains("requires an explicit workspace path"));
            }
            _ => panic!("ambient workspace selector must be rejected"),
        }
    }
}

#[test]
fn init_requires_explicit_workspace_or_store_root() {
    let cli = parse_cli_from([
        "session",
        "init",
        "--session-id",
        "11111111-1111-4111-8111-111111111111",
    ])
    .expect("parse init");

    match run(cli) {
        Err(CliRunError::BadRequest(message)) => {
            assert!(message.contains("writes require explicit --workspace"));
        }
        _ => panic!("init must reject an ambient workspace before writing"),
    }
}

#[test]
fn parses_peek_range_defaults() {
    let cli = parse_cli_from([
        "session",
        "peek-range",
        "--session-id",
        "11111111-1111-4111-8111-111111111111",
    ])
    .expect("parse peek-range");

    match cli.command {
        SessionCommand::PeekRange(args) => {
            assert_eq!(args.start, 0);
            assert!(args.end.is_none());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn json_and_toon_conflict() {
    let result = parse_cli_from([
        "session",
        "--json",
        "--toon",
        "lookup",
        "--session-id",
        "11111111-1111-4111-8111-111111111111",
    ]);
    assert!(result.is_err());
}

#[test]
fn parses_move_command() {
    let cli = parse_cli_from([
        "session",
        "move",
        "7b3a7c62-1f3f-45d6-b8a1-f2b83e3d9f71",
        "--to-workspace-root",
        "/repo/target",
    ])
    .expect("parse move");

    match cli.command {
        SessionCommand::Move(args) => {
            assert_eq!(
                args.id.as_deref(),
                Some("7b3a7c62-1f3f-45d6-b8a1-f2b83e3d9f71")
            );
            assert_eq!(args.to_workspace_root, Some(PathBuf::from("/repo/target")));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn move_rejects_ambient_workspace_aliases() {
    let temp = tempdir().unwrap();
    let missing_source = temp.path().join("not-created-source");

    for selector in ["", "  ", "default", ".."] {
        let mut cli = parse_cli_from([
            "session",
            "move",
            "7b3a7c62-1f3f-45d6-b8a1-f2b83e3d9f71",
            "--to-workspace-root",
            ".",
            "--store-root",
            missing_source.to_string_lossy().as_ref(),
        ])
        .expect("parse move");
        if let SessionCommand::Move(args) = &mut cli.command {
            args.to_workspace_root = Some(PathBuf::from(selector));
        }

        assert!(run(cli).is_err(), "selector {selector:?} must be rejected");
        assert!(!missing_source.exists());
    }
}

#[test]
fn parses_sessions_for_ticket_command() {
    let cli = parse_cli_from([
        "session",
        "sessions-for-ticket",
        "ticket-abc",
        "--strength",
        "linked",
    ])
    .expect("parse sessions-for-ticket");

    match cli.command {
        SessionCommand::SessionsForTicket(args) => {
            assert_eq!(args.ticket_id, "ticket-abc");
            assert_eq!(args.strength, "linked");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn move_roundtrip_executes_against_target_workspace() {
    let temp = tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    Command::new("git")
        .current_dir(&repo_root)
        .args(["init"])
        .status()
        .expect("git init")
        .success()
        .then_some(())
        .expect("git init failed");

    let source_store_root = repo_root.join(".workflow-tools/session");
    std::fs::create_dir_all(&source_store_root).unwrap();
    let target_workspace_root = repo_root.join("target-workspace");
    std::fs::create_dir_all(target_workspace_root.join(".workflow-tools/session")).unwrap();

    let session_id = "7b3a7c62-1f3f-45d6-b8a1-f2b83e3d9f71";
    let config = SessionStoreConfig::new(source_store_root.clone());
    let payload = CopilotHookPayload {
        session_id: session_id.to_string(),
        workspace_path: "default".to_string(),
        captured_at: Utc::now(),
        conversation_id: Some("conv-1".to_string()),
        agent_id: Some("agent-1".to_string()),
        model: None,
        trigger: None,
        provisioning: None,
        messages: vec![CopilotHookMessage {
            role: SessionRole::User,
            content: "move me".to_string(),
            tool_name: None,
            captured_at: None,
            event_meta: None,
        }],
        events: vec![],
        runtime: None,
    };
    config
        .persist_capture(SessionCaptureRequest::copilot(payload))
        .expect("seed session");

    let cli = parse_cli_from([
        "session",
        "--json",
        "--workspace",
        repo_root.to_string_lossy().as_ref(),
        "move",
        session_id,
        "--source-store-root",
        source_store_root.to_string_lossy().as_ref(),
        "--to-workspace-root",
        target_workspace_root.to_string_lossy().as_ref(),
    ])
    .expect("parse move");

    match run(cli).expect("run move") {
        CliOutput::Machine(value, _) => {
            assert_eq!(value["status"], "ok");
            assert_eq!(value["mode"], "execute");
            assert!(value["outcome"]["journal"]["id"].is_string());
        }
        other => panic!("unexpected output: {other:?}"),
    }

    let target_config =
        SessionStoreConfig::new(target_workspace_root.join(".workflow-tools/session"));
    assert!(matches!(
        config.read_session(session_id),
        Err(SessionError::NotFound { .. })
    ));
    assert_eq!(
        target_config.read_session(session_id).unwrap().session_id,
        session_id
    );
}

#[test]
fn handoff_reference_completeness_includes_durable_identity_fields() {
    let temp = tempdir().unwrap();
    let config = SessionStoreConfig::new(temp.path().join(".session"));

    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some("33333333-3333-4333-8333-333333333333".to_string()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .expect("init runtime context");
    let session_id = init.context.session_id;

    let rendered = config
        .render_handoff_terminal(
            &session_id,
            None,
            vec![session_api::SessionValidationGate {
                validation_spec_id: "val-handoff-reference-completeness".to_string(),
                required: true,
                outcome: Some("passed".to_string()),
                command: None,
            }],
            None,
        )
        .expect("render handoff");

    assert!(rendered.contains("session_id:"));
    assert!(rendered.contains("outgoing_run_id:"));
    assert!(rendered.contains("handoff "));
    assert!(rendered.contains("resume: session-cli resume --session-id"));
}

#[path = "tests/workflow.rs"]
mod workflow;
#[allow(unused_imports)]
pub use workflow::*;
