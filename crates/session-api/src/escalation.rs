//! Upward escalation workflow for sub-agents.
//!
//! A cheaper sub-agent encountering a hard problem writes a durable escalation
//! record and returns an `ESCALATION:<escalation_id>` marker in its final
//! message. The orchestrator/larger model reads the record and resolves it
//! (handle directly, grant a budget offset, escalate to user, or spawn a fresh
//! session). The durable record enables async pickup later.

use std::{
    fs,
    path::{
        Path,
        PathBuf,
    },
};

use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};
use uuid::Uuid;

use crate::{
    SessionError,
    SessionStoreConfig,
};

/// Status of an escalation record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EscalationStatus {
    /// Escalation is open and awaiting resolution.
    Open,
    /// Escalation has been resolved.
    Resolved,
}

/// Action taken to resolve an escalation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EscalationAction {
    /// Orchestrator handled the problem directly.
    Handled,
    /// Orchestrator granted a budget offset to the sub-agent.
    GrantedOffset,
    /// Orchestrator escalated to the user.
    EscalatedToUser,
    /// Orchestrator spawned a fresh session for this problem.
    SpawnedSession,
}

/// Resolution details for an escalation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscalationResolution {
    /// Action taken to resolve the escalation.
    pub action: EscalationAction,
    /// Optional note about the resolution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Grant ID if action is GrantedOffset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_grant_id: Option<String>,
    /// Session ID if action is SpawnedSession.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawned_session_id: Option<String>,
    /// When the escalation was resolved.
    pub resolved_at: DateTime<Utc>,
}

/// A durable escalation record.
///
/// Stored as `<store_root>/escalations/<escalation_id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscalationRecord {
    /// Unique escalation identifier (UUID).
    pub escalation_id: String,
    /// When the escalation was created.
    pub created_at: DateTime<Utc>,
    /// Session that created the escalation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Model that created the escalation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_model: Option<String>,
    /// The blocking decision or problem statement.
    pub blocking_decision: String,
    /// Context explaining the situation.
    pub context: String,
    /// Requested capability or resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_capability: Option<String>,
    /// Options the sub-agent considered before escalating.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options_considered: Vec<String>,
    /// Current status of the escalation.
    pub status: EscalationStatus,
    /// Resolution details (present when status is Resolved).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<EscalationResolution>,
}

/// Resolve the escalations directory for a session store.
fn escalations_dir(store_root: &Path) -> PathBuf {
    store_root.join("escalations")
}

/// Create a new escalation record and write it to disk.
///
/// # Arguments
///
/// * `config` - Session store configuration
/// * `blocking_decision` - The blocking decision or problem statement
/// * `context` - Context explaining the situation
/// * `requested_capability` - Optional capability or resource requested
/// * `options_considered` - Options the sub-agent considered
/// * `session_id` - Optional session that created the escalation
/// * `from_model` - Optional model that created the escalation
///
/// Returns the created escalation with its generated ID and Open status.
pub fn create_escalation(
    config: &SessionStoreConfig,
    blocking_decision: String,
    context: String,
    requested_capability: Option<String>,
    options_considered: Vec<String>,
    session_id: Option<String>,
    from_model: Option<String>,
) -> Result<EscalationRecord, SessionError> {
    let escalation = EscalationRecord {
        escalation_id: Uuid::new_v4().to_string(),
        created_at: Utc::now(),
        session_id,
        from_model,
        blocking_decision,
        context,
        requested_capability,
        options_considered,
        status: EscalationStatus::Open,
        resolution: None,
    };

    let dir = escalations_dir(&config.root);
    fs::create_dir_all(&dir).map_err(|e| SessionError::Io {
        path: dir.clone(),
        source: e,
    })?;

    let path = dir.join(format!("{}.json", escalation.escalation_id));
    let json = serde_json::to_string_pretty(&escalation).map_err(|e| {
        SessionError::Serialize {
            path: path.clone(),
            source: e,
        }
    })?;

    fs::write(&path, json).map_err(|e| SessionError::Io {
        path: path.clone(),
        source: e,
    })?;

    Ok(escalation)
}

