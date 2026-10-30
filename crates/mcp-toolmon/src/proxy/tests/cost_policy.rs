#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn missing_caller_model_is_rejected() {
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    match handle_client_message(call("read_file", None), Some(&g), &mut p, &mut pc) {
        (ClientAction::Respond(v), telemetry) => {
            assert_eq!(v["result"]["isError"], json!(true));
            let text = v["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.contains(CALLER_MODEL_ARG));
            let telemetry = telemetry.expect("expected telemetry for refused call");
            assert_eq!(telemetry.decision, "reject-missing-model");
            assert_eq!(telemetry.duration_ms, 0);
            assert_eq!(telemetry.response_bytes, Some(0));
        }
        other => panic!("expected Respond, got {other:?}"),
    }
}

#[test]
pub(super) fn missing_session_id_is_rejected() {
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    let mut request = call("read_file", Some("gpt-5-mini"));
    request["params"]["arguments"]
        .as_object_mut()
        .unwrap()
        .remove(SESSION_ID_ARG);

    match handle_client_message(request, Some(&g), &mut p, &mut pc) {
        (ClientAction::Respond(v), telemetry) => {
            assert_eq!(v["result"]["isError"], json!(true));
            let text = v["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.contains(SESSION_ID_ARG));
            assert_eq!(
                telemetry.expect("expected telemetry").decision,
                "reject-missing-session"
            );
        }
        other => panic!("expected Respond, got {other:?}"),
    }
}

#[test]
pub(super) fn expensive_measured_tool_is_refused() {
    // Build a gate with a rollup that measures read_file with cost 75
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir();
    let tid = std::thread::current().id();
    let path = dir.join(format!(
        "mcpcg-fixture-{:?}-{}-{}.json",
        tid,
        std::process::id(),
        n
    ));
    let rollup_path = dir.join(format!(
        "mcpcg-rollup-{:?}-{}-{}.json",
        tid,
        std::process::id(),
        n
    ));
    std::fs::write(
        &path,
        r#"{"models":[
                {"provider_id":"anthropic","model_id":"claude-opus-4-1","output_mtok":75.0},
                {"provider_id":"openai","model_id":"gpt-5-mini","output_mtok":2.0}
            ]}"#,
    )
    .unwrap();
    std::fs::write(
        &rollup_path,
        r#"{"report":{"tools":[
                {"tool_name":"read_file","call_count":10,"cost":75}
            ]}}"#,
    )
    .unwrap();
    let g = CostGatePolicy::new(
        Gate::load(
            std::path::Path::new(&path),
            toolmon_costgate::ModelBudgetCalibration::default(),
            Some(std::path::Path::new(&rollup_path)),
            None,
        )
        .unwrap(),
    );
    // Clean up temp files after loading
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&rollup_path);

    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    match handle_client_message(
        call("read_file", Some("claude-opus-4-1")),
        Some(&g),
        &mut p,
        &mut pc,
    ) {
        (ClientAction::Respond(v), telemetry) => {
            assert_eq!(v["result"]["isError"], json!(true));
            assert!(
                v["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_lowercase()
                    .contains("delegate")
            );
            assert_eq!(telemetry.expect("expected telemetry").decision, "delegate");
        }
        other => panic!("expected Respond, got {other:?}"),
    }
}

#[test]
pub(super) fn unmeasured_tool_fail_open() {
    let _routing = active_routing();
    // Without a rollup, even expensive models can call any tool (fail open)
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    match handle_client_message(
        call("read_file", Some("claude-opus-4-1")),
        Some(&g),
        &mut p,
        &mut pc,
    ) {
        (ClientAction::Forward(v), telemetry) => {
            let args = &v["params"]["arguments"];
            assert!(
                args.get(CALLER_MODEL_ARG).is_none(),
                "caller_model must be stripped"
            );
            assert!(
                telemetry.is_none(),
                "forwarded calls emit telemetry on response, not on forward"
            );
        }
        other => panic!("expected Forward (fail open), got {other:?}"),
    }
}

#[test]
pub(super) fn cheap_forwards_session_id_and_strips_proxy_arguments() {
    let _routing = active_routing();
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    let mut request = call("some_unknown_tool", Some("gpt-5-mini"));
    request["params"]["arguments"]
        .as_object_mut()
        .unwrap()
        .insert(GRANT_ID_ARG.to_string(), json!("grant-123"));
    // Use a light tool (cost 1) that gpt-5-mini (budget ~97) can afford
    match handle_client_message(request, Some(&g), &mut p, &mut pc) {
        (ClientAction::Forward(v), _) => {
            let args = &v["params"]["arguments"];
            assert!(
                args.get(CALLER_MODEL_ARG).is_none(),
                "caller_model must be stripped"
            );
            assert!(
                args.get(SESSION_ID_ARG).is_some(),
                "session_id must be forwarded"
            );
            assert!(
                args.get(GRANT_ID_ARG).is_none(),
                "grant_id must be stripped"
            );
        }
        other => panic!("expected Forward, got {other:?}"),
    }
}

