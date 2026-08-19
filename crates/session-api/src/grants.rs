//! Budget-offset grants for the graded cost gate.
//!
//! A grant boosts a session or subagent's effective budget by the specified
//! offset. Grants are stored as JSON files in `<store_root>/grants/`.
//!
//! The gate (mcp-toolmon) reads these files to resolve offsets. This module
//! provides the writer side: create, list, and revoke operations.

use std::{
    fs,
    path::{
        Path,
        PathBuf,
    },
};

use chrono::{
    DateTime,
    Duration,
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

/// Scope of a budget grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetGrantScope {
    /// Grant applies to a specific session.
    Session,
    /// Grant applies to subagent spawns.
    Subagent,
}

/// A budget-offset grant record.
///
/// Stored as `<grants_dir>/<grant_id>.json` with fields matching the gate's
/// reader contract (offset/model/expires_at required; grant_id/scope stored
/// but not used by the gate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetGrant {
    /// Unique grant identifier (UUID).
    pub grant_id: String,
    /// Scope of the grant (session or subagent).
    pub scope: BudgetGrantScope,
    /// Budget offset (added to base budget).
    pub offset: u32,
    /// Optional model constraint (case-insensitive match).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional expiration timestamp (RFC3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

impl BudgetGrant {
    /// Create a new grant with a generated UUID.
    pub fn new(
        scope: BudgetGrantScope,
        offset: u32,
        model: Option<String>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            grant_id: Uuid::new_v4().to_string(),
            scope,
            offset,
            model,
            expires_at: expires_at.map(|dt| dt.to_rfc3339()),
        }
    }

    /// Check if this grant is expired.
    pub fn is_expired(&self) -> bool {
        if let Some(expires) = &self.expires_at {
            if let Ok(exp_time) = DateTime::parse_from_rfc3339(expires) {
                return exp_time < Utc::now();
            }
        }
        false
    }
}

/// Resolve the grants directory for a session store.
fn grants_dir(store_root: &Path) -> PathBuf {
    store_root.join("grants")
}

/// Create a new budget grant and write it to disk.
///
/// # Arguments
///
/// * `config` - Session store configuration
/// * `scope` - Grant scope (session or subagent)
/// * `offset` - Budget offset to add
/// * `model` - Optional model constraint
/// * `ttl_seconds` - Optional TTL in seconds (from now)
///
/// Returns the created grant with its generated ID.
pub fn create_grant(
    config: &SessionStoreConfig,
    scope: BudgetGrantScope,
    offset: u32,
    model: Option<String>,
    ttl_seconds: Option<u64>,
) -> Result<BudgetGrant, SessionError> {
    let expires_at =
        ttl_seconds.map(|secs| Utc::now() + Duration::seconds(secs as i64));
    let grant = BudgetGrant::new(scope, offset, model, expires_at);

    let dir = grants_dir(&config.root);
    fs::create_dir_all(&dir).map_err(|e| SessionError::Io {
        path: dir.clone(),
        source: e,
    })?;

    let path = dir.join(format!("{}.json", grant.grant_id));
    let json = serde_json::to_string_pretty(&grant).map_err(|e| {
        SessionError::Serialize {
            path: path.clone(),
            source: e,
        }
    })?;

    fs::write(&path, json).map_err(|e| SessionError::Io {
        path: path.clone(),
        source: e,
    })?;

    Ok(grant)
}

/// List all grants in the store.
///
/// Returns all valid grant files, skipping any that fail to parse.
/// Expired grants are included in the list (callers can filter with `is_expired`).
pub fn list_grants(
    config: &SessionStoreConfig
) -> Result<Vec<BudgetGrant>, SessionError> {
    let dir = grants_dir(&config.root);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(&dir).map_err(|e| SessionError::Io {
        path: dir.clone(),
        source: e,
    })?;

    let mut grants = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(grant) = serde_json::from_str::<BudgetGrant>(&content) {
                grants.push(grant);
            }
        }
    }

    Ok(grants)
}

