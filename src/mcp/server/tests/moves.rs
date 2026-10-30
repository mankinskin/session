#[allow(unused_imports)]
use super::*;

#[tokio::test]
pub(super) async fn move_preflight_and_apply_roundtrip() {
    let temp = tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    Command::new("git")
        .current_dir(&repo_root)
        .args(["init"])
        .status()
        .expect("git init")
        .success()
        .then_some(())
        .expect("git init failed");

    let source_store_root = repo_root.join(".workflow-tools/session");
    std::fs::create_dir_all(&source_store_root).unwrap();
    let target_workspace_root = repo_root.join("target-workspace");
    std::fs::create_dir_all(target_workspace_root.join(".workflow-tools/session")).unwrap();

    let session_id = "7b3a7c62-1f3f-45d6-b8a1-f2b83e3d9f71";
    let config = SessionStoreConfig::new(source_store_root.clone());
    seed(&config, session_id, "agent-3");

    let server = SessionServer::new(source_store_root.clone());
    let preflight = server
        .session_move_preflight(Parameters(SessionMoveInput {
            id: session_id.to_string(),
            to_workspace_root: target_workspace_root.to_string_lossy().to_string(),
        }))
        .await
        .expect("move preflight");
    let preflight_json = extract_json(preflight);
    assert_eq!(preflight_json["status"], "ok");
    assert_eq!(preflight_json["mode"], "preflight");
    assert!(preflight_json["plan"]["supported"].as_bool().unwrap());

    let apply = server
        .session_move_apply(Parameters(SessionMoveInput {
            id: session_id.to_string(),
            to_workspace_root: target_workspace_root.to_string_lossy().to_string(),
        }))
        .await
        .expect("move apply");
    let apply_json = extract_json(apply);
    assert_eq!(apply_json["status"], "ok");
    assert_eq!(apply_json["mode"], "apply");
    assert!(apply_json["outcome"]["journal"]["id"].is_string());

    let target_config =
        SessionStoreConfig::new(target_workspace_root.join(".workflow-tools/session"));
    assert!(matches!(
        config.read_session(session_id),
        Err(SessionError::NotFound { .. })
    ));
    assert_eq!(
        target_config.read_session(session_id).unwrap().session_id,
        session_id
    );
}

#[tokio::test]
pub(super) async fn move_targets_reject_ambient_aliases_before_source_access() {
    let temp = tempdir().unwrap();
    let missing_store = temp.path().join("not-created-store");
    let server = SessionServer::new(missing_store.clone());

    for selector in ["", "  ", "default", ".."] {
        let input = SessionMoveInput {
            id: "7b3a7c62-1f3f-45d6-b8a1-f2b83e3d9f71".to_string(),
            to_workspace_root: selector.to_string(),
        };
        assert!(
            server
                .session_move_preflight(Parameters(input))
                .await
                .is_err()
        );

        let input = SessionMoveInput {
            id: "7b3a7c62-1f3f-45d6-b8a1-f2b83e3d9f71".to_string(),
            to_workspace_root: selector.to_string(),
        };
        assert!(server.session_move_apply(Parameters(input)).await.is_err());
        assert!(!missing_store.exists());
    }
}
