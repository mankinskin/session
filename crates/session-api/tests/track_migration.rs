use session_api::{
    PersistedSessionManifest,
    SessionLinks,
    SessionMetadata,
    SessionRecord,
};
use std::path::PathBuf;

/// AC1: Existing sessions load without error
/// AC2: track_id reports null for pre-existing sessions
///
/// This test loads REAL sessions from the repo-root .session/sessions/ folder
/// to verify that pre-existing sessions (without track fields) deserialize successfully
/// and report track_id = None.
#[test]
fn test_existing_sessions_load_without_error_real_data()
-> Result<(), Box<dyn std::error::Error>> {
    // Use the real repo-root session store
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("Failed to find repo root");

    let sessions_dir = repo_root.join(".session/sessions");

    // Skip if the store doesn't exist (CI or fresh clone)
    if !sessions_dir.exists() {
        eprintln!(
            "Skipping real-data test: .session/sessions/ not found at repo root"
        );
        return Ok(());
    }

    let mut sessions_loaded = 0;

    // Read session.json files from actual sessions on disk
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let manifest_path = entry.path().join("session.json");

        if !manifest_path.exists() {
            continue;
        }

        // AC1: Session deserializes without error
        let manifest_bytes = std::fs::read(&manifest_path)?;
        let manifest: PersistedSessionManifest =
            serde_json::from_slice(&manifest_bytes)?;

        // AC2: track_id should be None for pre-existing sessions
        assert_eq!(
            manifest.track_id, None,
            "Expected track_id=None for pre-existing session {}, got {:?}",
            manifest.session_id, manifest.track_id
        );

        // All track fields should be None
        assert_eq!(manifest.anchor_ticket_id, None);
        assert_eq!(manifest.parent_session_id, None);
        assert_eq!(manifest.spawned_session_id, None);

        sessions_loaded += 1;

        if sessions_loaded >= 10 {
            break; // Sample only 10 sessions for speed
        }
    }

    if sessions_loaded == 0 {
        eprintln!(
            "Warning: No sessions found in real store, cannot fully verify AC1/AC2"
        );
        return Ok(());
    }

    println!(
        "✓ AC1: {} existing sessions loaded without error",
        sessions_loaded
    );
    println!("✓ AC2: All track_id fields are None for pre-existing sessions");

    Ok(())
}

/// Test PersistedSessionManifest serialization/deserialization with track fields
#[test]
fn test_manifest_roundtrip_with_track_fields()
-> Result<(), Box<dyn std::error::Error>> {
    use chrono::Utc;

    let now = Utc::now();

    // Create manifest with track fields
    let manifest = PersistedSessionManifest {
        schema_version: 1,
        session_id: "test-session".to_string(),
        source: "test".to_string(),
        started_at: now,
        captured_at: now,
        metadata: SessionMetadata {
            workspace_slug: "test".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: None,
            trigger: None,
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree: None,
        },
        links: SessionLinks::default(),
        track_id: Some("track-xyz".to_string()),
        anchor_ticket_id: Some("ticket-abc".to_string()),
        parent_session_id: Some("parent-def".to_string()),
        spawned_session_id: Some("spawned-ghi".to_string()),
        emitted_handoff_ids: vec![],
        picked_up_handoff_ids: vec![],
        active_run_id: String::new(),
        runs: vec![],
        pinned_entities: vec![],
        workflow: Default::default(),
    };

    // Serialize
    let json = serde_json::to_string_pretty(&manifest)?;

    // Verify track fields are in JSON
    assert!(json.contains("track_id"));
    assert!(json.contains("track-xyz"));

    // Deserialize
    let deserialized: PersistedSessionManifest = serde_json::from_str(&json)?;

    // Verify all track fields preserved
    assert_eq!(deserialized.track_id, Some("track-xyz".to_string()));
    assert_eq!(
        deserialized.anchor_ticket_id,
        Some("ticket-abc".to_string())
    );
    assert_eq!(
        deserialized.parent_session_id,
        Some("parent-def".to_string())
    );
    assert_eq!(
        deserialized.spawned_session_id,
        Some("spawned-ghi".to_string())
    );

    println!("✓ Manifest round-trip preserves all track fields");

    Ok(())
}

/// Test that manifest without track fields deserializes with None values
#[test]
fn test_legacy_manifest_deserialization()
-> Result<(), Box<dyn std::error::Error>> {
    // JSON without track fields (old format)
    let legacy_json = r#"{
        "schema_version": 1,
        "session_id": "legacy-session",
        "source": "copilot-hook",
        "started_at": "2026-07-26T23:40:14.755Z",
        "captured_at": "2026-07-26T23:51:30.249Z",
        "metadata": {
            "workspace_slug": "default",
            "agent_id": "copilot-agent",
            "trigger": "PostToolUse",
            "producer": "copilot-agent",
            "copilot_version": "0.58.0",
            "vscode_version": "1.130.0",
            "protocol_version": 1
        },
        "links": {}
    }"#;

    // Should deserialize successfully
    let manifest: PersistedSessionManifest = serde_json::from_str(legacy_json)?;

    // All track fields should default to None
    assert_eq!(manifest.track_id, None);
    assert_eq!(manifest.anchor_ticket_id, None);
    assert_eq!(manifest.parent_session_id, None);
    assert_eq!(manifest.spawned_session_id, None);

    println!(
        "✓ Legacy manifest (without track fields) deserializes with None values"
    );

    Ok(())
}

/// Test SessionRecord from/to PersistedSessionManifest preserves track fields
#[test]
fn test_session_record_track_field_conversion() {
    use chrono::Utc;

    let now = Utc::now();

    let record = SessionRecord {
        schema_version: 1,
        session_id: "test-session".to_string(),
        source: "test".to_string(),
        started_at: now,
        captured_at: now,
        metadata: SessionMetadata {
            workspace_slug: "test".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: None,
            trigger: None,
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree: None,
        },
        turns: vec![],
        links: SessionLinks::default(),
        track_id: Some("track-123".to_string()),
        anchor_ticket_id: Some("ticket-456".to_string()),
        parent_session_id: Some("parent-789".to_string()),
        spawned_session_id: Some("spawned-abc".to_string()),
        emitted_handoff_ids: vec![],
        picked_up_handoff_ids: vec![],
    };

    // Convert to PersistedSessionManifest
    let manifest: PersistedSessionManifest = (&record).into();

    // Verify track fields are copied
    assert_eq!(manifest.track_id, Some("track-123".to_string()));
    assert_eq!(manifest.anchor_ticket_id, Some("ticket-456".to_string()));
    assert_eq!(manifest.parent_session_id, Some("parent-789".to_string()));
    assert_eq!(manifest.spawned_session_id, Some("spawned-abc".to_string()));

    println!(
        "✓ SessionRecord -> PersistedSessionManifest preserves track fields"
    );
}
