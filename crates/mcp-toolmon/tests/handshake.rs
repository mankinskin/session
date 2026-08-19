//! Handshake replay cache tests (T5, R5). See spec `1bef7b3d-…` Validation
//! Strategy test matrix and Normative Requirements R5.

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

async fn do_handshake(
    supervisor: &Supervisor,
    init_id: i64,
) -> Value {
    let init = json!({
        "jsonrpc": "2.0",
        "id": init_id,
        "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "test-client" } }
    });
    assert!(
        supervisor.write_line(&init.to_string()).await,
        "write initialize failed"
    );
    let line = supervisor
        .read_line()
        .await
        .expect("child closed without responding to initialize");
    let resp: Value = serde_json::from_str(&line).unwrap();
    let notif =
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert!(
        supervisor.write_line(&notif.to_string()).await,
        "write notifications/initialized failed"
    );
    resp
}

async fn call_generation(
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

fn spawn_v1(
    shadow_root: &TempDir,
    canonical: &std::path::Path,
) -> Supervisor {
    write_exe(canonical, &fake_v1_bytes());
    let command = vec![canonical.to_string_lossy().to_string()];
    Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
        .unwrap()
}

/// R5 ordering: after a direct `swap_child()` call, the very next
/// `tools/call` succeeds — proving the new child already completed the
/// replayed handshake, since the fixture rejects `tools/call` observed
/// before `initialize` with a JSON-RPC error.
#[tokio::test]
async fn handshake_replayed_before_tool_calls() {
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    let supervisor = spawn_v1(&shadow_root, &canonical);

    do_handshake(&supervisor, 1).await;

    write_exe(&canonical, &fake_v2_bytes());
    let synthesized = supervisor.swap_child_with_drain_ms(200).await;
    assert!(synthesized.is_empty());

    // If the new child had received this tools/call before the replayed
    // initialize, the fixture would answer with an `error` object instead
    // of `result` (see fake-mcp-v{1,2}.rs `seen_initialize` guard).
    let resp = call_generation(&supervisor, 2, Duration::from_secs(2)).await;
    assert!(
        resp.get("error").is_none(),
        "new child rejected tools/call as pre-handshake: {resp}"
    );
    assert_eq!(resp["result"]["content"][0]["text"].as_str().unwrap(), "v2");

    let _ = supervisor.shutdown().await;
}

/// R5 suppression: the client-facing transcript across a swap contains
/// exactly one `initialize` response — the original. The replayed
/// `initialize` response from the new child is consumed internally and
/// never surfaces on `Supervisor::read_line()`.
#[tokio::test]
async fn handshake_response_never_forwarded() {
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    let supervisor = spawn_v1(&shadow_root, &canonical);

    let original_resp = do_handshake(&supervisor, 1).await;
    assert_eq!(original_resp["result"]["serverInfo"]["name"], "fake-mcp-v1");
    assert!(original_resp.get("protocolVersion").is_none()); // sanity: field lives under result

    write_exe(&canonical, &fake_v2_bytes());
    let synthesized = supervisor.swap_child_with_drain_ms(200).await;
    assert!(synthesized.is_empty());

    // The very next line the client reads after the swap must be the
    // tools/call response, not a leaked second initialize response (which
    // would carry `result.serverInfo`/`result.protocolVersion` instead of
    // `result.content`).
    let resp = call_generation(&supervisor, 2, Duration::from_secs(2)).await;
    assert!(
        resp["result"].get("serverInfo").is_none(),
        "a second initialize response leaked to the client: {resp}"
    );
    assert_eq!(resp["result"]["content"][0]["text"].as_str().unwrap(), "v2");

    let _ = supervisor.shutdown().await;
}

/// R5 divergence: v1 -> v2 declare different `serverInfo`. The swap must
/// still succeed and a divergence warning must be recorded, but the swap is
/// not aborted.
#[tokio::test]
async fn capability_divergence_logged_not_fatal() {
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    let supervisor = spawn_v1(&shadow_root, &canonical);

    do_handshake(&supervisor, 1).await;

    write_exe(&canonical, &fake_v2_bytes());
    let synthesized = supervisor.swap_child_with_drain_ms(200).await;
    assert!(synthesized.is_empty());

    // Divergence is recorded synchronously inside `swap_child_with_drain_ms`
    // (replay runs to completion before the swap returns), so no bounded
    // wait is needed here.
    let log = supervisor.divergence_log().await;
    assert!(
        log.iter().any(|l| l.contains("serverInfo")),
        "expected a serverInfo divergence entry, got: {log:?}"
    );

    // Swap still succeeded: the post-swap call is served by v2.
    let resp = call_generation(&supervisor, 2, Duration::from_secs(2)).await;
    assert_eq!(resp["result"]["content"][0]["text"].as_str().unwrap(), "v2");

    let _ = supervisor.shutdown().await;
}
