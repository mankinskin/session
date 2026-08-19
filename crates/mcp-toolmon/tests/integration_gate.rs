//! Integration tests for mcp-toolmon gating logic.
//!
//! These tests verify end-to-end behavior using synthetic price tables and
//! rollups. They cover:
//! - Expensive model + expensive tool → Delegate
//! - Cheap model + same tool → Allow
//! - Unmeasured tool → Allow (fail-open)
//! - Unknown model → Reject
//! - Missing price table → fail-open (Gate::load error)

use mcp_toolmon::proxy::{
    ClientAction,
    PendingCalls,
    PendingList,
    handle_client_message,
};
use serde_json::{
    Value,
    json,
};
use session_api::{
    SessionStoreConfig,
    SessionWorktreeCheckInRequest,
};
use session_workspace_resolver::{
    ResolverConfig,
    SessionWorkspaceResolver,
};
use std::{
    ffi::OsString,
    fs,
    io::{
        BufRead,
        BufReader,
        Write,
    },
    path::PathBuf,
    process::{
        Command,
        Stdio,
    },
    sync::Mutex,
};
use tempfile::TempDir;
use toolmon_costgate::{
    CostGatePolicy,
    gate::{
        Gate,
        ModelBudgetCalibration,
    },
};
use toolmon_policy_api::Decision;

static ENV_LOCK: Mutex<()> = Mutex::new(());
const TEST_SESSION_ID: &str = "77777777-7777-4777-8777-777777777777";

/// Helper: write JSON to a temp file.
fn write_json(
    dir: &TempDir,
    name: &str,
    value: &Value,
) -> PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    path
}

fn active_session_fixture() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let main_checkout = temp.path().join("repository");
    let worktree = main_checkout
        .join(".worktrees")
        .join(TEST_SESSION_ID)
        .join("feature");
    assert!(
        Command::new("git")
            .current_dir(temp.path())
            .args(["init", "--quiet", &main_checkout.to_string_lossy()])
            .status()
            .unwrap()
            .success()
    );
    for args in [
        ["config", "user.email", "tests@example.invalid"],
        ["config", "user.name", "mcp-toolmon integration tests"],
    ] {
        assert!(
            Command::new("git")
                .current_dir(&main_checkout)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    fs::write(main_checkout.join("README.md"), "fixture\n").unwrap();
    assert!(
        Command::new("git")
            .current_dir(&main_checkout)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&main_checkout)
            .args(["commit", "--quiet", "-m", "fixture"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&main_checkout)
            .args(["worktree", "add", "--quiet", "-b", "agent/test"])
            .arg(&worktree)
            .arg("HEAD")
            .status()
            .unwrap()
            .success()
    );
    SessionWorkspaceResolver::new(ResolverConfig {
        main_checkout: main_checkout.clone(),
        workspace_slug: "default".to_string(),
    })
    .unwrap();
    // The anchor store beneath the main checkout is the worktree registry.
    SessionStoreConfig::new(main_checkout.join(".session"), "default")
        .check_in_worktree(SessionWorktreeCheckInRequest {
            session_id: TEST_SESSION_ID.to_string(),
            owner_id: "agent".to_string(),
            ticket_id: "ticket".to_string(),
            worktree_path: worktree.clone(),
            branch: "agent/test".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();
    (temp, main_checkout)
}

struct ActiveSessionEnvironment {
    _lock: std::sync::MutexGuard<'static, ()>,
    _temp: TempDir,
    previous_main_checkout: Option<OsString>,
}

impl Drop for ActiveSessionEnvironment {
    fn drop(&mut self) {
        unsafe {
            match &self.previous_main_checkout {
                Some(value) => std::env::set_var("MCP_MAIN_CHECKOUT", value),
                None => std::env::remove_var("MCP_MAIN_CHECKOUT"),
            }
        }
    }
}

fn active_session_environment() -> ActiveSessionEnvironment {
    let lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let previous_main_checkout = std::env::var_os("MCP_MAIN_CHECKOUT");
    let (temp, main_checkout) = active_session_fixture();
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", main_checkout) };
    ActiveSessionEnvironment {
        _lock: lock,
        _temp: temp,
        previous_main_checkout,
    }
}

/// Helper: get the path to the built mcp-toolmon binary.
fn get_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mcp-toolmon"))
}

/// Helper: construct a tools/call JSON-RPC request.
fn tools_call_request(
    id: u32,
    tool: &str,
    caller_model: &str,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {
            "name": tool,
            "arguments": {
                "caller_model": caller_model,
                "session_id": TEST_SESSION_ID
            }
        }
    })
}

