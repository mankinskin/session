#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn tools_list_response_gets_schema_injected() {
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    // Record the list request id.
    let req = json!({"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}});
    let g = test_gate();
    let _ = handle_client_message(req, Some(&g), &mut p, &mut pc);

    let resp = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "result": { "tools": [
            { "name": "read_file", "inputSchema": { "type": "object", "properties": {}, "required": [] } },
            { "name": "write_file" }
        ] }
    });
    let (out, telemetry) = handle_server_message(resp, Some(&g), &mut p, &mut pc);
    for tool in out["result"]["tools"].as_array().unwrap() {
        assert_eq!(
            tool["inputSchema"]["properties"][CALLER_MODEL_ARG]["type"],
            json!("string")
        );
        assert_eq!(
            tool["inputSchema"]["properties"][SESSION_ID_ARG]["type"],
            json!("string")
        );
        let required = tool["inputSchema"]["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == CALLER_MODEL_ARG));
        assert!(required.iter().any(|v| v == SESSION_ID_ARG));
    }
    assert!(
        telemetry.is_none(),
        "tools/list response is not a tools/call, no telemetry expected"
    );
}

#[test]
pub(super) fn inject_creates_schema_when_absent() {
    let mut tool = json!({ "name": "x" });
    inject_caller_model_schema(&mut tool);
    assert_eq!(tool["inputSchema"]["type"], json!("object"));
    assert_eq!(
        tool["inputSchema"]["properties"][SESSION_ID_ARG]["type"],
        json!("string")
    );
    assert_eq!(tool["inputSchema"]["required"][0], json!(CALLER_MODEL_ARG));
    assert_eq!(tool["inputSchema"]["required"][1], json!(SESSION_ID_ARG));
}

#[test]
pub(super) fn allowed_call_emits_nonzero_tokens_estimated_on_response() {
    let _routing = active_routing();
    // (a) A forwarded (allowed) tools/call correlates its response by
    // JSON-RPC id and records a non-zero tokens_estimated derived from
    // the combined request+response payload.
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    let req = call("some_unknown_tool", Some("gpt-5-mini"));
    let (action, telemetry) = handle_client_message(req, Some(&g), &mut p, &mut pc);
    assert!(
        telemetry.is_none(),
        "no telemetry until the response arrives"
    );
    let forwarded = match action {
        ClientAction::Forward(v) => v,
        other => panic!("expected Forward, got {other:?}"),
    };
    let id = forwarded["id"].clone();

    let resp = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "content": [{ "type": "text", "text": "some tool output" }] }
    });
    let (_, telemetry) = handle_server_message(resp, Some(&g), &mut p, &mut pc);
    let telemetry = telemetry.expect("expected telemetry once the response is correlated");
    assert_eq!(telemetry.decision, "allow");
    assert_eq!(telemetry.tool_name, "some_unknown_tool");
    assert!(
        telemetry.response_bytes.unwrap_or(0) > 0,
        "response_bytes should be non-zero"
    );
    assert!(
        telemetry.response_chars.unwrap_or(0) > 0,
        "response_chars should be non-zero"
    );
    assert!(
        telemetry.tokens_estimated.unwrap_or(0) > 0,
        "tokens_estimated should be non-zero for a real intercepted tools/call"
    );
    assert_eq!(
        telemetry.tokens_estimated,
        Some((telemetry.request_chars.unwrap_or(0) + telemetry.response_chars.unwrap_or(0)) / 4)
    );
}

#[test]
pub(super) fn duration_ms_is_populated_for_forwarded_calls() {
    let _routing = active_routing();
    // (c) duration_ms measures wall-clock from forward to response receipt.
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    let req = call("some_unknown_tool", Some("gpt-5-mini"));
    let (action, _) = handle_client_message(req, Some(&g), &mut p, &mut pc);
    let forwarded = match action {
        ClientAction::Forward(v) => v,
        other => panic!("expected Forward, got {other:?}"),
    };
    let id = forwarded["id"].clone();

    // Sleep a measurable span so duration_ms is guaranteed nonzero.
    std::thread::sleep(std::time::Duration::from_millis(5));

    let resp = json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [] } });
    let (_, telemetry) = handle_server_message(resp, Some(&g), &mut p, &mut pc);
    let telemetry = telemetry.expect("expected telemetry");
    assert!(
        telemetry.duration_ms >= 5,
        "duration_ms should reflect the wall-clock span, got {}",
        telemetry.duration_ms
    );
}

#[test]
pub(super) fn refused_call_records_zero_duration_and_response_counts() {
    // (b/AC4 null-vs-zero): refused calls never reach the server, so
    // response counts and duration_ms are recorded as zero (measured),
    // not omitted — the call itself was still observed.
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    let (_, telemetry) = handle_client_message(call("read_file", None), Some(&g), &mut p, &mut pc);
    let telemetry = telemetry.expect("expected telemetry for the refused call");
    assert_eq!(telemetry.response_bytes, Some(0));
    assert_eq!(telemetry.response_chars, Some(0));
    assert_eq!(telemetry.duration_ms, 0);
    assert!(telemetry.request_chars.unwrap_or(0) > 0);
}
