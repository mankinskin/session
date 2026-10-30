#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn test_handle_client_message_expensive_model_refused() {
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
    let msg = tools_call_request(1, "get_ticket_description", "claude-opus-4-8");
    let mut pending = PendingList::default();
    let mut pending_calls = PendingCalls::default();
    let (action, _telemetry) =
        handle_client_message(msg, Some(&policy), &mut pending, &mut pending_calls);

    // Should be refused with Delegate guidance.
    let error_text = extract_error_text(action);
    assert!(error_text.contains("cost 80"));
    assert!(error_text.contains("budget"));
}

#[test]
pub(super) fn test_handle_client_message_cheap_model_allowed() {
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
    let msg = tools_call_request(1, "get_ticket_description", "claude-haiku-3-7");
    let mut pending = PendingList::default();
    let mut pending_calls = PendingCalls::default();
    let (action, _telemetry) =
        handle_client_message(msg, Some(&policy), &mut pending, &mut pending_calls);

    // Should be forwarded (allowed).
    match action {
        ClientAction::Forward(val) => {
            assert!(val["params"]["arguments"].get("caller_model").is_none());
            assert!(val["params"]["arguments"].get("session_id").is_some());
        }
        ClientAction::Respond(val) => {
            panic!("Expected Forward, got Respond: {:?}", val);
        }
    }
}

#[test]
pub(super) fn test_handle_client_message_no_gate_fail_open() {
    let msg = tools_call_request(1, "some_tool", "claude-opus-4-8");
    let mut pending = PendingList::default();
    let mut pending_calls = PendingCalls::default();
    let (action, _telemetry) = handle_client_message(msg, None, &mut pending, &mut pending_calls);

    // No gate (fail-open) → should forward unchanged.
    match action {
        ClientAction::Forward(_) => {}
        ClientAction::Respond(val) => {
            panic!("Expected Forward in fail-open mode, got Respond: {:?}", val);
        }
    }
}
