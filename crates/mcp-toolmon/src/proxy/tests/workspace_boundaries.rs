#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn workspace_defaults_are_validated_against_actual_working_directory() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout) = main_checkout_fixture();
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
    let main_checkout_workspace = normalized(&main_checkout);
    for workspace in [
        None,
        Some("default"),
        Some(""),
        Some(main_checkout_workspace.as_str()),
    ] {
        let mut request = call("update_ticket", Some("gpt-5-mini"));
        if let Some(workspace) = workspace {
            request["params"]["arguments"]["workspace"] = json!(workspace);
        }
        let (action, _) = route_with_schema(
            request,
            &test_gate(),
            "update_ticket",
            json!({"workspace": {"type": "string"}}),
        );
        if workspace == Some(main_checkout_workspace.as_str()) {
            let ClientAction::Forward(forwarded) = action else {
                panic!("explicit checkout path should forward");
            };
            assert_eq!(
                forwarded["params"]["arguments"]["workspace"],
                json!(workspace)
            );
        } else {
            assert!(response_text(action).contains("PATH_OUTSIDE_SESSION_WORKTREE"));
        }
    }
    unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
}

#[test]
pub(super) fn anchor_falls_back_to_the_process_working_directory() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
    let resolver = anchored_resolver().expect("working directory should anchor the resolver");
    let candidates = resolver
        .refused_candidates(DEFAULT_STORE_DIR)
        .expect("candidates should enumerate");
    let working_dir = canonicalized_normalized(&std::env::current_dir().unwrap());
    assert!(
        candidates
            .iter()
            .any(|candidate| normalized(candidate).starts_with(&working_dir)),
        "expected a candidate anchored on {working_dir}, got {candidates:?}"
    );
}

#[test]
pub(super) fn registered_main_checkout_is_not_overridden_by_directory_candidates() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, _worktree) = routing_fixture(SessionWorktreeStatus::Active, true);
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", main_checkout) };
    let (ClientAction::Forward(_), _) = route(allowed_call(), &test_gate()) else {
        panic!("an explicit main-checkout assignment should be forwarded");
    };
    unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
}

#[test]
pub(super) fn inactive_registered_worktree_is_rejected() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, _worktree) =
        routing_fixture(SessionWorktreeStatus::Superseded, false);
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", main_checkout) };
    let text = response_text(route(allowed_call(), &test_gate()).0);
    assert!(text.contains("inactive worktree assignment"), "{text}");
    unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
}

#[test]
pub(super) fn absolute_workspace_is_contained_by_session_worktree() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    let inside = worktree.join("nested");
    std::fs::create_dir_all(&inside).unwrap();
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };

    let mut request = allowed_call();
    request["params"]["arguments"]["workspace"] = json!(inside.to_string_lossy());
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "read_file",
        json!({"workspace": {"type": "string"}}),
    ) else {
        panic!("expected forwarded request");
    };
    assert_eq!(forwarded["params"]["arguments"]["workspace"], json!(inside));

    let outside = main_checkout.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let mut request = allowed_call();
    request["params"]["arguments"]["workspace"] = json!(outside.to_string_lossy());
    let text = response_text(
        route_with_schema(
            request,
            &test_gate(),
            "read_file",
            json!({"workspace": {"type": "string"}}),
        )
        .0,
    );
    assert!(text.contains(outside.to_string_lossy().as_ref()));
    assert!(text.contains("resolved session worktree"));

    let mut request = allowed_call();
    request["params"]["arguments"]["workspace"] = json!(main_checkout.to_string_lossy());
    let text = response_text(
        route_with_schema(
            request,
            &test_gate(),
            "read_file",
            json!({"workspace": {"type": "string"}}),
        )
        .0,
    );
    assert!(text.contains(main_checkout.to_string_lossy().as_ref()));
    assert!(text.contains("resolved session worktree"));

    let mut request = allowed_call();
    request["params"]["arguments"]["workspace"] = json!("nested");
    let (action, _) = route_with_schema(
        request,
        &test_gate(),
        "read_file",
        json!({"workspace": {"type": "string"}}),
    );
    assert!(response_text(action).contains("PATH_OUTSIDE_SESSION_WORKTREE"));
    unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
}

#[test]
pub(super) fn resolved_store_root_is_logged() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", main_checkout) };
    let mut request = allowed_call();
    request["params"]["arguments"]["workspace"] = json!(normalized(&worktree));
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "read_file",
        json!({"workspace": {"type": "string"}}),
    ) else {
        panic!("expected forwarded request");
    };
    assert_eq!(
        forwarded["params"]["arguments"]["workspace"],
        json!(normalized(&worktree))
    );
    unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
}
