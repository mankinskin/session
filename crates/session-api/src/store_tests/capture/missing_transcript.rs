// Regression fixtures for session 4e24320a-ce4a-4b2e-8fe0-180975e06abd, where
// `session_peek_skeleton` failed because the capture hook had written
// `session.json` but not yet `transcript.json`.

fn persist_manifest_only_session(
    tempdir: &TempDir,
    session_id: &str,
) -> (SessionStoreConfig, std::path::PathBuf) {
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let plan = config
        .persist_capture(sample_request(
            session_id,
            Some("conversation-missing-transcript"),
            sample_time(),
            &["first prompt"],
        ))
        .unwrap();
    std::fs::remove_file(&plan.paths.transcript_path).unwrap();
    assert!(plan.paths.manifest_path.exists());
    (config, plan.paths.transcript_path)
}

#[test]
fn peek_skeleton_succeeds_for_manifest_only_session() {
    let tempdir = TempDir::new().unwrap();
    let (config, _) =
        persist_manifest_only_session(&tempdir, "session-manifest-only");

    let skeleton = config.peek_skeleton("session-manifest-only", 40).unwrap();

    assert_eq!(skeleton.session_id, "session-manifest-only");
    assert_eq!(skeleton.total_turns, 0);
    assert!(skeleton.entries.is_empty());
}

#[test]
fn read_session_returns_empty_turns_for_manifest_only_session() {
    let tempdir = TempDir::new().unwrap();
    let (config, _) =
        persist_manifest_only_session(&tempdir, "session-manifest-read");

    let record = config.read_session("session-manifest-read").unwrap();

    assert_eq!(record.session_id, "session-manifest-read");
    assert!(record.turns.is_empty());
}

#[test]
fn peek_range_succeeds_for_manifest_only_session() {
    let tempdir = TempDir::new().unwrap();
    let (config, _) =
        persist_manifest_only_session(&tempdir, "session-manifest-range");

    let range = config.peek_range("session-manifest-range", 0, Some(5)).unwrap();

    assert_eq!(range.total_turns, 0);
    assert!(range.turns.is_empty());
}

#[test]
fn peek_skeleton_still_reports_not_found_without_manifest() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));

    let error = config.peek_skeleton("session-absent", 40).unwrap_err();

    assert!(
        matches!(error, SessionError::NotFound { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn read_session_rejects_empty_transcript_file() {
    let tempdir = TempDir::new().unwrap();
    let (config, transcript_path) =
        persist_manifest_only_session(&tempdir, "session-empty-transcript");
    std::fs::write(&transcript_path, "").unwrap();

    let error = config.read_session("session-empty-transcript").unwrap_err();

    assert!(
        matches!(error, SessionError::Deserialize { ref path, .. } if *path == transcript_path),
        "unexpected error: {error:?}"
    );
}

#[test]
fn read_session_rejects_corrupt_transcript_file() {
    let tempdir = TempDir::new().unwrap();
    let (config, transcript_path) =
        persist_manifest_only_session(&tempdir, "session-corrupt-transcript");
    std::fs::write(&transcript_path, "{\"turns\": [").unwrap();

    let error = config.read_session("session-corrupt-transcript").unwrap_err();

    assert!(
        matches!(error, SessionError::Deserialize { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn read_session_rejects_transcript_with_unsupported_schema() {
    let tempdir = TempDir::new().unwrap();
    let (config, transcript_path) =
        persist_manifest_only_session(&tempdir, "session-bad-schema");
    let transcript = PersistedSessionTranscript {
        schema_version: SESSION_SCHEMA_VERSION + 1,
        session_id: "session-bad-schema".to_string(),
        captured_at: sample_time(),
        turns: vec![],
    };
    std::fs::write(&transcript_path, serde_json::to_string(&transcript).unwrap())
        .unwrap();

    let error = config.read_session("session-bad-schema").unwrap_err();

    assert!(
        matches!(error, SessionError::SchemaVersionMismatch { .. }),
        "unexpected error: {error:?}"
    );
}

#[test]
fn transcript_written_after_manifest_is_read_normally() {
    let tempdir = TempDir::new().unwrap();
    let (config, _) =
        persist_manifest_only_session(&tempdir, "session-late-transcript");
    assert_eq!(
        config.peek_skeleton("session-late-transcript", 40).unwrap().total_turns,
        0
    );

    config
        .persist_capture(sample_request(
            "session-late-transcript",
            Some("conversation-missing-transcript"),
            sample_time_later(),
            &["first prompt", "second prompt"],
        ))
        .unwrap();

    let skeleton = config.peek_skeleton("session-late-transcript", 40).unwrap();
    assert!(skeleton.total_turns >= 1, "skeleton: {skeleton:?}");
}

#[test]
fn query_sessions_includes_manifest_only_session() {
    let tempdir = TempDir::new().unwrap();
    let (config, _) =
        persist_manifest_only_session(&tempdir, "session-manifest-query");

    let result = config
        .query_sessions(&SessionQuery {
            session_id_prefix: Some("session-manifest-query".to_string()),
            conversation_id: None,
            agent_id: None,
            text: None,
            limit: None,
        })
        .unwrap();

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.sessions.len(), 1);
    assert!(result.sessions[0].turns.is_empty());
}
