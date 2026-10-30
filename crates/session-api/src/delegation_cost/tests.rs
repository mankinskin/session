use super::*;
use crate::{SessionLinks, SessionMetadata, SessionTurn, SessionTurnEventMeta};
use chrono::Utc;

fn base_meta() -> SessionTurnEventMeta {
    SessionTurnEventMeta::default()
}

fn record_with_turns(turns: Vec<SessionTurn>) -> SessionRecord {
    SessionRecord {
        schema_version: 1,
        session_id: "sess-1".to_string(),
        source: "test".to_string(),
        started_at: Utc::now(),
        captured_at: Utc::now(),
        metadata: SessionMetadata {
            workspace_path: "test".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: None,
            trigger: None,
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree: None,
        },
        turns,
        links: SessionLinks::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: Vec::new(),
        picked_up_handoff_ids: Vec::new(),
    }
}

fn tool_turn(
    sequence: usize,
    tool_name: &str,
    run_id: Option<&str>,
    args: serde_json::Value,
) -> SessionTurn {
    SessionTurn {
        sequence,
        role: SessionRole::Tool,
        content: "ok".to_string(),
        captured_at: Utc::now(),
        tool_name: Some(tool_name.to_string()),
        model: None,
        event_meta: Some(SessionTurnEventMeta {
            tool_success: Some(true),
            tool_arguments_json: Some(args),
            subagent_run_id: run_id.map(String::from),
            ..base_meta()
        }),
    }
}

#[test]
fn normalizes_backslash_and_drive_letter_case_for_dedup() {
    assert_eq!(
        normalize_path_for_dedup("C:\\foo\\bar.md"),
        normalize_path_for_dedup("c:/foo/bar.md")
    );
    assert_eq!(normalize_path_for_dedup("c:/foo/bar.md"), "c:/foo/bar.md");
}

#[test]
fn parallel_spans_are_attributed_without_double_counting() {
    // Two sub-agent spans dispatched in parallel: their tool calls
    // interleave in the flat transcript, but each turn's own
    // subagent_run_id (stamped from parent_event_id ancestry, not
    // index-range overlap) unambiguously identifies its owner.
    let turns = vec![
        tool_turn(
            0,
            "runSubagent",
            None,
            serde_json::json!({"agentName": "Explore", "description": "probe A"}),
        ),
        tool_turn(
            1,
            "runSubagent",
            None,
            serde_json::json!({"agentName": "Explore", "description": "probe B"}),
        ),
        tool_turn(
            2,
            "read_file",
            Some("call-a"),
            serde_json::json!({"filePath": "x.rs"}),
        ),
        tool_turn(
            3,
            "read_file",
            Some("call-b"),
            serde_json::json!({"filePath": "y.rs"}),
        ),
        tool_turn(
            4,
            "read_file",
            Some("call-a"),
            serde_json::json!({"filePath": "z.rs"}),
        ),
    ];
    // Re-key the wrapper turns' own tool_call_id so agent_info resolves.
    let mut turns = turns;
    turns[0].event_meta.as_mut().unwrap().tool_call_id = Some("call-a".to_string());
    turns[1].event_meta.as_mut().unwrap().tool_call_id = Some("call-b".to_string());

    let record = record_with_turns(turns);
    let report = compute_delegation_cost_report(&record);

    assert_eq!(report.subagent_count, 2);
    let call_a = report
        .subagents
        .iter()
        .find(|s| s.run_id == "call-a")
        .expect("call-a span");
    let call_b = report
        .subagents
        .iter()
        .find(|s| s.run_id == "call-b")
        .expect("call-b span");
    assert_eq!(call_a.tool_call_count, 2);
    assert_eq!(call_b.tool_call_count, 1);
    // No tool call is double-counted or dropped: 3 real reads total.
    assert_eq!(
        call_a.tool_call_count + call_b.tool_call_count + report.parent_tool_call_count,
        3
    );
}

#[test]
fn duplicate_reads_are_path_normalization_safe() {
    let turns = vec![
        tool_turn(
            0,
            "read_file",
            Some("call-a"),
            serde_json::json!({"filePath": "C:\\repo\\notes.md"}),
        ),
        tool_turn(
            1,
            "read_file",
            Some("call-b"),
            serde_json::json!({"filePath": "c:/repo/notes.md"}),
        ),
    ];
    let record = record_with_turns(turns);
    let report = compute_delegation_cost_report(&record);

    assert_eq!(report.cross_agent_duplicate_reads.len(), 1);
    let dup = &report.cross_agent_duplicate_reads[0];
    assert_eq!(dup.agent_count, 2);
    assert_eq!(dup.total_count, 2);
    assert_eq!(dup.key, "c:/repo/notes.md");
}

