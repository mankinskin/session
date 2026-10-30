#[allow(unused_imports)]
use super::*;

#[tokio::test]
pub(super) async fn unassign_deleted_checkout_returns_main_and_retains_audit() {
    let dir = tempdir().unwrap();
    let session_id = "11111111-1111-4111-8111-111111111111";
    let worktree = dir
        .path()
        .join(".worktrees")
        .join(session_id)
        .join("unassign");
    for args in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=tests",
            "-c",
            "user.email=tests@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(dir.path())
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    assert!(
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["worktree", "add", "--quiet", "-b", "feature/unassign"])
            .arg(&worktree)
            .status()
            .unwrap()
            .success()
    );
    let store = dir.path().join(".workflow-tools/session");
    let config = SessionStoreConfig::new(&store);
    config
        .check_in_worktree(SessionWorktreeCheckInRequest {
            session_id: session_id.to_string(),
            owner_id: "agent".to_string(),
            ticket_id: "ticket".to_string(),
            worktree_path: worktree.clone(),
            branch: "feature/unassign".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();
    std::fs::remove_dir_all(&worktree).unwrap();
    let server = SessionServer::new(store);
    let make_input = |owner: &str| UnassignInput {
        workspace: dir.path().to_string_lossy().to_string(),
        session_id: session_id.to_string(),
        owner_id: owner.to_string(),
        ticket_id: "ticket".to_string(),
        expected_worktree_path: worktree.to_string_lossy().to_string(),
        expected_branch: "feature/unassign".to_string(),
        reason: "Requester selected main".to_string(),
    };
    assert!(
        server
            .session_unassign(Parameters(make_input("other")))
            .await
            .is_err()
    );
    assert!(config.registered_worktree(session_id).unwrap().is_some());
    let payload = extract_json(
        server
            .session_unassign(Parameters(make_input("agent")))
            .await
            .unwrap(),
    );
    assert_eq!(payload["session_id"], session_id);
    assert_eq!(
        payload["main_checkout"],
        dir.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert!(PathBuf::from(payload["audit_path"].as_str().unwrap()).is_file());
    assert!(config.registered_worktree(session_id).unwrap().is_none());
}

#[tokio::test]
pub(super) async fn check_in_then_lookup() {
    let dir = tempdir().unwrap();
    let workspace_root = dir.path();
    let store_root = workspace_root.join(".workflow-tools/session");
    let session_id = "11111111-1111-4111-8111-111111111111";
    let worktree = workspace_root
        .join(".worktrees")
        .join(session_id)
        .join("check-in");
    init_checkout_with_worktree(workspace_root, &worktree, "feature/x");
    let server = SessionServer::new(store_root.clone());

    let receipt = server
        .session_check_in(Parameters(CheckInInput {
            workspace: workspace_root.display().to_string(),
            session_id: session_id.to_string(),
            owner_id: "agent".to_string(),
            ticket_id: "t1".to_string(),
            worktree_path: worktree.to_string_lossy().to_string(),
            branch: "feature/x".to_string(),
            predecessor_session_id: None,
        }))
        .await
        .expect("check-in");
    assert!(!receipt.is_error.unwrap_or(false));

    let lookup = server
        .session_lookup(Parameters(LookupInput {
            session_id: session_id.to_string(),
        }))
        .await
        .expect("lookup");
    assert!(!lookup.is_error.unwrap_or(false));
}

#[test]
pub(super) fn workspace_validation_rejects_ambient_aliases() {
    for value in [None, Some(""), Some("default"), Some("..")] {
        let err = workspace::validate_explicit_workspace_selector(value)
            .expect_err("should reject ambient selector");
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("invalid workspace selector"),
            "error should mention 'invalid workspace selector': {err_msg}"
        );
        assert!(
            err_msg.contains("entity creation requires an explicit workspace path"),
            "error should state the requirement: {err_msg}"
        );
    }
}

#[test]
pub(super) fn workspace_validation_accepts_current_directory() {
    workspace::validate_explicit_workspace_selector(Some("."))
        .expect("'.' should resolve to the MCP server's cwd");
}

#[test]
pub(super) fn explicit_dot_workspace_uses_absolute_canonical_session_store() {
    let server = SessionServer::new(PathBuf::from("."));
    let config = server.config_for_workspace(".").unwrap();
    let current_dir = std::env::current_dir().unwrap();

    assert_eq!(
        config.root,
        workspace::canonical_store_root(&current_dir, ".session")
    );
}

#[tokio::test]
pub(super) async fn explicit_dot_workspace_runtime_init_reads_back_from_selected_store() {
    const CHILD_ENV: &str = "SESSION_MCP_DOT_SELECTOR_CHILD";
    const SESSION_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    if std::env::var_os(CHILD_ENV).is_none() {
        let temp = tempdir().unwrap();
        let parent = temp.path().join("parent");
        let selected = parent.join("selected");
        let sibling = parent.join("sibling");
        std::fs::create_dir_all(&selected).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();

        let output = Command::new(std::env::current_exe().unwrap())
            .arg("explicit_dot_workspace_runtime_init_reads_back_from_selected_store")
            .arg("--nocapture")
            .env(CHILD_ENV, "1")
            .current_dir(&selected)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child regression failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let store_root = workspace::canonical_store_root(&selected, ".session");
        let context_path = store_root
            .join("sessions")
            .join(SESSION_ID)
            .join("session.json");
        let context: Value = serde_json::from_slice(&std::fs::read(context_path).unwrap()).unwrap();
        assert_eq!(context["session_id"], SESSION_ID);
        assert!(!parent.join(".workflow-tools/session").exists());
        assert!(!sibling.join(".workflow-tools/session").exists());
        return;
    }

    let selected = std::env::current_dir().unwrap();
    let server = SessionServer::new(
        selected
            .parent()
            .unwrap()
            .join("sibling/.workflow-tools/session"),
    );
    let result = server
        .session_runtime_init(Parameters(RuntimeInitInput {
            workspace: ".".to_string(),
            session_id: SESSION_ID.to_string(),
            predecessor_run_id: None,
            force_new_run: false,
        }))
        .await
        .expect("initialize selected workspace runtime");
    assert!(!result.is_error.unwrap_or(false));
}