#[test]
pub(super) fn forwarded_session_id_keeps_proxy_arguments_private() {
    let _routing = active_routing();
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    let mut request = call("some_unknown_tool", Some("gpt-5-mini"));
    request["params"]["arguments"]
        .as_object_mut()
        .unwrap()
        .insert(GRANT_ID_ARG.to_string(), json!("grant-123"));

    match handle_client_message(request, Some(&g), &mut p, &mut pc) {
        (ClientAction::Forward(v), _) => {
            let args = &v["params"]["arguments"];
            assert_eq!(args[SESSION_ID_ARG], json!(TEST_SESSION_ID));
            assert!(args.get(CALLER_MODEL_ARG).is_none());
            assert!(args.get(GRANT_ID_ARG).is_none());
        }
        other => panic!("expected Forward, got {other:?}"),
    }
}

#[test]
pub(super) fn expensive_light_tool_forwards() {
    let _routing = active_routing();
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    match handle_client_message(
        call("runSubagent", Some("claude-opus-4-1")),
        Some(&g),
        &mut p,
        &mut pc,
    ) {
        (ClientAction::Forward(_), _) => {}
        other => panic!("expected Forward, got {other:?}"),
    }
}

#[test]
pub(super) fn unknown_caller_model_is_rejected() {
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    match handle_client_message(
        call("read_file", Some("github-copilot")),
        Some(&g),
        &mut p,
        &mut pc,
    ) {
        (ClientAction::Respond(v), telemetry) => {
            assert_eq!(v["result"]["isError"], json!(true));
            let text = v["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.to_lowercase().contains("unknown caller_model"));
            assert_eq!(telemetry.expect("expected telemetry").decision, "reject");
        }
        other => panic!("expected Respond, got {other:?}"),
    }
}

#[test]
pub(super) fn parenthetical_client_qualifier_is_tolerated() {
    let _routing = active_routing();
    // "gpt-5-mini (copilot)" doesn't match exactly or by substring, but
    // stripping the trailing "(copilot)" qualifier resolves to "gpt-5-mini".
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    match handle_client_message(
        call("some_unknown_tool", Some("gpt-5-mini (copilot)")),
        Some(&g),
        &mut p,
        &mut pc,
    ) {
        (ClientAction::Forward(v), _) => {
            let args = &v["params"]["arguments"];
            assert!(
                args.get(CALLER_MODEL_ARG).is_none(),
                "caller_model must be stripped"
            );
        }
        other => panic!("expected Forward (allow after normalization), got {other:?}"),
    }

    // The soft warning surfaces on the eventual server response.
    let resp = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "content": [{ "type": "text", "text": "ok" }] }
    });
    let (out, telemetry) = handle_server_message(resp, Some(&g), &mut p, &mut pc);
    assert!(
        out["result"]["costGateWarning"]
            .as_str()
            .unwrap()
            .contains("normalized")
    );
    assert_eq!(telemetry.unwrap().decision, "allow-normalized");
}

#[test]
pub(super) fn space_and_underscore_separators_are_normalized() {
    let _routing = active_routing();
    // The gate's own key normalization folds spaces/underscores to hyphens,
    // so "Claude_Opus 4 1" resolves directly to "claude-opus-4-1" without
    // needing the proxy's fallback (and therefore without a warning).
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    match handle_client_message(
        call("runSubagent", Some("Claude_Opus 4 1")),
        Some(&g),
        &mut p,
        &mut pc,
    ) {
        (ClientAction::Forward(v), _) => {
            let args = &v["params"]["arguments"];
            assert!(
                args.get(CALLER_MODEL_ARG).is_none(),
                "caller_model must be stripped"
            );
        }
        other => panic!("expected Forward (allow after normalization), got {other:?}"),
    }
    let resp = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "content": [{ "type": "text", "text": "ok" }] }
    });
    let (out, _) = handle_server_message(resp, Some(&g), &mut p, &mut pc);
    assert!(out["result"]["costGateWarning"].is_null());
}

#[test]
pub(super) fn genuinely_unknown_model_still_rejected_after_normalization() {
    // Normalizing "Totally Unknown Model (copilot)" still doesn't match
    // anything in the price table, so the call is rejected as before.
    let g = test_gate();
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    match handle_client_message(
        call("read_file", Some("Totally Unknown Model (copilot)")),
        Some(&g),
        &mut p,
        &mut pc,
    ) {
        (ClientAction::Respond(v), telemetry) => {
            assert_eq!(v["result"]["isError"], json!(true));
            let text = v["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.to_lowercase().contains("unknown caller_model"));
            assert_eq!(telemetry.expect("expected telemetry").decision, "reject");
        }
        other => panic!("expected Respond, got {other:?}"),
    }
}

#[test]
pub(super) fn no_gate_is_passthrough() {
    let mut p = PendingList::default();
    let mut pc = PendingCalls::default();
    match handle_client_message(call("read_file", None), None, &mut p, &mut pc) {
        (ClientAction::Forward(_), None) => {}
        other => panic!("expected Forward with no telemetry, got {other:?}"),
    }
}