#[test]
fn failures_are_attributed_to_the_owning_span() {
    let mut turn = tool_turn(
        0,
        "run_in_terminal",
        Some("call-a"),
        serde_json::json!({"command": "cargo test"}),
    );
    turn.event_meta.as_mut().unwrap().tool_success = Some(false);
    turn.event_meta.as_mut().unwrap().result_code = Some("error".to_string());
    turn.event_meta.as_mut().unwrap().error_message = Some("compile error".to_string());

    let record = record_with_turns(vec![turn]);
    let report = compute_delegation_cost_report(&record);

    let span = &report.subagents[0];
    assert_eq!(span.failures.len(), 1);
    assert_eq!(span.failures[0].summary, "compile error");
}

#[test]
fn real_token_and_cost_totals_flow_per_span() {
    let mut assistant_turn = SessionTurn {
        sequence: 0,
        role: SessionRole::Assistant,
        content: "work".to_string(),
        captured_at: Utc::now(),
        tool_name: None,
        model: Some("gpt-5".to_string()),
        event_meta: Some(SessionTurnEventMeta {
            input_tokens: Some(1000),
            output_tokens: Some(200),
            cost_usd: Some(0.05),
            subagent_run_id: Some("call-a".to_string()),
            ..base_meta()
        }),
    };
    assistant_turn.event_meta.as_mut().unwrap().model_id = Some("gpt-5".to_string());

    let record = record_with_turns(vec![assistant_turn]);
    let report = compute_delegation_cost_report(&record);

    assert_eq!(report.subagent_count, 1);
    let span = &report.subagents[0];
    assert_eq!(span.input_tokens, 1000);
    assert_eq!(span.output_tokens, 200);
    assert!((span.cost_usd.unwrap() - 0.05).abs() < 1e-9);
}

// -- classify_shell_command taxonomy (ticket 77eb143b AC1) --

#[test]
fn classifies_read_like_exploratory_heads() {
    assert_eq!(
        classify_shell_command("grep -n foo bar.rs"),
        ShellCommandCategory::ReadLikeExploratory
    );
    assert_eq!(
        classify_shell_command("ls -la .spec/specs/"),
        ShellCommandCategory::ReadLikeExploratory
    );
    assert_eq!(
        classify_shell_command("rtk cat foo.txt"),
        ShellCommandCategory::ReadLikeExploratory
    );
}

#[test]
fn classifies_cli_shadowing_mcp_bare_and_path_qualified() {
    assert_eq!(
        classify_shell_command("ticket create --type defect --title x"),
        ShellCommandCategory::CliShadowingMcp
    );
    assert_eq!(
        classify_shell_command("spec get 7be68a48 --full 2>&1"),
        ShellCommandCategory::CliShadowingMcp
    );
    assert_eq!(
        classify_shell_command("./target/debug/ticket.exe board check-in bd5e9aee"),
        ShellCommandCategory::CliShadowingMcp
    );
    assert_eq!(
        classify_shell_command(
            "rtk c:/Users/linus/git/graph_app/context-engine/target/release/test.exe list-specs --workspace default"
        ),
        ShellCommandCategory::CliShadowingMcp
    );
}

#[test]
fn strips_leading_cd_chain_before_classifying() {
    assert_eq!(
        classify_shell_command(
            "cd /c/Users/linus/git/graph_app/context-engine && ./target/debug/spec.exe health --all"
        ),
        ShellCommandCategory::CliShadowingMcp
    );
}

/// Regression test for ticket `77eb143b` defect 1: a `cd`-chained
/// read-like command must classify as `ReadLikeExploratory`, not fall
/// through to `Other`. This exact command shape (`cd <dir> && grep ...
/// | head ...`) is drawn from the `3e9bc20b` baseline session log
/// (`.session/sessions/3e9bc20b-4fe8-4996-ae7f-7be32525e429/events.json`,
/// event `3fc6d1d5-1e14-4195-9d76-cf0ded4fa8ed`), one of the 13
/// misclassified commands the bug caused across the two baseline
/// sessions before this fix.
#[test]
fn cd_chained_read_like_command_classifies_as_read_like_not_other() {
    assert_eq!(
        classify_shell_command(
            "cd .session/sessions/aaf84892-09e4-4be6-be4c-b360b140b15e && grep -A 30 '### **validation**' transcript.json | head -40"
        ),
        ShellCommandCategory::ReadLikeExploratory
    );
    // Semicolon-separated cd chain, read-like via `cat`.
    assert_eq!(
        classify_shell_command("cd /repo; cat file.md"),
        ShellCommandCategory::ReadLikeExploratory
    );
}