/// List all escalations in the store.
///
/// Returns all valid escalation files matching the optional status filter.
/// Malformed files are silently skipped.
///
/// # Arguments
///
/// * `config` - Session store configuration
/// * `status_filter` - Optional status to filter by
pub fn list_escalations(
    config: &SessionStoreConfig,
    status_filter: Option<EscalationStatus>,
) -> Result<Vec<EscalationRecord>, SessionError> {
    let dir = escalations_dir(&config.root);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(&dir).map_err(|e| SessionError::Io {
        path: dir.clone(),
        source: e,
    })?;

    let mut escalations = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(escalation) =
                serde_json::from_str::<EscalationRecord>(&content)
            {
                if status_filter.is_none()
                    || status_filter == Some(escalation.status)
                {
                    escalations.push(escalation);
                }
            }
        }
    }

    Ok(escalations)
}

/// Get a single escalation by ID.
///
/// Returns `None` if the escalation doesn't exist or can't be parsed.
pub fn get_escalation(
    config: &SessionStoreConfig,
    escalation_id: &str,
) -> Option<EscalationRecord> {
    let path =
        escalations_dir(&config.root).join(format!("{}.json", escalation_id));

    if !path.exists() {
        return None;
    }

    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<EscalationRecord>(&content).ok()
}

/// Resolve an escalation by updating its status and adding resolution details.
///
/// Returns the updated escalation record.
///
/// # Errors
///
/// Returns error if the escalation doesn't exist or can't be updated.
pub fn resolve_escalation(
    config: &SessionStoreConfig,
    escalation_id: &str,
    resolution: EscalationResolution,
) -> Result<EscalationRecord, SessionError> {
    let path =
        escalations_dir(&config.root).join(format!("{}.json", escalation_id));

    if !path.exists() {
        return Err(SessionError::NotFound { path });
    }

    let content = fs::read_to_string(&path).map_err(|e| SessionError::Io {
        path: path.clone(),
        source: e,
    })?;

    let mut escalation: EscalationRecord = serde_json::from_str(&content)
        .map_err(|e| SessionError::Deserialize {
            path: path.clone(),
            source: e,
        })?;

    escalation.status = EscalationStatus::Resolved;
    escalation.resolution = Some(resolution);

    let json = serde_json::to_string_pretty(&escalation).map_err(|e| {
        SessionError::Serialize {
            path: path.clone(),
            source: e,
        }
    })?;

    fs::write(&path, json).map_err(|e| SessionError::Io {
        path: path.clone(),
        source: e,
    })?;

    Ok(escalation)
}

/// Generate the standard escalation marker string.
///
/// The marker format is `ESCALATION:<escalation_id>`.
pub fn escalation_marker(id: &str) -> String {
    format!("ESCALATION:{}", id)
}

