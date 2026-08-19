//! Supervisor lifecycle tests (T4): swap, pending-request failure synthesis
//! (R6), and never-exit respawn fallback (R7). See spec `1bef7b3d-…`
//! Validation Strategy and Failure Modes tables.

use std::time::Duration;

use mcp_toolmon::supervisor::Supervisor;
use serde_json::{
    Value,
    json,
};
use tempfile::TempDir;

fn fake_v1_bytes() -> Vec<u8> {
    std::fs::read(env!("CARGO_BIN_EXE_fake-mcp-v1")).unwrap()
}

fn fake_v2_bytes() -> Vec<u8> {
    std::fs::read(env!("CARGO_BIN_EXE_fake-mcp-v2")).unwrap()
}

/// Copy `bytes` to `path`, preserving the executable bit on unix (the
/// fixture binaries are already executable; `fs::copy` preserves
/// permissions from the source file's mode on unix but not reliably across
/// an overwrite of an existing dest, so set it explicitly).
fn write_exe(
    path: &std::path::Path,
    bytes: &[u8],
) {
    std::fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

fn canonical_exe_name() -> &'static str {
    if cfg!(windows) {
        "canonical.exe"
    } else {
        "canonical"
    }
}

/// Bounded-wait helper (spec: no bare `sleep` as a synchronization
/// mechanism). Polls `condition` with a short yield between checks until it
/// returns `true` or `timeout` elapses, at which point it panics with `msg`.
async fn wait_until<F: Fn() -> bool>(
    condition: F,
    timeout: Duration,
    msg: &str,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if condition() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("{msg}");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn call_generation(
    supervisor: &Supervisor,
    id: i64,
) -> Value {
    let req = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"generation","arguments":{}}});
    assert!(
        supervisor.write_line(&req.to_string()).await,
        "write to child failed"
    );
    let line = supervisor
        .read_line()
        .await
        .expect("child closed without responding");
    serde_json::from_str(&line).unwrap()
}

/// Perform the MCP handshake (initialize + notifications/initialized) so
/// the fixture's `seen_initialize` ordering guard (T5) does not reject a
/// subsequent `tools/call` as a pre-handshake violation. These T4 tests
/// predate the handshake-replay cache and only care about swap/respawn
/// behavior, not the handshake itself, so this is test setup, not a
/// behavior change.
async fn perform_handshake(supervisor: &Supervisor) {
    let init =
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}});
    assert!(
        supervisor.write_line(&init.to_string()).await,
        "write initialize failed"
    );
    supervisor
        .read_line()
        .await
        .expect("child closed without responding to initialize");
    let notif = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    assert!(
        supervisor.write_line(&notif.to_string()).await,
        "write notifications/initialized failed"
    );
}

/// Like [`call_generation`] but tolerant of the brief mid-swap window where
/// there is transiently no healthy child to write to (R7): retries the
/// write itself (not the read) up to `timeout`, bounded by a real deadline
/// rather than a fixed sleep count.
async fn call_generation_retrying(
    supervisor: &Supervisor,
    id: i64,
    timeout: Duration,
) -> Value {
    let req = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"generation","arguments":{}}});
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if supervisor.write_line(&req.to_string()).await {
            let line = supervisor
                .read_line()
                .await
                .expect("child closed without responding");
            return serde_json::from_str(&line).unwrap();
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("no healthy child accepted the request within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn generation_text(resp: &Value) -> &str {
    resp["result"]["content"][0]["text"].as_str().unwrap()
}

#[tokio::test]
async fn swap_child_replaces_running_child() {
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());

    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor =
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap();

    perform_handshake(&supervisor).await;

    let v1 = call_generation(&supervisor, 1).await;
    assert_eq!(generation_text(&v1), "v1");

    // Simulate the canonical binary having been rebuilt: overwrite it with
    // the v2 fixture bytes, then trigger a swap directly (C1 determinism).
    write_exe(&canonical, &fake_v2_bytes());
    let synthesized = supervisor.swap_child_with_drain_ms(200).await;
    assert!(
        synthesized.is_empty(),
        "no in-flight requests were pending during this swap"
    );

    let v2 = call_generation(&supervisor, 2).await;
    assert_eq!(generation_text(&v2), "v2");

    let _ = supervisor.shutdown().await;
}

