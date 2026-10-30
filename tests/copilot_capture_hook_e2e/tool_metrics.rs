#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn e2e_hook_binary_populates_tool_metrics_from_captured_tool_events() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path = write_fixture_transcript(
        fixture_dir.path(),
        "fixture-tool-calls.jsonl",
        TOOL_CALL_TRANSCRIPT,
    );

    let store_dir = tempdir().expect("tempdir");
    let store_root = store_dir.path().join("memory-api-store");
    fs::create_dir_all(&store_root).expect("create temp store root");

    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let output = Command::new(hook_bin)
        .env("MCP_MAIN_CHECKOUT", fixture_dir.path())
        .arg("--transcript-path")
        .arg(&transcript_path)
        .arg("--store-root")
        .arg(&store_root)
        .arg("--trigger")
        .arg("Stop")
        .output()
        .expect("run session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let tool_metrics_path = store_root
        .join("sessions")
        .join("fixture-tool-calls")
        .join("tool-metrics.json");
    let raw = fs::read_to_string(&tool_metrics_path)
        .expect("tool-metrics.json should exist once a tool call was captured");
    let summary: serde_json::Value =
        serde_json::from_str(&raw).expect("tool-metrics.json should be valid json");

    let grep = &summary["tools"]["grep_search"];
    assert_eq!(grep["call_count"], 1);
    assert_eq!(grep["success_count"], 1);
    assert_eq!(
        grep["duration_ms_values"][0], 1500,
        "duration is derived from the start/complete bracket"
    );
}

#[test]
pub(super) fn e2e_val_session_api_tool_metrics_gate_asserts_nonempty_tools_map() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path = write_fixture_transcript(
        fixture_dir.path(),
        "fixture-tool-metrics-gate.jsonl",
        PRODUCER_SHAPED_TOOL_TRANSCRIPT,
    );

    let store_dir = tempdir().expect("tempdir");
    let store_root = store_dir.path().join("memory-api-store");
    fs::create_dir_all(&store_root).expect("create temp store root");

    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let output = Command::new(hook_bin)
        .env("MCP_MAIN_CHECKOUT", fixture_dir.path())
        .arg("--transcript-path")
        .arg(&transcript_path)
        .arg("--store-root")
        .arg(&store_root)
        .arg("--trigger")
        .arg("Stop")
        .output()
        .expect("run session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let tool_metrics_path = store_root
        .join("sessions")
        .join("fixture-tool-metrics-gate")
        .join("tool-metrics.json");
    let raw = fs::read_to_string(&tool_metrics_path).expect(
        "tool-metrics.json should exist once a tool call was captured from \
         a producer-shaped transcript",
    );
    let summary: serde_json::Value =
        serde_json::from_str(&raw).expect("tool-metrics.json should be valid json");

    let tools = summary["tools"]
        .as_object()
        .expect("tools field should be a json object");
    assert!(
        !tools.is_empty(),
        "tools map in {} must be non-empty for a transcript with captured \
         tool execution events; got: {raw}",
        tool_metrics_path.display()
    );
    assert_eq!(tools["read_file"]["call_count"], 1);
}

#[test]
pub(super) fn e2e_hook_binary_captures_output_chars_from_hook_stdin_tool_response() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let transcript_path = write_fixture_transcript(
        fixture_dir.path(),
        "fixture-hook-stdin-output.jsonl",
        HOOK_STDIN_TOOL_TRANSCRIPT,
    );

    let store_dir = tempdir().expect("tempdir");
    let store_root = store_dir.path().join("memory-api-store");
    fs::create_dir_all(&store_root).expect("create temp store root");

    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let stdin_payload = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "run_in_terminal",
        "tool_response": "hi\n",
        "tool_use_id": "call-hook-1__vscode-1785422593000",
    })
    .to_string();

    let mut child = Command::new(hook_bin)
        .env("MCP_MAIN_CHECKOUT", fixture_dir.path())
        .arg("--transcript-path")
        .arg(&transcript_path)
        .arg("--store-root")
        .arg(&store_root)
        .arg("--trigger")
        .arg("Stop")
        .arg("--from-hook-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn session-capture-hook");

    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin_payload.as_bytes())
        .expect("write hook stdin payload");

    let output = child
        .wait_with_output()
        .expect("wait for session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let tool_metrics_path = store_root
        .join("sessions")
        .join("fixture-hook-stdin-output")
        .join("tool-metrics.json");
    let raw = fs::read_to_string(&tool_metrics_path)
        .expect("tool-metrics.json should exist once a tool call was captured");
    let summary: serde_json::Value =
        serde_json::from_str(&raw).expect("tool-metrics.json should be valid json");

    let output_sizes = summary["tools"]["run_in_terminal"]["output_char_sizes"]
        .as_array()
        .expect("output_char_sizes should be a json array");
    assert_eq!(
        output_sizes.as_slice(),
        &[serde_json::Value::from(3)],
        "output size should reflect the hook stdin tool_response char count (\"hi\\n\" = 3 chars); got: {raw}"
    );

    let output_source = summary["tools"]["run_in_terminal"]["output_source"]
        .as_array()
        .expect("output_source should be a json array");
    assert_eq!(
        output_source.as_slice(),
        &[serde_json::Value::from("hook_payload")],
        "output_source should attribute the size to the hook stdin payload; got: {raw}"
    );
}