#[test]
fn bare_test_is_shell_builtin_not_test_cli_unless_known_subcommand() {
    // `test -f`/`test -d`/`test !` are the POSIX shell builtin, not the
    // test-cli binary, and must not be miscounted as cli_shadowing_mcp.
    assert_eq!(
        classify_shell_command("test -f .spec/specs/x/body.md && echo exists"),
        ShellCommandCategory::Other
    );
    assert_eq!(
        classify_shell_command("test ! -f tmp/scratch.md && echo gone"),
        ShellCommandCategory::Other
    );
    // A bare `test` invocation with a known test-cli subcommand is the
    // real CLI shadowing mcp_test-mcp_.
    assert_eq!(
        classify_shell_command("test list-specs --workspace default"),
        ShellCommandCategory::CliShadowingMcp
    );
}

#[test]
fn classifies_cargo_run_cli_vs_legitimate_cargo() {
    assert_eq!(
        classify_shell_command("cargo run -p test-cli -- record --id foo"),
        ShellCommandCategory::CargoRunCli
    );
    assert_eq!(
        classify_shell_command("cargo run --release --package spec-cli -- get 123"),
        ShellCommandCategory::CargoRunCli
    );
    assert_eq!(
        classify_shell_command("cargo build -p session-api"),
        ShellCommandCategory::LegitimateDev
    );
    assert_eq!(
        classify_shell_command("rtk cargo test -p session-api"),
        ShellCommandCategory::LegitimateDev
    );
}

#[test]
fn classifies_git_as_legitimate_dev() {
    assert_eq!(
        classify_shell_command("git status"),
        ShellCommandCategory::LegitimateDev
    );
    assert_eq!(
        classify_shell_command("rtk git log --oneline -5"),
        ShellCommandCategory::LegitimateDev
    );
}

#[test]
fn classifies_unmatched_commands_as_other() {
    assert_eq!(
        classify_shell_command("python3 -c \"print(1)\""),
        ShellCommandCategory::Other
    );
    assert_eq!(
        classify_shell_command("mkdir -p .spec/specs/x"),
        ShellCommandCategory::Other
    );
}

#[test]
fn shell_command_categories_partition_every_terminal_call() {
    let turns = vec![
        tool_turn(
            0,
            "runSubagent",
            None,
            serde_json::json!({"agentName": "Implement Agent", "description": "d"}),
        ),
        tool_turn(
            1,
            "run_in_terminal",
            Some("run-1"),
            serde_json::json!({"command": "grep -n foo bar.rs"}),
        ),
        tool_turn(
            2,
            "run_in_terminal",
            Some("run-1"),
            serde_json::json!({"command": "./target/debug/ticket.exe get abc"}),
        ),
        tool_turn(
            3,
            "run_in_terminal",
            Some("run-1"),
            serde_json::json!({"command": "cargo run -p test-cli -- record"}),
        ),
        tool_turn(
            4,
            "run_in_terminal",
            Some("run-1"),
            serde_json::json!({"command": "cargo build -p session-api"}),
        ),
        tool_turn(
            5,
            "run_in_terminal",
            Some("run-1"),
            serde_json::json!({"command": "python3 -c \"print(1)\""}),
        ),
    ];
    let mut turns = turns;
    turns[0].event_meta.as_mut().unwrap().tool_call_id = Some("run-1".to_string());

    let record = record_with_turns(turns);
    let report = compute_delegation_cost_report(&record);

    let span = &report.subagents[0];
    assert_eq!(
        span.shell_command_categories.get("read_like_exploratory"),
        Some(&1)
    );
    assert_eq!(
        span.shell_command_categories.get("cli_shadowing_mcp"),
        Some(&1)
    );
    assert_eq!(span.shell_command_categories.get("cargo_run_cli"), Some(&1));
    assert_eq!(
        span.shell_command_categories.get("legitimate_dev"),
        Some(&1)
    );
    assert_eq!(span.shell_command_categories.get("other"), Some(&1));
    let total: u64 = report.shell_command_categories.values().sum();
    assert_eq!(total, 5);
    assert_eq!(report.mcp_tool_discovery_failure_count, 1);
    assert_eq!(report.mcp_tool_failure_fallback_count, 0);
}
