#[allow(unused_imports)]
use super::*;

#[test]
pub(in super::super) fn read_session_rejects_unknown_schema_version() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));

    let plan = config
        .persist_capture(sample_request(
            "session-schema",
            Some("conversation-schema"),
            sample_time(),
            &["check schema"],
        ))
        .unwrap();

    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&plan.paths.manifest_path).unwrap()).unwrap();
    manifest["schema_version"] = serde_json::json!(SESSION_SCHEMA_VERSION + 1);
    std::fs::write(
        &plan.paths.manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let err = config.read_session("session-schema").unwrap_err();
    assert!(matches!(err, SessionError::SchemaVersionMismatch { .. }));
}

#[test]
pub(in super::super) fn runtime_init_uses_the_provisioned_worktree_uuid() {
    let current_dir = std::env::current_dir().unwrap();
    let tempdir = TempDir::new_in(&current_dir).unwrap();
    let session_id = Uuid::new_v4().to_string();
    let store_root = tempdir
        .path()
        .join(".worktrees")
        .join(&session_id)
        .join("workspace-policy-refactor")
        .join(".session");
    std::fs::create_dir_all(&store_root).unwrap();
    let config = SessionStoreConfig::new(store_root.strip_prefix(&current_dir).unwrap());

    let result = config
        .init_runtime_context(SessionRuntimeInitRequest::default())
        .unwrap();

    assert_eq!(result.context.session_id, session_id);
    assert!(
        store_root
            .join("sessions")
            .join(&result.context.session_id)
            .join("session.json")
            .is_file()
    );
}

#[test]
pub(in super::super) fn worktree_identity_rejects_slug_values() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let session_id = "epic-kickoff-8fdfe135";

    let error = config
        .check_in_worktree(sample_worktree_request(
            session_id,
            "github-copilot",
            "ticket-a",
            tempdir.path().join("worktree"),
            "agent/example",
        ))
        .unwrap_err();

    assert!(matches!(
        error,
        SessionError::InvalidSessionId(ref value) if value == session_id
    ));
    assert!(error.to_string().contains("must be a UUID"));
}

#[test]
pub(in super::super) fn legacy_slug_keyed_session_record_remains_readable() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let session_id = "structured-ticket-entities-iteration";

    config
        .persist_capture(sample_request(
            session_id,
            Some("legacy-conversation"),
            sample_time(),
            &["legacy session"],
        ))
        .unwrap();

    assert_eq!(
        config.read_session(session_id).unwrap().session_id,
        session_id
    );
}

#[test]
pub(in super::super) fn context_schema_init_is_idempotent_without_forcing_a_new_run() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));

    let first = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let second = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(first.context.session_id.clone()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap();

    assert!(first.created_workspace);
    assert!(first.created_run);
    assert!(!second.created_workspace);
    assert!(!second.created_run);
    assert_eq!(first.context.session_id, second.context.session_id);
    assert_eq!(first.context.active_run_id, second.context.active_run_id);
    assert_eq!(second.context.runs.len(), 1);
}

#[test]
pub(in super::super) fn run_lineage_init_resume_creates_distinct_linked_run() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));

    let first = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let resumed = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(first.context.session_id.clone()),
            predecessor_run_id: Some(first.context.active_run_id.clone()),
            force_new_run: true,
        })
        .unwrap();

    assert_eq!(first.context.session_id, resumed.context.session_id);
    assert_ne!(first.context.active_run_id, resumed.context.active_run_id);
    assert_eq!(resumed.context.runs.len(), 2);
    assert_eq!(
        resumed.run.predecessor_run_id.as_deref(),
        Some(first.context.active_run_id.as_str())
    );
}

#[test]
pub(in super::super) fn context_pin_unpin_is_idempotent_and_persistent() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;
    let urn = "ce://default/tickets/effba966-f0a8-4d7d-b289-b7feba826cf8";

    let pinned_once = config
        .pin_runtime_entity(
            &workspace_id,
            urn,
            Some("primary-focus".to_string()),
            Some("epic context".to_string()),
        )
        .unwrap();
    let pinned_twice = config
        .pin_runtime_entity(&workspace_id, urn, None, None)
        .unwrap();

    assert_eq!(pinned_once.pinned_entities.len(), 1);
    assert_eq!(pinned_twice.pinned_entities.len(), 1);

    let loaded = config.read_runtime_context(&workspace_id).unwrap();
    assert_eq!(loaded.pinned_entities.len(), 1);

    let unpinned_once = config.unpin_runtime_entity(&workspace_id, urn).unwrap();
    let unpinned_twice = config.unpin_runtime_entity(&workspace_id, urn).unwrap();
    assert!(unpinned_once.pinned_entities.is_empty());
    assert!(unpinned_twice.pinned_entities.is_empty());
}

#[test]
pub(in super::super) fn context_pin_rejects_malformed_entity_urn_segments() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();

    let error = config
        .pin_runtime_entity(&init.context.session_id, "ce:///tickets/", None, None)
        .unwrap_err();

    assert!(matches!(error, SessionError::InvalidEntityUrn(_)));
}

#[test]
pub(in super::super) fn context_view_returns_headers_only() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    config
        .pin_runtime_entity(
            &workspace_id,
            "ce://default/specs/709f067a-21b6-41b6-8879-3cacef4bacaf",
            Some("guard".to_string()),
            Some("runtime contract".to_string()),
        )
        .unwrap();

    let view = config.view_runtime_context(&workspace_id).unwrap();
    let json = serde_json::to_string(&view).unwrap();

    assert_eq!(view.pinned_count, 1);
    assert!(json.contains("pinned_headers"));
    assert!(!json.contains("body"));
    assert!(!json.contains("content"));
}