/// Revoke a grant by deleting its file.
///
/// Returns `true` if the grant was deleted, `false` if it didn't exist.
pub fn revoke_grant(
    config: &SessionStoreConfig,
    grant_id: &str,
) -> Result<bool, SessionError> {
    let path = grants_dir(&config.root).join(format!("{}.json", grant_id));

    if !path.exists() {
        return Ok(false);
    }

    fs::remove_file(&path).map_err(|e| SessionError::Io {
        path: path.clone(),
        source: e,
    })?;

    Ok(true)
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
    fn test_create_grant_writes_correct_json() {
        let (_tmp, config) = test_config();

        let grant = create_grant(
            &config,
            BudgetGrantScope::Session,
            30,
            Some("claude-sonnet-4.5".to_string()),
            None,
        )
        .unwrap();

        // Read the written file
        let path =
            grants_dir(&config.root).join(format!("{}.json", grant.grant_id));
        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Verify all expected fields are present
        assert!(parsed.get("grant_id").is_some());
        assert_eq!(parsed["grant_id"].as_str().unwrap(), grant.grant_id);
        assert_eq!(parsed["scope"].as_str().unwrap(), "session");
        assert_eq!(parsed["offset"].as_u64().unwrap(), 30);
        assert_eq!(parsed["model"].as_str().unwrap(), "claude-sonnet-4.5");
        assert!(parsed.get("expires_at").is_none());
    }

    #[test]
    fn test_create_grant_with_ttl() {
        let (_tmp, config) = test_config();

        let grant = create_grant(
            &config,
            BudgetGrantScope::Subagent,
            50,
            None,
            Some(3600), // 1 hour
        )
        .unwrap();

        assert_eq!(grant.scope, BudgetGrantScope::Subagent);
        assert_eq!(grant.offset, 50);
        assert!(grant.model.is_none());
        assert!(grant.expires_at.is_some());

        // Verify expires_at is in the future
        let expires =
            DateTime::parse_from_rfc3339(grant.expires_at.as_ref().unwrap())
                .unwrap();
        assert!(expires > Utc::now());
    }

    #[test]
    fn test_grant_round_trip_with_gate_format() {
        let (_tmp, config) = test_config();

        let grant = create_grant(
            &config,
            BudgetGrantScope::Session,
            25,
            Some("claude-opus-4.5".to_string()),
            Some(7200),
        )
        .unwrap();

        // Read and parse using the gate's Grant struct fields (simulate gate reader)
        let path =
            grants_dir(&config.root).join(format!("{}.json", grant.grant_id));
        let content = fs::read_to_string(&path).unwrap();

        // Parse with a minimal struct matching the gate's fields
        #[derive(Deserialize)]
        struct GateGrant {
            #[serde(default)]
            offset: u32,
            #[serde(default)]
            model: Option<String>,
            #[serde(default)]
            expires_at: Option<String>,
        }

        let gate_grant: GateGrant = serde_json::from_str(&content).unwrap();
        assert_eq!(gate_grant.offset, 25);
        assert_eq!(gate_grant.model.as_deref(), Some("claude-opus-4.5"));
        assert!(gate_grant.expires_at.is_some());
    }

    #[test]
    fn test_list_grants() {
        let (_tmp, config) = test_config();

        // Create multiple grants
        let grant1 =
            create_grant(&config, BudgetGrantScope::Session, 10, None, None)
                .unwrap();
        let grant2 =
            create_grant(&config, BudgetGrantScope::Subagent, 20, None, None)
                .unwrap();

        let grants = list_grants(&config).unwrap();
        assert_eq!(grants.len(), 2);

        let ids: Vec<_> = grants.iter().map(|g| g.grant_id.as_str()).collect();
        assert!(ids.contains(&grant1.grant_id.as_str()));
        assert!(ids.contains(&grant2.grant_id.as_str()));
    }

    #[test]
    fn test_list_grants_skips_malformed() {
        let (_tmp, config) = test_config();

        let dir = grants_dir(&config.root);
        fs::create_dir_all(&dir).unwrap();

        // Write a valid grant
        create_grant(&config, BudgetGrantScope::Session, 10, None, None)
            .unwrap();

        // Write a malformed file
        fs::write(dir.join("bad.json"), "not valid json").unwrap();

        // list_grants should succeed and return only the valid grant
        let grants = list_grants(&config).unwrap();
        assert_eq!(grants.len(), 1);
    }

    #[test]
    fn test_revoke_grant() {
        let (_tmp, config) = test_config();

        let grant =
            create_grant(&config, BudgetGrantScope::Session, 15, None, None)
                .unwrap();

        // Verify file exists
        let path =
            grants_dir(&config.root).join(format!("{}.json", grant.grant_id));
        assert!(path.exists());

        // Revoke
        let revoked = revoke_grant(&config, &grant.grant_id).unwrap();
        assert!(revoked);
        assert!(!path.exists());

        // Revoke again returns false
        let revoked = revoke_grant(&config, &grant.grant_id).unwrap();
        assert!(!revoked);
    }

    #[test]
    fn test_revoke_nonexistent_returns_false() {
        let (_tmp, config) = test_config();

        let revoked = revoke_grant(&config, "nonexistent-id").unwrap();
        assert!(!revoked);
    }

    #[test]
    fn test_expired_grant_detection() {
        let (_tmp, config) = test_config();

        // Create an expired grant (TTL of 0 seconds puts it in the past)
        let grant =
            create_grant(&config, BudgetGrantScope::Session, 10, None, Some(0))
                .unwrap();

        // Wait a tiny bit to ensure it's in the past
        std::thread::sleep(std::time::Duration::from_millis(10));

        assert!(grant.is_expired());
    }

    #[test]
    fn test_scope_serialization() {
        assert_eq!(
            serde_json::to_string(&BudgetGrantScope::Session).unwrap(),
            "\"session\""
        );
        assert_eq!(
            serde_json::to_string(&BudgetGrantScope::Subagent).unwrap(),
            "\"subagent\""
        );
    }

    #[test]
    fn test_list_includes_expired_grants() {
        let (_tmp, config) = test_config();

        // Create an expired grant
        create_grant(&config, BudgetGrantScope::Session, 10, None, Some(0))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));

        // list_grants should still return it (gate filters expired at read time)
        let grants = list_grants(&config).unwrap();
        assert_eq!(grants.len(), 1);
        assert!(grants[0].is_expired());
    }
}