#[tokio::test]
async fn inflight_request_synthesized_error_on_kill() {
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());

    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor =
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap();

    perform_handshake(&supervisor).await;

    // Simulate a request that was forwarded to the child and is still
    // in-flight (its response has not yet been read/resolved) when a swap
    // is triggered mid-flight.
    let pending_id = json!(42);
    supervisor.record_pending(&pending_id).await;

    // Tight drain window; nothing will ever resolve this id, so the drain
    // deadline (not a bare sleep) is the only thing gating completion.
    let synthesized = supervisor.swap_child_with_drain_ms(50).await;

    assert_eq!(
        synthesized.len(),
        1,
        "exactly the one pending id should be synthesized"
    );
    let err = &synthesized[0];
    assert_eq!(
        err["id"], pending_id,
        "synthesized error must carry the original request id"
    );
    assert!(
        err["error"].is_object(),
        "synthesized response must be a JSON-RPC error object"
    );
    assert_eq!(err["error"]["code"], -32001);

    // The new generation must still be healthy afterward — this proves the
    // kill+drain path doesn't itself leave the supervisor without a child.
    wait_until(
        || supervisor.retry_count() == 0,
        Duration::from_millis(500),
        "expected a clean respawn to require no retries for a valid binary",
    )
    .await;
    assert!(supervisor.has_healthy_child().await);
    let v1 = call_generation(&supervisor, 99).await;
    assert_eq!(generation_text(&v1), "v1");

    let _ = supervisor.shutdown().await;
}

#[tokio::test]
async fn respawn_backoff_no_process_exit() {
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());

    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor =
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap();

    perform_handshake(&supervisor).await;

    let v1 = call_generation(&supervisor, 1).await;
    assert_eq!(generation_text(&v1), "v1");

    // Corrupt the canonical binary in place (simulates a half-written or
    // bad build landing at the installed path) and swap.
    write_exe(&canonical, b"not an executable at all, just garbage bytes");

    let synthesized = supervisor.swap_child_with_drain_ms(50).await;
    assert!(synthesized.is_empty());

    // The proxy/task must still be alive and answering — this is the R7
    // assertion: no panic, no process exit, retries observed, and service
    // restored from the last-known-good (v1) shadow copy.
    assert!(
        supervisor.retry_count() > 0,
        "expected respawn attempts against the corrupt binary to be retried and counted"
    );
    assert!(
        supervisor.has_healthy_child().await,
        "must have fallen back to the last-known-good shadow copy"
    );
    let fallback = call_generation(&supervisor, 2).await;
    assert_eq!(
        generation_text(&fallback),
        "v1",
        "service must be restored from the last-known-good copy, not the corrupt one"
    );

    let _ = supervisor.shutdown().await;
}

#[tokio::test]
async fn no_swap_tool_appears_in_tools_list() {
    // R4/no-tool-exposed guard: nothing in this crate's proxy layer injects
    // a restart/reload tool. This is asserted at the fixture level: the
    // fixture's own static tools/list is unaffected by any swap, since the
    // supervisor has no tools/list-rewriting responsibility at all (that
    // lives in proxy.rs and only ever adds caller_model to existing tools).
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());

    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor =
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap();

    let list_before = list_tools(&supervisor, 1).await;

    write_exe(&canonical, &fake_v2_bytes());
    let _ = supervisor.swap_child_with_drain_ms(200).await;

    let list_after = list_tools(&supervisor, 2).await;

    assert_eq!(list_before, vec!["generation".to_string()]);
    assert_eq!(list_after, vec!["generation".to_string()]);

    let _ = supervisor.shutdown().await;
}

async fn list_tools(
    supervisor: &Supervisor,
    id: i64,
) -> Vec<String> {
    let req =
        json!({"jsonrpc":"2.0","id":id,"method":"tools/list","params":{}});
    supervisor.write_line(&req.to_string()).await;
    let line = supervisor.read_line().await.unwrap();
    let resp: Value = serde_json::from_str(&line).unwrap();
    resp["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

/// Concurrency check for the atomicity fix: while requests are continuously
/// round-tripped through the supervisor, a swap is triggered mid-stream.
/// This does not (and cannot, from outside the process) directly observe
/// the internal `ChildHandles` struct, but it does prove there is no
/// visible message corruption or panic under concurrent read/write/swap —
/// the practical, externally-observable consequence a mismatched-generation
/// bug would produce. The structural guarantee (single `Arc<ChildHandles>`
/// snapshot per call) is documented in `src/supervisor.rs`.
#[tokio::test]
async fn concurrent_swap_produces_no_corrupted_responses() {
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());

    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor = std::sync::Arc::new(
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap(),
    );

    perform_handshake(&supervisor).await;

    let caller = {
        let supervisor = supervisor.clone();
        tokio::spawn(async move {
            for i in 0..20i64 {
                let resp = call_generation_retrying(
                    &supervisor,
                    i,
                    Duration::from_secs(2),
                )
                .await;
                let text = generation_text(&resp);
                assert!(
                    text == "v1" || text == "v2",
                    "response must be a whole, valid generation string, never corrupted: {text:?}"
                );
            }
        })
    };

    write_exe(&canonical, &fake_v2_bytes());
    let _ = supervisor.swap_child_with_drain_ms(200).await;

    caller.await.unwrap();
    let _ = supervisor.shutdown().await;
}