/// Parse an escalation ID from text containing an escalation marker.
///
/// Extracts the ID from any text containing `ESCALATION:<id>`.
/// Returns `None` if no marker is found.
pub fn parse_escalation_marker(text: &str) -> Option<String> {
    const PREFIX: &str = "ESCALATION:";

    let start_pos = text.find(PREFIX)?;
    let id_start = start_pos + PREFIX.len();
    let remaining = &text[id_start..];

    // Extract the UUID (alphanumeric and hyphens until a non-UUID character)
    let id_end = remaining
        .find(|c: char| !c.is_alphanumeric() && c != '-')
        .unwrap_or(remaining.len());

    let id = &remaining[..id_end];
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn test_config() -> (TempDir, SessionStoreConfig) {
        let tmp = TempDir::new().unwrap();
        let config = SessionStoreConfig {
            root: tmp.path().to_path_buf(),
            workspace_slug: "test".to_string(),
        };
        (tmp, config)
    }

    #[test]
    fn test_create_escalation_writes_correct_json() {
        let (_tmp, config) = test_config();

        let escalation = create_escalation(
            &config,
            "Cannot decide between A and B".to_string(),
            "Both options have trade-offs".to_string(),
            Some("architecture decision".to_string()),
            vec!["Option A".to_string(), "Option B".to_string()],
            Some("session-123".to_string()),
            Some("claude-haiku-4.5".to_string()),
        )
        .unwrap();

        // Read the written file
        let path = escalations_dir(&config.root)
            .join(format!("{}.json", escalation.escalation_id));
        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Verify all expected fields are present
        assert!(parsed.get("escalation_id").is_some());
        assert_eq!(
            parsed["escalation_id"].as_str().unwrap(),
            escalation.escalation_id
        );
        assert_eq!(parsed["status"].as_str().unwrap(), "open");
        assert_eq!(
            parsed["blocking_decision"].as_str().unwrap(),
            "Cannot decide between A and B"
        );
        assert_eq!(
            parsed["context"].as_str().unwrap(),
            "Both options have trade-offs"
        );
        assert_eq!(
            parsed["requested_capability"].as_str().unwrap(),
            "architecture decision"
        );
        assert_eq!(parsed["session_id"].as_str().unwrap(), "session-123");
        assert_eq!(parsed["from_model"].as_str().unwrap(), "claude-haiku-4.5");
        assert!(parsed["options_considered"].is_array());
        assert!(parsed.get("resolution").is_none());
    }

    #[test]
    fn test_list_escalations_filters_by_status() {
        let (_tmp, config) = test_config();

        // Create an open escalation
        let open_esc = create_escalation(
            &config,
            "Problem 1".to_string(),
            "Context 1".to_string(),
            None,
            vec![],
            None,
            None,
        )
        .unwrap();

        // Create another open escalation and resolve it
        let to_resolve = create_escalation(
            &config,
            "Problem 2".to_string(),
            "Context 2".to_string(),
            None,
            vec![],
            None,
            None,
        )
        .unwrap();

        resolve_escalation(
            &config,
            &to_resolve.escalation_id,
            EscalationResolution {
                action: EscalationAction::Handled,
                note: Some("Fixed it".to_string()),
                offset_grant_id: None,
                spawned_session_id: None,
                resolved_at: Utc::now(),
            },
        )
        .unwrap();

        // List all escalations
        let all = list_escalations(&config, None).unwrap();
        assert_eq!(all.len(), 2);

        // List only open escalations
        let open =
            list_escalations(&config, Some(EscalationStatus::Open)).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].escalation_id, open_esc.escalation_id);

        // List only resolved escalations
        let resolved =
            list_escalations(&config, Some(EscalationStatus::Resolved))
                .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].escalation_id, to_resolve.escalation_id);
    }

    #[test]
    fn test_async_pickup() {
        let (_tmp, config) = test_config();

        // Simulate: sub-agent creates escalation
        let created = create_escalation(
            &config,
            "Hard problem".to_string(),
            "Need help".to_string(),
            None,
            vec![],
            Some("session-xyz".to_string()),
            Some("claude-haiku-4.5".to_string()),
        )
        .unwrap();

        // Simulate: different reader (orchestrator) picks it up later
        let found =
            list_escalations(&config, Some(EscalationStatus::Open)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].escalation_id, created.escalation_id);
        assert_eq!(found[0].session_id, Some("session-xyz".to_string()));
    }

    #[test]
    fn test_get_escalation() {
        let (_tmp, config) = test_config();

        let created = create_escalation(
            &config,
            "Test".to_string(),
            "Context".to_string(),
            None,
            vec![],
            None,
            None,
        )
        .unwrap();

        let retrieved =
            get_escalation(&config, &created.escalation_id).unwrap();
        assert_eq!(retrieved.escalation_id, created.escalation_id);
        assert_eq!(retrieved.status, EscalationStatus::Open);

        // Non-existent ID returns None
        assert!(get_escalation(&config, "nonexistent").is_none());
    }

    #[test]
    fn test_resolve_escalation_all_actions() {
        let (_tmp, config) = test_config();

        // Test Handled action
        let esc1 = create_escalation(
            &config,
            "Problem 1".to_string(),
            "Context 1".to_string(),
            None,
            vec![],
            None,
            None,
        )
        .unwrap();

        let resolved1 = resolve_escalation(
            &config,
            &esc1.escalation_id,
            EscalationResolution {
                action: EscalationAction::Handled,
                note: Some("Handled directly".to_string()),
                offset_grant_id: None,
                spawned_session_id: None,
                resolved_at: Utc::now(),
            },
        )
        .unwrap();

        assert_eq!(resolved1.status, EscalationStatus::Resolved);
        assert!(resolved1.resolution.is_some());
        assert_eq!(
            resolved1.resolution.as_ref().unwrap().action,
            EscalationAction::Handled
        );

        // Test GrantedOffset action
        let esc2 = create_escalation(
            &config,
            "Problem 2".to_string(),
            "Context 2".to_string(),
            None,
            vec![],
            None,
            None,
        )
        .unwrap();

        let resolved2 = resolve_escalation(
            &config,
            &esc2.escalation_id,
            EscalationResolution {
                action: EscalationAction::GrantedOffset,
                note: None,
                offset_grant_id: Some("grant-123".to_string()),
                spawned_session_id: None,
                resolved_at: Utc::now(),
            },
        )
        .unwrap();

        assert_eq!(
            resolved2.resolution.as_ref().unwrap().action,
            EscalationAction::GrantedOffset
        );
        assert_eq!(
            resolved2.resolution.as_ref().unwrap().offset_grant_id,
            Some("grant-123".to_string())
        );

        // Test SpawnedSession action
        let esc3 = create_escalation(
            &config,
            "Problem 3".to_string(),
            "Context 3".to_string(),
            None,
            vec![],
            None,
            None,
        )
        .unwrap();

        let resolved3 = resolve_escalation(
            &config,
            &esc3.escalation_id,
            EscalationResolution {
                action: EscalationAction::SpawnedSession,
                note: None,
                offset_grant_id: None,
                spawned_session_id: Some("session-new".to_string()),
                resolved_at: Utc::now(),
            },
        )
        .unwrap();

        assert_eq!(
            resolved3.resolution.as_ref().unwrap().action,
            EscalationAction::SpawnedSession
        );
        assert_eq!(
            resolved3.resolution.as_ref().unwrap().spawned_session_id,
            Some("session-new".to_string())
        );

        // Test EscalatedToUser action
        let esc4 = create_escalation(
            &config,
            "Problem 4".to_string(),
            "Context 4".to_string(),
            None,
            vec![],
            None,
            None,
        )
        .unwrap();

        let resolved4 = resolve_escalation(
            &config,
            &esc4.escalation_id,
            EscalationResolution {
                action: EscalationAction::EscalatedToUser,
                note: Some("User input needed".to_string()),
                offset_grant_id: None,
                spawned_session_id: None,
                resolved_at: Utc::now(),
            },
        )
        .unwrap();

        assert_eq!(
            resolved4.resolution.as_ref().unwrap().action,
            EscalationAction::EscalatedToUser
        );
    }

    #[test]
    fn test_resolve_missing_escalation_errors() {
        let (_tmp, config) = test_config();

        let result = resolve_escalation(
            &config,
            "nonexistent",
            EscalationResolution {
                action: EscalationAction::Handled,
                note: None,
                offset_grant_id: None,
                spawned_session_id: None,
                resolved_at: Utc::now(),
            },
        );

        assert!(result.is_err());
        match result {
            Err(SessionError::NotFound { path }) => {
                assert!(path.to_string_lossy().contains("nonexistent"));
            },
            _ => panic!("Expected NotFound error"),
        }
    }

    #[test]
    fn test_escalation_marker_round_trip() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let marker = escalation_marker(id);
        assert_eq!(marker, "ESCALATION:550e8400-e29b-41d4-a716-446655440000");

        let parsed = parse_escalation_marker(&marker).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_parse_escalation_marker_from_message() {
        let message = "I've analyzed the problem and found it requires architectural decisions beyond my scope. ESCALATION:abc123-def456-ghi789 Please review and provide guidance.";

        let parsed = parse_escalation_marker(message).unwrap();
        assert_eq!(parsed, "abc123-def456-ghi789");
    }

    #[test]
    fn test_parse_escalation_marker_returns_none_when_absent() {
        let message = "This is just a regular message with no escalation.";
        assert!(parse_escalation_marker(message).is_none());
    }

    #[test]
    fn test_serde_status_lowercase() {
        let open = EscalationStatus::Open;
        let json = serde_json::to_string(&open).unwrap();
        assert_eq!(json, "\"open\"");

        let resolved = EscalationStatus::Resolved;
        let json = serde_json::to_string(&resolved).unwrap();
        assert_eq!(json, "\"resolved\"");
    }

    #[test]
    fn test_serde_action_kebab_case() {
        let handled = EscalationAction::Handled;
        let json = serde_json::to_string(&handled).unwrap();
        assert_eq!(json, "\"handled\"");

        let granted = EscalationAction::GrantedOffset;
        let json = serde_json::to_string(&granted).unwrap();
        assert_eq!(json, "\"granted-offset\"");

        let escalated = EscalationAction::EscalatedToUser;
        let json = serde_json::to_string(&escalated).unwrap();
        assert_eq!(json, "\"escalated-to-user\"");

        let spawned = EscalationAction::SpawnedSession;
        let json = serde_json::to_string(&spawned).unwrap();
        assert_eq!(json, "\"spawned-session\"");
    }
}
