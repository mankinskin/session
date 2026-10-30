#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn session_resolving_to_worktree_does_not_insert_workspace_argument() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, _worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        call("get_ticket", Some("gpt-5-mini")),
        &test_gate(),
        "get_ticket",
        json!({"workspace": {"type": "string"}}),
    ) else {
        panic!("expected forwarded request");
    };
    assert!(forwarded["params"]["arguments"].get("workspace").is_none());
    unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
}

#[test]
pub(super) fn reads_from_main_checkout_are_forwarded() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout) = main_checkout_fixture();
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
    for tool in [
        "list_workspaces",
        "get_ticket",
        "session_lookup",
        "spec_get",
        "test_list_specs",
    ] {
        let (ClientAction::Forward(forwarded), _) = route_with_schema(
            call(tool, Some("gpt-5-mini")),
            &test_gate(),
            tool,
            json!({"workspace": {"type": "string"}}),
        ) else {
            panic!("{tool} should forward from the main checkout");
        };
        assert!(forwarded["params"]["arguments"].get("workspace").is_none());
    }
    unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
}

#[test]
pub(super) fn deleted_worktree_allows_only_explicit_main_store_unassignment() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    std::fs::remove_dir_all(&worktree).unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    let mut request = call("session_unassign", Some("gpt-5-mini"));
    request["params"]["arguments"]["workspace"] = json!(main_checkout);
    let (action, _) = route_with_schema(
        request,
        &test_gate(),
        "session_unassign",
        json!({"workspace": {"type": "string"}}),
    );
    assert!(matches!(action, ClientAction::Forward(_)));
    for workspace in [
        None,
        Some(PathBuf::from("")),
        Some(PathBuf::from("default")),
        Some(PathBuf::from("..")),
        Some(worktree.clone()),
        Some(main_checkout.join("other-repo")),
    ] {
        let mut request = call("session_unassign", Some("gpt-5-mini"));
        if let Some(workspace) = workspace {
            request["params"]["arguments"]["workspace"] = json!(workspace);
        }
        let (action, _) = route_with_schema(
            request,
            &test_gate(),
            "session_unassign",
            json!({"workspace": {"type": "string"}}),
        );
        assert!(matches!(action, ClientAction::Respond(_)));
    }
    let mut request = call("update_ticket", Some("gpt-5-mini"));
    request["params"]["arguments"]["workspace"] = json!(main_checkout);
    assert!(matches!(
        route_with_schema(
            request.clone(),
            &test_gate(),
            "update_ticket",
            json!({"workspace": {"type": "string"}}),
        )
        .0,
        ClientAction::Respond(_)
    ));
    SessionStoreConfig::new(main_checkout.join(".session"))
        .unassign_worktree(session_api::SessionWorktreeUnassignRequest {
            session_id: TEST_SESSION_ID.to_string(),
            owner_id: "agent".to_string(),
            ticket_id: "ticket".to_string(),
            expected_worktree_path: worktree,
            expected_branch: "agent/test".to_string(),
            reason: "Requester selected main".to_string(),
        })
        .unwrap();
    assert!(matches!(
        route_with_schema(
            request,
            &test_gate(),
            "update_ticket",
            json!({"workspace": {"type": "string"}}),
        )
        .0,
        ClientAction::Forward(_)
    ));
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn leftover_worktree_and_captured_metadata_do_not_reject_canonical_store_calls() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, _worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    std::fs::remove_file(
        main_checkout
            .join(".session/local/worktrees")
            .join(format!("{TEST_SESSION_ID}.json")),
    )
    .unwrap();
    let workspace = main_checkout.join("workflow-tools");
    std::fs::create_dir_all(&workspace).unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    for tool in ["get_ticket", "spec_get", "test_get_spec", "update_ticket"] {
        let mut request = call(tool, Some("gpt-5-mini"));
        request["params"]["arguments"]["workspace"] = json!(workspace);
        let (action, _) = route_with_schema(
            request,
            &test_gate(),
            tool,
            json!({"workspace": {"type": "string"}}),
        );
        assert!(matches!(action, ClientAction::Forward(_)), "{tool}");
    }
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn unassigned_session_mutations_are_forwarded_from_main_checkout() {
    // Worktree assignment is opt-in: a session that never called
    // session_check_in is not blocked, it simply hasn't opted into
    // isolation. Mutations proceed against the main checkout, same as
    // reads.
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout) = main_checkout_fixture();
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
    for tool in ["update_ticket", "unknown_tool", "get_unknown"] {
        let mut request = call(tool, Some("gpt-5-mini"));
        request["params"]["arguments"]["workspace"] = json!(normalized(&main_checkout));
        let (ClientAction::Forward(forwarded), _) = route_with_schema(
            request,
            &test_gate(),
            tool,
            json!({"workspace": {"type": "string"}}),
        ) else {
            panic!("{tool} should forward from an unassigned session");
        };
        assert_eq!(
            forwarded["params"]["arguments"]["workspace"],
            json!(normalized(&main_checkout))
        );
    }
    unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
}