/// Extract decision guidance from ClientAction::Respond error payload.
fn extract_error_text(action: ClientAction) -> String {
    match action {
        ClientAction::Respond(val) => val["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

#[test]
fn test_expensive_model_expensive_tool_delegate() {
    let tmp = TempDir::new().unwrap();

    // Price table with an expensive model (output_mtok > threshold).
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

    // Rollup with a high-cost tool.
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

    let gate = Gate::load(
        &table_path,
        ModelBudgetCalibration::default(),
        Some(&rollup_path),
        None,
    )
    .unwrap();

    // Expensive model's base_budget will be 0 (since output_mtok >= budget_zero_price).
    // Tool cost is 50. Decision should be Delegate.
    let decision = gate.evaluate("claude-opus-4-8", "expensive_tool", None);
    match decision {
        Decision::Delegate { guidance } => {
            assert!(guidance.contains("expensive_tool"));
            assert!(guidance.contains("cost 50"));
        },
        _ => panic!("Expected Delegate, got {:?}", decision),
    }
}

#[test]
fn test_cheap_model_expensive_tool_allow() {
    let tmp = TempDir::new().unwrap();

    // Price table with a cheap model (low output_mtok).
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

    // Rollup with a high-cost tool (but within cheap model's budget).
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

    let gate = Gate::load(
        &table_path,
        ModelBudgetCalibration::default(),
        Some(&rollup_path),
        None,
    )
    .unwrap();

    // Cheap model (output_mtok=2.0) should have high base_budget (~97).
    // Tool cost is 50, which is within budget. Decision should be Allow.
    let decision = gate.evaluate("claude-haiku-3-7", "expensive_tool", None);
    assert_eq!(decision, Decision::Allow);
}

#[test]
fn test_unmeasured_tool_allow() {
    let tmp = TempDir::new().unwrap();

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

    // Rollup with NO entry for "unmeasured_tool" (or insufficient call count).
    let rollup = json!({
        "report": {
            "tools": []
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    let gate = Gate::load(
        &table_path,
        ModelBudgetCalibration::default(),
        Some(&rollup_path),
        None,
    )
    .unwrap();

    // Unmeasured tool → cost=0 → fail-open → Allow.
    let decision = gate.evaluate("claude-opus-4-8", "unmeasured_tool", None);
    assert_eq!(decision, Decision::Allow);
}

#[test]
fn test_unknown_model_reject() {
    let tmp = TempDir::new().unwrap();

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
                    "tool_name": "some_tool",
                    "call_count": 10,
                    "cost": 10
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    let gate = Gate::load(
        &table_path,
        ModelBudgetCalibration::default(),
        Some(&rollup_path),
        None,
    )
    .unwrap();

    // Unknown model "gpt-99-turbo" is not in the price table.
    let decision = gate.evaluate("gpt-99-turbo", "some_tool", None);
    match decision {
        Decision::Reject { guidance } => {
            assert!(guidance.to_lowercase().contains("unrecognized"));
        },
        _ => panic!("Expected Reject, got {:?}", decision),
    }
}

#[test]
fn test_missing_price_table_fail_open() {
    let tmp = TempDir::new().unwrap();
    let nonexistent_path = tmp.path().join("does_not_exist.json");

    // Attempting to load a missing price table should fail, resulting in fail-open.
    let result = Gate::load(
        &nonexistent_path,
        ModelBudgetCalibration::default(),
        None,
        None,
    );
    assert!(
        result.is_err(),
        "Expected Gate::load to fail for missing table"
    );
}

#[test]
fn test_handle_client_message_expensive_model_refused() {
    let tmp = TempDir::new().unwrap();

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
                    "tool_name": "get_ticket_description",
                    "call_count": 100,
                    "cost": 80
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    let gate = Gate::load(
        &table_path,
        ModelBudgetCalibration::default(),
        Some(&rollup_path),
        None,
    )
    .unwrap();

    let policy = CostGatePolicy::new(gate);
    let msg =
        tools_call_request(1, "get_ticket_description", "claude-opus-4-8");
    let mut pending = PendingList::default();
    let mut pending_calls = PendingCalls::default();
    let (action, _telemetry) = handle_client_message(
        msg,
        Some(&policy),
        &mut pending,
        &mut pending_calls,
    );

    // Should be refused with Delegate guidance.
    let error_text = extract_error_text(action);
    assert!(error_text.contains("cost 80"));
    assert!(error_text.contains("budget"));
}

#[test]
fn test_handle_client_message_cheap_model_allowed() {
    let tmp = TempDir::new().unwrap();
    let _routing = active_session_environment();

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
                    "tool_name": "get_ticket_description",
                    "call_count": 100,
                    "cost": 80
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    let gate = Gate::load(
        &table_path,
        ModelBudgetCalibration::default(),
        Some(&rollup_path),
        None,
    )
    .unwrap();

    let policy = CostGatePolicy::new(gate);
    let msg =
        tools_call_request(1, "get_ticket_description", "claude-haiku-3-7");
    let mut pending = PendingList::default();
    let mut pending_calls = PendingCalls::default();
    let (action, _telemetry) = handle_client_message(
        msg,
        Some(&policy),
        &mut pending,
        &mut pending_calls,
    );

    // Should be forwarded (allowed).
    match action {
        ClientAction::Forward(val) => {
            assert!(val["params"]["arguments"].get("caller_model").is_none());
            assert!(val["params"]["arguments"].get("session_id").is_some());
        },
        ClientAction::Respond(val) => {
            panic!("Expected Forward, got Respond: {:?}", val);
        },
    }
}

#[test]
fn test_handle_client_message_no_gate_fail_open() {
    let msg = tools_call_request(1, "some_tool", "claude-opus-4-8");
    let mut pending = PendingList::default();
    let mut pending_calls = PendingCalls::default();
    let (action, _telemetry) =
        handle_client_message(msg, None, &mut pending, &mut pending_calls);

    // No gate (fail-open) → should forward unchanged.
    match action {
        ClientAction::Forward(_) => {},
        ClientAction::Respond(val) => {
            panic!(
                "Expected Forward in fail-open mode, got Respond: {:?}",
                val
            );
        },
    }
}

//
// STDIO / JSON-RPC integration tests (BLOCKER 1)
//

#[test]
fn test_stdio_expensive_model_refused() {
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
    let response: Value =
        serde_json::from_str(&line).expect("Failed to parse response JSON");

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
fn test_stdio_cheap_model_allowed() {
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
    let response: Value =
        serde_json::from_str(&line).expect("Failed to parse response JSON");

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
fn test_stdio_telemetry_recorded_for_allowed_call() {
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

    let contents = fs::read_to_string(&telemetry_path)
        .expect("expected telemetry JSONL file to be written");
    let last_line = contents
        .lines()
        .last()
        .expect("expected at least one telemetry line");
    let telemetry: Value = serde_json::from_str(last_line)
        .expect("telemetry line should be valid JSON");

    assert_eq!(telemetry["tool_name"], "expensive_tool");
    assert_eq!(telemetry["decision"], "allow");
    assert!(telemetry["response_bytes"].as_u64().unwrap() > 0);
    assert!(telemetry["tokens_estimated"].as_u64().unwrap() > 0);
    assert!(telemetry["duration_ms"].as_u64().is_some());
}

#[test]
fn test_stdio_tokens_estimated_increases_with_larger_payload() {
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

    let contents = fs::read_to_string(&telemetry_path)
        .expect("expected telemetry JSONL file to be written");
    let lines: Vec<&str> = contents.lines().collect();
    assert!(lines.len() >= 2, "expected at least two telemetry lines");

    let first: Value = serde_json::from_str(lines[0])
        .expect("first telemetry line should be valid JSON");
    let second: Value = serde_json::from_str(lines[1])
        .expect("second telemetry line should be valid JSON");

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

//
// verdict subcommand tests (BLOCKER 2)
//

#[test]
fn test_verdict_allow() {
    let tmp = TempDir::new().unwrap();

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
                    "tool_name": "cheap_tool",
                    "call_count": 10,
                    "cost": 10
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    let output = Command::new(get_binary_path())
        .arg("verdict")
        .arg("--model")
        .arg("claude-haiku-3-7")
        .arg("--tool")
        .arg("cheap_tool")
        .arg("--table")
        .arg(table_path.display().to_string())
        .arg("--rollup")
        .arg(rollup_path.display().to_string())
        .output()
        .expect("Failed to execute verdict subcommand");

    assert!(output.status.success(), "verdict should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "Allow", "Expected 'Allow' verdict");
}

#[test]
fn test_verdict_delegate() {
    let tmp = TempDir::new().unwrap();

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

    let output = Command::new(get_binary_path())
        .arg("verdict")
        .arg("--model")
        .arg("claude-opus-4-8")
        .arg("--tool")
        .arg("expensive_tool")
        .arg("--table")
        .arg(table_path.display().to_string())
        .arg("--rollup")
        .arg(rollup_path.display().to_string())
        .output()
        .expect("Failed to execute verdict subcommand");

    assert!(output.status.success(), "verdict should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("Delegate:"),
        "Expected 'Delegate:' verdict"
    );
    assert!(stdout.contains("cost 50"), "Expected cost in guidance");
}

#[test]
fn test_verdict_reject_unknown_model() {
    let tmp = TempDir::new().unwrap();

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
                    "tool_name": "some_tool",
                    "call_count": 10,
                    "cost": 10
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    let output = Command::new(get_binary_path())
        .arg("verdict")
        .arg("--model")
        .arg("unknown-model-99")
        .arg("--tool")
        .arg("some_tool")
        .arg("--table")
        .arg(table_path.display().to_string())
        .arg("--rollup")
        .arg(rollup_path.display().to_string())
        .output()
        .expect("Failed to execute verdict subcommand");

    assert!(output.status.success(), "verdict should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("Reject:"), "Expected 'Reject:' verdict");
    assert!(
        stdout.to_lowercase().contains("unrecognized"),
        "Expected unrecognized model guidance"
    );
}

#[test]
fn test_verdict_missing_args() {
    // Test error case: missing required arguments.
    let output = Command::new(get_binary_path())
        .arg("verdict")
        .arg("--model")
        .arg("some-model")
        // Missing --tool and --table
        .output()
        .expect("Failed to execute verdict subcommand");

    assert!(!output.status.success(), "verdict should exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage"), "Expected usage message in stderr");
}
