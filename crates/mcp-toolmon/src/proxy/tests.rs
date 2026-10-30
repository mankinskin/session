use super::workspace_resolution::{DEFAULT_STORE_DIR, MAIN_CHECKOUT_ENV, anchored_resolver};
use super::*;

const TEST_SESSION_ID: &str = "66666666-6666-4666-8666-666666666666";
use session_api::{SessionStoreConfig, SessionWorktreeCheckInRequest, SessionWorktreeStatus};
use session_workspace_resolver::{ResolverConfig, SessionWorkspaceResolver};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};
use tempfile::TempDir;
use toolmon_costgate::{CostGatePolicy, Gate};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn test_gate() -> CostGatePolicy {
    // Write a tiny fixture table to a unique temp file and load it. A
    // per-call counter avoids collisions between parallel tests (same pid).
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir();
    let path = dir.join(format!("mcpcg-fixture-{}-{}.json", std::process::id(), n));
    std::fs::write(
        &path,
        r#"{"models":[
                {"provider_id":"anthropic","model_id":"claude-opus-4-1","output_mtok":75.0},
                {"provider_id":"openai","model_id":"gpt-5-mini","output_mtok":2.0}
            ]}"#,
    )
    .unwrap();
    let g = Gate::load(
        Path::new(&path),
        toolmon_costgate::ModelBudgetCalibration::default(),
        None,
        None,
    )
    .unwrap();
    let _ = std::fs::remove_file(&path);
    CostGatePolicy::new(g)
}

fn call(tool: &str, model: Option<&str>) -> Value {
    let mut args = serde_json::Map::new();
    if let Some(m) = model {
        args.insert(CALLER_MODEL_ARG.into(), json!(m));
    }
    args.insert(SESSION_ID_ARG.into(), json!(TEST_SESSION_ID));
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": args }
    })
}

fn routing_fixture(
    status: SessionWorktreeStatus,
    use_main_checkout: bool,
) -> (TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
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
        ["config", "user.name", "mcp-toolmon tests"],
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
    std::fs::write(main_checkout.join("README.md"), "fixture\n").unwrap();
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
    let _resolver = SessionWorkspaceResolver::new(ResolverConfig {
        main_checkout: main_checkout.clone(),
        workspace_path: "default".to_string(),
    })
    .unwrap();
    // The anchor store is the worktree registry: assignments always live
    // beneath the checkout the servers were launched in.
    let store = SessionStoreConfig::new(main_checkout.join(".session"));
    store
        .check_in_worktree(SessionWorktreeCheckInRequest {
            session_id: TEST_SESSION_ID.to_string(),
            owner_id: "agent".to_string(),
            ticket_id: "ticket".to_string(),
            worktree_path: worktree.clone(),
            branch: "agent/test".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();
    if use_main_checkout {
        let registry_path = main_checkout
            .join(".session/local/worktrees")
            .join(format!("{TEST_SESSION_ID}.json"));
        let mut registry: Value =
            serde_json::from_slice(&std::fs::read(&registry_path).unwrap()).unwrap();
        registry["assignment"]["path"] = json!(main_checkout.to_string_lossy());
        std::fs::write(registry_path, serde_json::to_vec_pretty(&registry).unwrap()).unwrap();
    }
    let path = main_checkout.join(format!(".session/sessions/{TEST_SESSION_ID}/session.json"));
    let mut record = store.read_session(TEST_SESSION_ID).unwrap();
    record.metadata.worktree.as_mut().unwrap().status = status;
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    let registry_path = main_checkout
        .join(".session/local/worktrees")
        .join(format!("{TEST_SESSION_ID}.json"));
    let mut registry: Value =
        serde_json::from_slice(&std::fs::read(&registry_path).unwrap()).unwrap();
    registry["assignment"]["status"] = json!(status);
    std::fs::write(registry_path, serde_json::to_vec_pretty(&registry).unwrap()).unwrap();
    (temp, main_checkout, worktree)
}

fn main_checkout_fixture() -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let main_checkout = temp.path().join("repository");
    std::fs::create_dir_all(main_checkout.join(".git")).unwrap();
    std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
    std::fs::create_dir_all(main_checkout.join(".worktrees")).unwrap();
    (temp, main_checkout)
}

struct TestRouting {
    _guard: std::sync::MutexGuard<'static, ()>,
    _temp: TempDir,
}

impl Drop for TestRouting {
    fn drop(&mut self) {
        unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
    }
}

fn active_routing() -> TestRouting {
    let guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (temp, main_checkout, _worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, main_checkout) };
    TestRouting {
        _guard: guard,
        _temp: temp,
    }
}

fn allowed_call() -> Value {
    call("read_file", Some("gpt-5-mini"))
}

fn route(request: Value, gate: &CostGatePolicy) -> (ClientAction, Option<CallTelemetry>) {
    let mut pending = PendingList::default();
    let mut pending_calls = PendingCalls::default();
    handle_client_message(request, Some(gate), &mut pending, &mut pending_calls)
}

fn route_with_schema(
    request: Value,
    gate: &CostGatePolicy,
    tool: &str,
    properties: Value,
) -> (ClientAction, Option<CallTelemetry>) {
    let mut pending = PendingList::default();
    let mut pending_calls = PendingCalls::default();
    let _ = handle_client_message(
        json!({"jsonrpc":"2.0","id":99,"method":"tools/list","params":{}}),
        Some(gate),
        &mut pending,
        &mut pending_calls,
    );
    let _ = handle_server_message(
        json!({
            "jsonrpc": "2.0",
            "id": 99,
            "result": { "tools": [{
                "name": tool,
                "inputSchema": { "type": "object", "properties": properties }
            }] }
        }),
        Some(gate),
        &mut pending,
        &mut pending_calls,
    );
    handle_client_message(request, Some(gate), &mut pending, &mut pending_calls)
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn canonicalized_normalized(path: &Path) -> String {
    normalized(&std::fs::canonicalize(path).unwrap())
        .trim_start_matches("//?/")
        .to_string()
}

fn response_text(action: ClientAction) -> String {
    let ClientAction::Respond(value) = action else {
        panic!("expected routing rejection");
    };
    assert_eq!(value["result"]["isError"], true);
    value["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

fn copy_request(from: Value, to: Value, root: &Path) -> Value {
    let mut request = call("fs_copy_file", Some("gpt-5-mini"));
    request["params"]["arguments"]["from"] = from;
    request["params"]["arguments"]["to"] = to;
    request["params"]["arguments"]["root"] = json!(root);
    request
}

fn route_copy(request: Value) -> ClientAction {
    route_with_schema(
        request,
        &test_gate(),
        "fs_copy_file",
        json!({"from": {}, "to": {}, "root": {}}),
    )
    .0
}

#[path = "tests/assignment_routing.rs"]
mod assignment_routing;
#[allow(unused_imports)]
pub use assignment_routing::*;

#[path = "tests/path_arguments.rs"]
mod path_arguments;
#[allow(unused_imports)]
pub use path_arguments::*;

#[path = "tests/workspace_boundaries.rs"]
mod workspace_boundaries;
#[allow(unused_imports)]
pub use workspace_boundaries::*;

#[path = "tests/cost_policy.rs"]
mod cost_policy;
#[allow(unused_imports)]
pub use cost_policy::*;

#[path = "tests/telemetry.rs"]
mod telemetry;
#[allow(unused_imports)]
pub use telemetry::*;