#[test]
pub(super) fn unassigned_session_mutation_works_without_worktrees_directory() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let main_checkout = temp.path().join("repository");
    std::fs::create_dir_all(main_checkout.join(".git")).unwrap();
    std::fs::create_dir_all(main_checkout.join(".workflow-tools/session")).unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

    let mut request = call("update_ticket", Some("gpt-5-mini"));
    request["params"]["arguments"]["workspace"] = json!(normalized(&main_checkout));
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "update_ticket",
        json!({"workspace": {"type": "string"}}),
    ) else {
        panic!("unassigned mutation should use a git checkout without worktrees");
    };
    assert_eq!(
        forwarded["params"]["arguments"]["workspace"],
        json!(normalized(&main_checkout))
    );

    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn assigned_session_with_vanished_worktree_blocks_mutation() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    std::fs::remove_dir_all(&worktree).unwrap();
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
    let (action, _) = route_with_schema(
        call("update_ticket", Some("gpt-5-mini")),
        &test_gate(),
        "update_ticket",
        json!({"workspace": {"type": "string"}}),
    );
    let text = response_text(action);
    assert!(text.to_ascii_lowercase().contains("worktree"), "{text}");
    unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
}

#[test]
pub(super) fn unassigned_session_check_in_with_direct_worktree_workspace_is_forwarded() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let main_checkout = temp.path().join("repository");
    let worktree = main_checkout.join(".worktrees").join("bootstrap");
    std::fs::create_dir_all(main_checkout.join(".git")).unwrap();
    std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(
        worktree.join(".git"),
        "gitdir: ../../.git/worktrees/bootstrap\n",
    )
    .unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

    let mut request = call("session_check_in", Some("gpt-5-mini"));
    request["params"]["arguments"]["workspace"] = json!(normalized(&worktree));
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "session_check_in",
        json!({"workspace": {"type": "string"}}),
    ) else {
        panic!("expected forwarded request");
    };
    assert_eq!(
        forwarded["params"]["arguments"]["workspace"],
        json!(normalized(&worktree))
    );
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn unassigned_session_check_in_with_nested_worktree_workspace_is_forwarded() {
    // The nested layout (`.worktrees/<session-uuid>/<slug>`) is the
    // documented default; the bootstrap escape hatch must accept it, not
    // just the legacy flat `.worktrees/<slug>` layout.
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let main_checkout = temp.path().join("repository");
    let worktree = main_checkout
        .join(".worktrees")
        .join(TEST_SESSION_ID)
        .join("topic-slug");
    std::fs::create_dir_all(main_checkout.join(".git")).unwrap();
    std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(
        worktree.join(".git"),
        "gitdir: ../../../.git/worktrees/topic-slug\n",
    )
    .unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

    let mut request = call("session_check_in", Some("gpt-5-mini"));
    request["params"]["arguments"]["workspace"] = json!(normalized(&worktree));
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "session_check_in",
        json!({"workspace": {"type": "string"}}),
    ) else {
        panic!("expected forwarded request");
    };
    assert_eq!(
        forwarded["params"]["arguments"]["workspace"],
        json!(normalized(&worktree))
    );
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn unassigned_session_check_in_rejects_non_direct_worktree_target() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let main_checkout = temp.path().join("repository");
    let worktree = main_checkout.join(".worktrees").join("bootstrap");
    let nested = worktree.join("nested");
    std::fs::create_dir_all(main_checkout.join(".git")).unwrap();
    std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        worktree.join(".git"),
        "gitdir: ../../.git/worktrees/bootstrap\n",
    )
    .unwrap();
    std::fs::write(nested.join(".git"), "gitdir: ../../.git/fake\n").unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

    let mut request = call("session_check_in", Some("gpt-5-mini"));
    request["params"]["arguments"]["workspace"] = json!(nested.to_string_lossy());
    let text = response_text(
        route_with_schema(
            request,
            &test_gate(),
            "session_check_in",
            json!({"workspace": {"type": "string"}}),
        )
        .0,
    );
    assert!(text.contains("must be a direct child"));
    assert!(text.contains(&normalized(&main_checkout.join(".worktrees"))));
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}
