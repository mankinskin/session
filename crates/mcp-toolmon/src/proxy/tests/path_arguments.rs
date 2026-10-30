#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn fs_delete_dir_accepts_absolute_paths_from_unassigned_checkout() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let main_checkout = temp.path().join("repository");
    let target = main_checkout.join("context-engine").join(".ticket");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(main_checkout.join(".git")).unwrap();
    std::fs::create_dir_all(main_checkout.join(".workflow-tools/session")).unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

    let mut request = call("fs_delete_dir", Some("gpt-5-mini"));
    request["params"]["arguments"]["path"] = json!(target);
    request["params"]["arguments"]["root"] = json!(main_checkout);
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "fs_delete_dir",
        json!({
            "path": {"type": "string"},
            "recursive": {"type": "boolean"},
            "root": {"type": "string"}
        }),
    ) else {
        panic!("fs_delete_dir should forward from an unassigned checkout");
    };
    assert_eq!(forwarded["params"]["arguments"]["path"], json!(target));
    assert_eq!(
        forwarded["params"]["arguments"]["root"],
        json!(main_checkout)
    );

    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn registered_paths_validate_without_changing_declared_arguments() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    let nested = worktree.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

    let mut request = call("peek_read", Some("gpt-5-mini"));
    let supplied_path = nested.join("..").join("nested");
    request["params"]["arguments"]["path"] = json!(supplied_path);
    request["params"]["arguments"]["untouched"] = json!("value");
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "peek_read",
        json!({
            "path": {"type": "string"},
            "untouched": {"type": "string"}
        }),
    ) else {
        panic!("expected forwarded request");
    };
    assert_eq!(
        forwarded["params"]["arguments"]["path"],
        json!(supplied_path)
    );
    assert_eq!(
        forwarded["params"]["arguments"]["untouched"],
        json!("value")
    );
    assert!(forwarded["params"]["arguments"].get("workspace").is_none());

    let mut request = call("peek_read", Some("gpt-5-mini"));
    request["params"]["arguments"]["path"] = json!(nested);
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "peek_read",
        json!({"path": {"type": "string"}}),
    ) else {
        panic!("expected forwarded request");
    };
    assert_eq!(forwarded["params"]["arguments"]["path"], json!(nested));

    let outside = main_checkout.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let mut request = call("peek_read", Some("gpt-5-mini"));
    request["params"]["arguments"]["path"] = json!(outside);
    let text = response_text(
        route_with_schema(
            request,
            &test_gate(),
            "peek_read",
            json!({"path": {"type": "string"}}),
        )
        .0,
    );
    assert!(text.contains("outside"));

    let mut request = call("unknown_tool", Some("gpt-5-mini"));
    request["params"]["arguments"]["workspace"] = json!("unchanged");
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "unknown_tool",
        json!({"untouched": {"type": "string"}}),
    ) else {
        panic!("expected forwarded request");
    };
    assert_eq!(
        forwarded["params"]["arguments"]["workspace"],
        json!("unchanged")
    );
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn relative_rename_paths_from_main_checkout_are_forwarded_unchanged() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let cwd = std::env::current_dir().unwrap();
    let temp = tempfile::tempdir_in(&cwd).unwrap();
    let main_checkout = temp.path().join("repository");
    let source = main_checkout.join("nested").join("ROADMAP.md");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::create_dir_all(main_checkout.join(".git")).unwrap();
    std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
    std::fs::write(&source, "roadmap").unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

    let mut request = call("fs_rename_file", Some("gpt-5-mini"));
    request["params"]["arguments"]["from"] = json!(source.strip_prefix(&cwd).unwrap());
    request["params"]["arguments"]["to"] = json!("ROADMAP.v1.md");
    request["params"]["arguments"]["root"] = json!(main_checkout);
    let mut expected = request["params"]["arguments"].clone();
    expected.as_object_mut().unwrap().remove(CALLER_MODEL_ARG);
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "fs_rename_file",
        json!({"from": {}, "to": {}, "root": {}}),
    ) else {
        panic!("relative rename inside actual checkout should forward");
    };
    assert_eq!(forwarded["params"]["arguments"], expected);
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn worktree_rename_to_missing_destination_preserves_arguments() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    let source = worktree.join("nested").join("ROADMAP.md");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "roadmap").unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

    let mut request = call("fs_rename_file", Some("gpt-5-mini"));
    request["params"]["arguments"]["from"] = json!(source);
    request["params"]["arguments"]["to"] = json!("ROADMAP.v1.md");
    request["params"]["arguments"]["root"] = json!(worktree);
    let mut expected = request["params"]["arguments"].clone();
    expected.as_object_mut().unwrap().remove(CALLER_MODEL_ARG);
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "fs_rename_file",
        json!({"from": {}, "to": {}, "root": {}}),
    ) else {
        panic!("rename to a new sibling in the worktree should forward");
    };
    assert_eq!(forwarded["params"]["arguments"], expected);
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn mutations_reject_destinations_outside_assigned_worktree() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    for (tool, destination) in [
        (
            "fs_move_file",
            json!(main_checkout.join("missing").join("new.md")),
        ),
        ("fs_copy_file", json!(main_checkout.join("README.md"))),
        ("fs_rename_file", json!(main_checkout.join("new.md"))),
        ("fs_rename_file", json!("../outside.md")),
    ] {
        let mut request = call(tool, Some("gpt-5-mini"));
        request["params"]["arguments"]["from"] = json!(worktree.join("README.md"));
        request["params"]["arguments"]["to"] = destination;
        request["params"]["arguments"]["root"] = json!(worktree);
        let (action, _) = route_with_schema(
            request,
            &test_gate(),
            tool,
            json!({"from": {}, "to": {}, "root": {}}),
        );
        let text = response_text(action);
        assert!(
            text.contains("PATH_OUTSIDE_SESSION_WORKTREE"),
            "{tool}: {text}"
        );
    }
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn copy_from_existing_file_outside_worktree_into_worktree_is_forwarded() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    let external = temp.path().join("external.md");
    std::fs::write(&external, "external\n").unwrap();
    for source in [main_checkout.join("README.md"), external] {
        let destination = worktree.join("copied.md");
        let request = copy_request(json!(source), json!(destination), &worktree);
        let ClientAction::Forward(forwarded) = route_copy(request) else {
            panic!(
                "copy source outside worktree should forward: {}",
                source.display()
            );
        };
        assert_eq!(forwarded["params"]["arguments"]["from"], json!(source));
        assert_eq!(forwarded["params"]["arguments"]["to"], json!(destination));
    }
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn copy_into_missing_nested_worktree_directory_is_forwarded() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    let destination = worktree.join("a").join("b").join("copied.md");
    let request = copy_request(
        json!(main_checkout.join("README.md")),
        json!(destination),
        &worktree,
    );
    assert!(matches!(route_copy(request), ClientAction::Forward(_)));
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn copy_rejects_missing_or_directory_source() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    for source in [
        temp.path().join("missing.md"),
        main_checkout.join("missing").join("nested.md"),
        temp.path().to_path_buf(),
    ] {
        let request = copy_request(json!(source), json!(worktree.join("copied.md")), &worktree);
        let text = response_text(route_copy(request));
        assert!(
            text.contains("copy source") && text.contains("not a file"),
            "{}: {text}",
            source.display()
        );
    }
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn copy_with_outside_source_still_rejects_outside_destination() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    let request = copy_request(
        json!(main_checkout.join("README.md")),
        json!(temp.path().join("escaped.md")),
        &worktree,
    );
    let text = response_text(route_copy(request));
    assert!(text.contains("PATH_OUTSIDE_SESSION_WORKTREE"), "{text}");
    assert!(
        text.contains("Budget-offset grants do not authorize filesystem access"),
        "{text}"
    );
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn move_from_outside_worktree_is_still_rejected() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    let mut request = call("fs_move_file", Some("gpt-5-mini"));
    request["params"]["arguments"]["from"] = json!(main_checkout.join("README.md"));
    request["params"]["arguments"]["to"] = json!(worktree.join("moved.md"));
    request["params"]["arguments"]["root"] = json!(worktree);
    let (action, _) = route_with_schema(
        request,
        &test_gate(),
        "fs_move_file",
        json!({"from": {}, "to": {}, "root": {}}),
    );
    assert!(response_text(action).contains("PATH_OUTSIDE_SESSION_WORKTREE"));
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn mutation_to_missing_nested_worktree_destination_is_forwarded_unchanged() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    let mut request = call("fs_move_file", Some("gpt-5-mini"));
    let destination = worktree.join("new").join("nested").join("file.md");
    request["params"]["arguments"]["from"] = json!(worktree.join("README.md"));
    request["params"]["arguments"]["to"] = json!(destination);
    request["params"]["arguments"]["root"] = json!(worktree);
    let (ClientAction::Forward(forwarded), _) = route_with_schema(
        request,
        &test_gate(),
        "fs_move_file",
        json!({"from": {}, "to": {}, "root": {}}),
    ) else {
        panic!("missing destination inside assigned worktree should forward");
    };
    assert_eq!(forwarded["params"]["arguments"]["to"], json!(destination));
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn relative_mutation_path_is_not_reinterpreted_as_worktree_relative() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, _worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    let mut request = call("fs_delete_file", Some("gpt-5-mini"));
    request["params"]["arguments"]["path"] = json!("README.md");
    let (action, _) =
        route_with_schema(request, &test_gate(), "fs_delete_file", json!({"path": {}}));
    assert!(response_text(action).contains("PATH_OUTSIDE_SESSION_WORKTREE"));
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}

#[test]
pub(super) fn junction_or_symlink_escape_is_rejected_even_for_missing_destination() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_temp, main_checkout, worktree) = routing_fixture(SessionWorktreeStatus::Active, false);
    let outside = main_checkout.join("outside");
    let link = worktree.join("escape");
    std::fs::create_dir_all(&outside).unwrap();
    #[cfg(windows)]
    {
        let output = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };
    let mut request = call("fs_move_file", Some("gpt-5-mini"));
    request["params"]["arguments"]["from"] = json!(worktree.join("README.md"));
    request["params"]["arguments"]["to"] = json!(link.join("missing").join("new.md"));
    request["params"]["arguments"]["root"] = json!(worktree);
    let (action, _) = route_with_schema(
        request,
        &test_gate(),
        "fs_move_file",
        json!({"from": {}, "to": {}, "root": {}}),
    );
    assert!(response_text(action).contains("PATH_OUTSIDE_SESSION_WORKTREE"));
    unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
}
