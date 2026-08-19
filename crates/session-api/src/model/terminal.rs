use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTerminalCreateRequest {
    pub session_id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionTerminalStatus {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTerminalManifest {
    pub terminal_id: String,
    pub session_id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    pub created_at: DateTime<Utc>,
    pub status: SessionTerminalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTerminalEvent {
    pub sequence: usize,
    pub captured_at: DateTime<Utc>,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTerminalRecord {
    pub manifest: SessionTerminalManifest,
    #[serde(default)]
    pub events: Vec<SessionTerminalEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTerminalPeekResult {
    pub manifest: SessionTerminalManifest,
    pub events: Vec<SessionTerminalEvent>,
    pub next_offset: usize,
    pub has_more: bool,
}
