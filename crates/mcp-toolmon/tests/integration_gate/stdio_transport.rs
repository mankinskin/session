#[allow(unused_imports)]
use super::*;

//
// STDIO / JSON-RPC integration tests (BLOCKER 1)
//

#[test]
pub(super) fn test_stdio_expensive_model_refused() {
    let tmp = TempDir::new().unwrap();

    // Setup fixtures identical to test_expensive_model_expensive_tool_delegate.
    let price_table = json!({
        "models": [
            {
                "provider_id": "anthropic",
                "model_id": "claude-opus-4-8",
                "output_mtok": 60.0
            }
        ]
    });
    let table_path = write_json(&tmp, "prices.json", &price_table);

    let rollup = json!({
        "report": {
            "tools": [
                {
                    "tool_name": "expensive_tool",
                    "call_count": 10,
                    "cost": 50
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    // Spawn the binary with a dummy passthrough command (--).
    let mut child = Command::new(get_binary_path())
        .arg("--")
        .arg("cat") // Dummy server that echoes input.
        .env("COST_GATE_TABLE", table_path.display().to_string())
        .env("COST_GATE_TOOL_METRICS", rollup_path.display().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to spawn mcp-toolmon binary");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Send initialize request.
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }
    });
    writeln!(stdin, "{}", serde_json::to_string(&init_req).unwrap()).unwrap();
    stdin.flush().unwrap();

    // Read initialize response (skip it).
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();

    // Send tools/call with expensive model and expensive tool.
    let call_req = tools_call_request(2, "expensive_tool", "claude-opus-4-8");
    writeln!(stdin, "{}", serde_json::to_string(&call_req).unwrap()).unwrap();
    stdin.flush().unwrap();

    // Read the response.
    line.clear();
    reader.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).expect("Failed to parse response JSON");

    // Verify it's a refusal response with the correct structure.
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 2);

    // The proxy returns a result with content containing the delegate guidance.
    let content = &response["result"]["content"];
    assert!(content.is_array(), "Expected content array");
    assert!(!content.as_array().unwrap().is_empty());

    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("cost 50"),
        "Expected cost 50 in refusal guidance"
    );
    assert!(
        text.contains("budget") || text.contains("Delegate"),
        "Expected budget/delegate guidance"
    );

    // Clean up.
    drop(stdin);
    let _ = child.wait();
}