/// Ticket 44119807 AC1 real-capture fix: the live PostToolUse hook payload's
/// `tool_response` is observed to always be sent empty, so `output_chars`
/// must instead come from the on-disk `chat-session-resources` spill-file
/// convention, keyed by `<transcript_path>/../chat-session-resources/
/// <session_id>/<tool_use_id>/content.txt`.
#[test]
pub(super) fn e2e_hook_binary_captures_output_chars_from_spill_file_when_hook_payload_empty() {
    let fixture_dir = tempdir().expect("temp fixture dir");
    let chat_root = fixture_dir.path().join("GitHub.copilot-chat");
    let transcripts_dir = chat_root.join("transcripts");
    fs::create_dir_all(&transcripts_dir).expect("create fixture transcripts dir");

    let transcript_path = write_fixture_transcript(
        &transcripts_dir,
        "fixture-spill-output.jsonl",
        HOOK_STDIN_SPILL_TRANSCRIPT,
    );

    let spill_dir = chat_root
        .join("chat-session-resources")
        .join("fixture-spill-output")
        .join("call-spill-1__vscode-1785422594630");
    fs::create_dir_all(&spill_dir).expect("create fixture spill dir");
    fs::write(spill_dir.join("content.txt"), "spilled tool output")
        .expect("write fixture spill content.txt");

    let store_dir = tempdir().expect("tempdir");
    let store_root = store_dir.path().join("memory-api-store");
    fs::create_dir_all(&store_root).expect("create temp store root");

    let hook_bin = std::env::var("CARGO_BIN_EXE_session-capture-hook")
        .expect("cargo should expose session-capture-hook binary path for integration tests");

    let stdin_payload = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "run_in_terminal",
        "tool_response": "",
        "tool_use_id": "call-spill-1__vscode-1785422594630",
        "session_id": "fixture-spill-output",
    })
    .to_string();

    let mut child = Command::new(hook_bin)
        .env("MCP_MAIN_CHECKOUT", fixture_dir.path())
        .arg("--transcript-path")
        .arg(&transcript_path)
        .arg("--store-root")
        .arg(&store_root)
        .arg("--trigger")
        .arg("Stop")
        .arg("--from-hook-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn session-capture-hook");

    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin_payload.as_bytes())
        .expect("write hook stdin payload");

    let output = child
        .wait_with_output()
        .expect("wait for session-capture-hook");

    assert!(
        output.status.success(),
        "session-capture-hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let tool_metrics_path = store_root
        .join("sessions")
        .join("fixture-spill-output")
        .join("tool-metrics.json");
    let raw = fs::read_to_string(&tool_metrics_path)
        .expect("tool-metrics.json should exist once a tool call was captured");
    let summary: serde_json::Value =
        serde_json::from_str(&raw).expect("tool-metrics.json should be valid json");

    let output_sizes = summary["tools"]["run_in_terminal"]["output_char_sizes"]
        .as_array()
        .expect("output_char_sizes should be a json array");
    assert_eq!(
        output_sizes.as_slice(),
        &[serde_json::Value::from(19)],
        "output size should reflect the spill file's char count (\"spilled tool output\" = 19 chars); got: {raw}"
    );

    let output_source = summary["tools"]["run_in_terminal"]["output_source"]
        .as_array()
        .expect("output_source should be a json array");
    assert_eq!(
        output_source.as_slice(),
        &[serde_json::Value::from("spill_file")],
        "output_source should attribute the size to the on-disk spill file; got: {raw}"
    );
}
