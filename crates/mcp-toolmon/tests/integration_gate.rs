//! Integration tests for mcp-toolmon gating logic.
//!
//! These tests verify end-to-end behavior using synthetic price tables and
//! rollups. They cover:
//! - Expensive model + expensive tool → Delegate
//! - Cheap model + same tool → Allow
//! - Unmeasured tool → Allow (fail-open)
//! - Unknown model → Reject
//! - Missing price table → fail-open (Gate::load error)

use mcp_toolmon::proxy::{ClientAction, PendingCalls, PendingList, handle_client_message};
use serde_json::{Value, json};
use session_api::{SessionStoreConfig, SessionWorktreeCheckInRequest};
use session_workspace_resolver::{ResolverConfig, SessionWorkspaceResolver};
use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::Mutex,
};
use tempfile::TempDir;
use toolmon_costgate::{
    CostGatePolicy,
    gate::{Gate, ModelBudgetCalibration},
};
use toolmon_policy_api::Decision;

static ENV_LOCK: Mutex<()> = Mutex::new(());
const TEST_SESSION_ID: &str = "77777777-7777-4777-8777-777777777777";

/// Helper: write JSON to a temp file.
fn write_json(dir: &TempDir, name: &str, value: &Value) -> PathBuf {
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
        workspace_path: "default".to_string(),
    })
    .unwrap();
    // The anchor store beneath the main checkout is the worktree registry.
    SessionStoreConfig::new(main_checkout.join(".session"))
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
fn tools_call_request(id: u32, tool: &str, caller_model: &str) -> Value {
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
        }
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
        }
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

#[path = "integration_gate/stdio_transport.rs"]
mod stdio_transport;
#[allow(unused_imports)]
pub use stdio_transport::*;

#[path = "integration_gate/verdicts.rs"]
mod verdicts;
#[allow(unused_imports)]
pub use verdicts::*;

#[path = "integration_gate/proxy_policy.rs"]
mod proxy_policy;
#[allow(unused_imports)]
pub use proxy_policy::*;
