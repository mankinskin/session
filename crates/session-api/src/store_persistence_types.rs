use super::*;
use crate::SessionWorkflowGraph;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSessionManifest {
    #[serde(default = "crate::default_session_schema_version")]
    pub schema_version: u32,
    pub session_id: String,
    pub source: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub captured_at: chrono::DateTime<chrono::Utc>,
    pub metadata: SessionMetadata,
    #[serde(default)]
    pub links: SessionLinks,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_ticket_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emitted_handoff_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub picked_up_handoff_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub active_run_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<SessionRunLineage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_entities: Vec<SessionPinnedEntity>,
    #[serde(default, skip_serializing_if = "SessionWorkflowGraph::is_empty")]
    pub workflow: SessionWorkflowGraph,
}

impl From<&SessionRecord> for PersistedSessionManifest {
    fn from(record: &SessionRecord) -> Self {
        Self {
            schema_version: record.schema_version,
            session_id: record.session_id.clone(),
            source: record.source.clone(),
            started_at: record.started_at,
            captured_at: record.captured_at,
            metadata: record.metadata.clone(),
            links: record.links.clone(),
            track_id: record.track_id.clone(),
            anchor_ticket_id: record.anchor_ticket_id.clone(),
            parent_session_id: record.parent_session_id.clone(),
            spawned_session_id: record.spawned_session_id.clone(),
            emitted_handoff_ids: record.emitted_handoff_ids.clone(),
            picked_up_handoff_ids: record.picked_up_handoff_ids.clone(),
            active_run_id: String::new(),
            runs: Vec::new(),
            pinned_entities: Vec::new(),
            workflow: SessionWorkflowGraph::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedSessionTranscript {
    #[serde(default = "crate::default_session_schema_version")]
    pub schema_version: u32,
    pub session_id: String,
    pub captured_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<SessionTurn>,
}

impl From<&SessionRecord> for PersistedSessionTranscript {
    fn from(record: &SessionRecord) -> Self {
        Self {
            schema_version: record.schema_version,
            session_id: record.session_id.clone(),
            captured_at: record.captured_at,
            turns: record.turns.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSessionEvents {
    #[serde(default = "crate::default_session_schema_version")]
    pub schema_version: u32,
    pub session_id: String,
    pub captured_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<CopilotHookEvent>,
}