#[test]
pub(super) fn test_stdio_cheap_model_allowed() {
    let tmp = TempDir::new().unwrap();
    let (_routing, main_checkout) = active_session_fixture();

    // Setup fixtures identical to test_cheap_model_expensive_tool_allow.
    let price_table = json!({
        "models": [
            {
                "provider_id": "anthropic",
                "model_id": "claude-haiku-3-7",
                "output_mtok": 2.0
            }
        ]
    });
    let table_path = write_json(&tmp, "prices.json", &price_table);

    let rollup = json!({
        "report": {
            "tools": [
                {
                    "tool_name": "expensive_tool",
                    "call_count": 10,
                    "cost": 50
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    // Spawn the binary with a dummy passthrough command.
    let mut child = Command::new(get_binary_path())
        .arg("--")
        .arg("cat")
        .env("COST_GATE_TABLE", table_path.display().to_string())
        .env("COST_GATE_TOOL_METRICS", rollup_path.display().to_string())
        .env("MCP_MAIN_CHECKOUT", main_checkout)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to spawn mcp-toolmon binary");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Send initialize request.
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }
    });
    writeln!(stdin, "{}", serde_json::to_string(&init_req).unwrap()).unwrap();
    stdin.flush().unwrap();

    // Read initialize response (skip it).
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();

    // Send tools/call with cheap model and expensive tool (within budget).
    let call_req = tools_call_request(2, "expensive_tool", "claude-haiku-3-7");
    writeln!(stdin, "{}", serde_json::to_string(&call_req).unwrap()).unwrap();
    stdin.flush().unwrap();

    // Read the response - should be forwarded to the cat command, which echoes it.
    line.clear();
    reader.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).expect("Failed to parse response JSON");

    // Verify the call was forwarded (not refused).
    // The cat command will echo the forwarded request.
    assert_eq!(response["method"], "tools/call");
    assert_eq!(response["params"]["name"], "expensive_tool");

    assert!(
        response["params"]["arguments"]
            .get("caller_model")
            .is_none(),
        "caller_model should be stripped"
    );
    assert!(
        response["params"]["arguments"].get("session_id").is_some(),
        "session_id should be forwarded"
    );

    // Clean up.
    drop(stdin);
    let _ = child.wait();
}

#[test]
pub(super) fn test_stdio_telemetry_recorded_for_allowed_call() {
    // (a) A real intercepted MCP tools/call (spawned binary, stdio transport)
    // records a CallTelemetry line with a non-zero tokens_estimated and a
    // populated duration_ms once the "cat" passthrough echoes the response.
    let tmp = TempDir::new().unwrap();
    let (_routing, main_checkout) = active_session_fixture();

    let price_table = json!({
        "models": [
            {
                "provider_id": "anthropic",
                "model_id": "claude-haiku-3-7",
                "output_mtok": 2.0
            }
        ]
    });
    let table_path = write_json(&tmp, "prices.json", &price_table);

    let rollup = json!({
        "report": {
            "tools": [
                {
                    "tool_name": "expensive_tool",
                    "call_count": 10,
                    "cost": 50
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);
    let telemetry_path = tmp.path().join("telemetry.jsonl");

    let mut child = Command::new(get_binary_path())
        .arg("--")
        .arg("cat")
        .env("COST_GATE_TABLE", table_path.display().to_string())
        .env("COST_GATE_TOOL_METRICS", rollup_path.display().to_string())
        .env(
            "COST_GATE_TELEMETRY_LOG",
            telemetry_path.display().to_string(),
        )
        .env("MCP_MAIN_CHECKOUT", main_checkout)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to spawn mcp-toolmon binary");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let call_req = tools_call_request(1, "expensive_tool", "claude-haiku-3-7");
    writeln!(stdin, "{}", serde_json::to_string(&call_req).unwrap()).unwrap();
    stdin.flush().unwrap();

    // Read the echoed response so the server->client thread has processed it
    // and had a chance to correlate + emit telemetry.
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();

    drop(stdin);
    let _ = child.wait();

    let contents =
        fs::read_to_string(&telemetry_path).expect("expected telemetry JSONL file to be written");
    let last_line = contents
        .lines()
        .last()
        .expect("expected at least one telemetry line");
    let telemetry: Value =
        serde_json::from_str(last_line).expect("telemetry line should be valid JSON");

    assert_eq!(telemetry["tool_name"], "expensive_tool");
    assert_eq!(telemetry["decision"], "allow");
    assert!(telemetry["response_bytes"].as_u64().unwrap() > 0);
    assert!(telemetry["tokens_estimated"].as_u64().unwrap() > 0);
    assert!(telemetry["duration_ms"].as_u64().is_some());
}

#[test]
pub(super) fn test_stdio_tokens_estimated_increases_with_larger_payload() {
    // AC3: exercise real proxy telemetry path with two differently sized
    // payloads and assert strict monotonicity for tokens_estimated.
    let tmp = TempDir::new().unwrap();
    let (_routing, main_checkout) = active_session_fixture();

    let price_table = json!({
        "models": [
            {
                "provider_id": "anthropic",
                "model_id": "claude-haiku-3-7",
                "output_mtok": 2.0
            }
        ]
    });
    let table_path = write_json(&tmp, "prices.json", &price_table);

    // No measured tool in rollup => fail-open allow for any tool name.
    let rollup = json!({ "report": { "tools": [] } });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);
    let telemetry_path = tmp.path().join("telemetry-monotonic.jsonl");

    let mut child = Command::new(get_binary_path())
        .arg("--")
        .arg("cat")
        .env("COST_GATE_TABLE", table_path.display().to_string())
        .env("COST_GATE_TOOL_METRICS", rollup_path.display().to_string())
        .env(
            "COST_GATE_TELEMETRY_LOG",
            telemetry_path.display().to_string(),
        )
        .env("MCP_MAIN_CHECKOUT", main_checkout)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to spawn mcp-toolmon binary");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let small = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "size_probe",
            "arguments": {
                "caller_model": "claude-haiku-3-7",
                "session_id": TEST_SESSION_ID,
                "payload": "x"
            }
        }
    });
    let large = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "size_probe",
            "arguments": {
                "caller_model": "claude-haiku-3-7",
                "session_id": TEST_SESSION_ID,
                "payload": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
            }
        }
    });

    writeln!(stdin, "{}", serde_json::to_string(&small).unwrap()).unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();

    line.clear();
    writeln!(stdin, "{}", serde_json::to_string(&large).unwrap()).unwrap();
    stdin.flush().unwrap();
    reader.read_line(&mut line).unwrap();

    drop(stdin);
    let _ = child.wait();

    let contents =
        fs::read_to_string(&telemetry_path).expect("expected telemetry JSONL file to be written");
    let lines: Vec<&str> = contents.lines().collect();
    assert!(lines.len() >= 2, "expected at least two telemetry lines");

    let first: Value =
        serde_json::from_str(lines[0]).expect("first telemetry line should be valid JSON");
    let second: Value =
        serde_json::from_str(lines[1]).expect("second telemetry line should be valid JSON");

    let first_tokens = first["tokens_estimated"]
        .as_u64()
        .expect("first tokens_estimated should be present");
    let second_tokens = second["tokens_estimated"]
        .as_u64()
        .expect("second tokens_estimated should be present");
    assert!(
        second_tokens > first_tokens,
        "larger payload should produce larger tokens_estimated ({} !< {})",
        first_tokens,
        second_tokens
    );
}
