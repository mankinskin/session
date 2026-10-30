//! Upward escalation workflow for sub-agents.
//!
//! A cheaper sub-agent encountering a hard problem writes a durable escalation
//! record and returns an `ESCALATION:<escalation_id>` marker in its final
//! message. The orchestrator/larger model reads the record and resolves it
//! (handle directly, grant a budget offset, escalate to user, or spawn a fresh
//! session). The durable record enables async pickup later.

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{SessionError, SessionStoreConfig};

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
    let json = serde_json::to_string_pretty(&escalation).map_err(|e| SessionError::Serialize {
        path: path.clone(),
        source: e,
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
            if let Ok(escalation) = serde_json::from_str::<EscalationRecord>(&content) {
                if status_filter.is_none() || status_filter == Some(escalation.status) {
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
    let path = escalations_dir(&config.root).join(format!("{}.json", escalation_id));

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
    let path = escalations_dir(&config.root).join(format!("{}.json", escalation_id));

    if !path.exists() {
        return Err(SessionError::NotFound { path });
    }

    let content = fs::read_to_string(&path).map_err(|e| SessionError::Io {
        path: path.clone(),
        source: e,
    })?;

    let mut escalation: EscalationRecord =
        serde_json::from_str(&content).map_err(|e| SessionError::Deserialize {
            path: path.clone(),
            source: e,
        })?;

    escalation.status = EscalationStatus::Resolved;
    escalation.resolution = Some(resolution);

    let json = serde_json::to_string_pretty(&escalation).map_err(|e| SessionError::Serialize {
        path: path.clone(),
        source: e,
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
#[path = "escalation/tests.rs"]
mod tests;
