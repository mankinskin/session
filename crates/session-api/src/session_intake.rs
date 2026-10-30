//! Immutable Session-owned submitted inputs and forward-only Mission linkage.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub const SESSION_INTAKE_SCHEMA_VERSION: u32 = 1;

/// Immutable description of the submitted input bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionIntakeRecord {
    pub schema_version: u32,
    pub session_id: String,
    pub request_id: String,
    pub input_digest: String,
    pub byte_len: u64,
    pub submitted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionIntakeLinkState {
    Reserved,
    Linked,
}

/// Session-side journal of the Session-to-Mission linkage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionIntakeLinkage {
    pub schema_version: u32,
    pub mission_id: Uuid,
    pub mission_workspace_root: PathBuf,
    pub state: SessionIntakeLinkState,
    pub reserved_at: DateTime<Utc>,
    pub linked_at: Option<DateTime<Utc>>,
}

/// Verified read-back of one persisted intake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIntake {
    pub record: SessionIntakeRecord,
    pub input_path: PathBuf,
    pub linkage: Option<SessionIntakeLinkage>,
    pub newly_submitted: bool,
}
