use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};
use serde_json::Value;
use std::path::PathBuf;

pub const SESSION_SCHEMA_VERSION: u32 = 1;

pub fn default_session_schema_version() -> u32 {
    SESSION_SCHEMA_VERSION
}

mod terminal;
mod workflow;

pub use handoff::{
    HandoffBacklogFilter,
    SessionFinishRecord,
    SessionFinishResult,
    SessionHandoffPackage,
    SessionHandoffRecord,
    SessionHandoffResult,
    SessionHandoffTargetTicket,
    SessionHandoffUpwardContextEntry,
    SessionHandoffUpwardContextRole,
};
pub use links::SessionLinks;
pub use pin_feedback::SessionPinFeedbackSink;
pub use terminal::{
    SessionTerminalCreateRequest,
    SessionTerminalEvent,
    SessionTerminalManifest,
    SessionTerminalPeekResult,
    SessionTerminalRecord,
    SessionTerminalStatus,
};
pub use workflow::{
    SessionTicketStateResolver,
    SessionValidationGate,
    SessionWorkflowDiagnostic,
    SessionWorkflowEdge,
    SessionWorkflowEdgeKind,
    SessionWorkflowGraph,
    SessionWorkflowNode,
    SessionWorkflowNodeDraft,
    SessionWorkflowNodeKind,
    SessionWorkflowNodePatch,
    SessionWorkflowNodeRequirement,
    SessionWorkflowNodeResolution,
    SessionWorkflowNodeStatus,
    SessionWorkflowSnapshot,
    SessionWorkflowValidationIssue,
    validate_workflow_graph,
};

mod handoff;
mod links;
mod pin_feedback;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionPinnedEntityKind {
    Ticket,
    Spec,
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPinnedEntity {
    pub urn: String,
    pub kind: SessionPinnedEntityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub pinned_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRunLineage {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_session_id: Option<String>,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRuntimeContext {
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub active_run_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<SessionRunLineage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_entities: Vec<SessionPinnedEntity>,
    #[serde(default)]
    pub workflow: SessionWorkflowGraph,
}

impl SessionRuntimeContext {
    pub fn canonical_session_id(&self) -> String {
        self.session_id.clone()
    }

    pub fn active_run(&self) -> Option<&SessionRunLineage> {
        self.runs
            .iter()
            .find(|run| run.run_id == self.active_run_id)
    }

    pub fn runs_for_session(
        &self,
        session_id: &str,
    ) -> Vec<&SessionRunLineage> {
        self.runs
            .iter()
            .filter(|run| {
                run.captured_session_id
                    .as_deref()
                    .map_or(false, |id| id == session_id)
            })
            .collect()
    }

    pub fn session_for_run(
        &self,
        run_id: &str,
    ) -> Option<&str> {
        self.runs
            .iter()
            .find(|run| run.run_id == run_id)
            .and_then(|run| run.captured_session_id.as_deref())
    }

    pub fn find_pin_mut(
        &mut self,
        urn: &str,
    ) -> Option<&mut SessionPinnedEntity> {
        self.pinned_entities.iter_mut().find(|pin| pin.urn == urn)
    }

    pub fn remove_pin(
        &mut self,
        urn: &str,
    ) -> bool {
        let before = self.pinned_entities.len();
        self.pinned_entities.retain(|pin| pin.urn != urn);
        self.pinned_entities.len() != before
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRuntimeInitRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_run_id: Option<String>,
    #[serde(default)]
    pub force_new_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRuntimeInitResult {
    pub context: SessionRuntimeContext,
    pub run: SessionRunLineage,
    pub created_workspace: bool,
    pub created_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPinnedEntityHeader {
    pub urn: String,
    pub kind: SessionPinnedEntityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRuntimeView {
    pub session_id: String,
    pub active_run_id: String,
    pub pinned_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_headers: Vec<SessionPinnedEntityHeader>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionTurnEventMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_success: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_requests_json: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_arguments_json: Option<Value>,
    // Token and cost attribution (ticket 6549b6a7)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    // Payload telemetry (MCP tool calls) — ticket 9d527ad1
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_chars: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_chars: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_estimated: Option<u64>,
    // Tool execution failure/timeout classification (ticket 84c7757d)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_code: Option<String>,
    /// Sub-agent span attribution (ticket b7c61f0e): the `tool_call_id` of the
    /// nearest enclosing `runSubagent` invocation whose
    /// `tool.execution_start`/`tool.execution_complete` bracket contains this
    /// event, resolved at capture time from true `parent_event_id` ancestry
    /// (not raw event-index overlap) so nested and parallel sub-agent spans
    /// are attributed without double-counting. `None` for top-level
    /// orchestrator turns that are not nested inside any `runSubagent` call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_run_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTurn {
    pub sequence: usize,
    pub role: SessionRole,
    pub content: String,
    pub captured_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Active model that produced this turn, when known. `None` means the turn
    /// inherits the session-level model in [`SessionMetadata::model`]. This lets
    /// mid-session model routing (a large model delegating to cheaper ones) be
    /// observed at turn granularity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_meta: Option<SessionTurnEventMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionProvisioningDiagnostic {
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub hook_event_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub workspace_slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provisioning: Option<SessionProvisioningDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copilot_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vscode_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<SessionWorktreeAssignment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionWorktreeAllocationMode {
    New,
    Reused,
    Rotated,
}

impl Default for SessionWorktreeAllocationMode {
    fn default() -> Self {
        Self::New
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionWorktreeStatus {
    Active,
    Superseded,
    Invalidated,
}

impl Default for SessionWorktreeStatus {
    fn default() -> Self {
        Self::Active
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorktreeAssignment {
    #[serde(default)]
    pub path: PathBuf,
    pub branch: String,
    #[serde(default)]
    pub allocation_mode: SessionWorktreeAllocationMode,
    #[serde(default)]
    pub status: SessionWorktreeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    #[serde(default = "default_session_schema_version")]
    pub schema_version: u32,
    pub session_id: String,
    pub source: String,
    pub started_at: DateTime<Utc>,
    pub captured_at: DateTime<Utc>,
    pub metadata: SessionMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<SessionTurn>,
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
    /// Handoff ids created (emitted) by this session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emitted_handoff_ids: Vec<String>,
    /// Handoff ids this session picked up (bound as target).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub picked_up_handoff_ids: Vec<String>,
}

impl SessionRecord {
    pub fn has_turns(&self) -> bool {
        !self.turns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    use super::{
        SessionLinks,
        SessionMetadata,
        SessionRecord,
        SessionRole,
        SessionTurn,
        SessionTurnEventMeta,
        SessionWorktreeAllocationMode,
        SessionWorktreeAssignment,
        SessionWorktreeStatus,
    };
    use crate::SESSION_SCHEMA_VERSION;

    fn sample_time() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 6, 2, 12, 0, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn session_record_round_trips_through_serde() {
        let record = SessionRecord {
            schema_version: SESSION_SCHEMA_VERSION,
            session_id: "session-123".to_string(),
            source: "copilot-hook".to_string(),
            started_at: sample_time(),
            captured_at: sample_time(),
            metadata: SessionMetadata {
                workspace_slug: "context-engine".to_string(),
                conversation_id: Some("conversation-1".to_string()),
                agent_id: Some("github-copilot-gpt-5.4".to_string()),
                ticket_id: Some("ticket-1".to_string()),
                model: Some("GPT-5.4".to_string()),
                trigger: Some("post-turn".to_string()),
                provisioning: None,
                producer: Some("copilot-agent".to_string()),
                copilot_version: Some("0.55.0".to_string()),
                vscode_version: Some("1.127.0".to_string()),
                protocol_version: Some(1),
                worktree: Some(SessionWorktreeAssignment {
                    path: PathBuf::from("worktrees/session-123"),
                    branch: "session/session-123".to_string(),
                    allocation_mode: SessionWorktreeAllocationMode::New,
                    status: SessionWorktreeStatus::Active,
                    predecessor_session_id: None,
                    predecessor_path: None,
                }),
            },
            turns: vec![SessionTurn {
                sequence: 0,
                role: SessionRole::User,
                content: "Summarize the test failures".to_string(),
                captured_at: sample_time(),
                tool_name: None,
                model: Some("GPT-5.4".to_string()),
                event_meta: Some(SessionTurnEventMeta {
                    event_id: Some("evt-1".to_string()),
                    parent_event_id: None,
                    event_type: Some("user.message".to_string()),
                    turn_id: Some("turn-1".to_string()),
                    message_id: Some("msg-1".to_string()),
                    tool_call_id: None,
                    tool_success: None,
                    reasoning_text: None,
                    tool_requests_json: None,
                    tool_arguments_json: None,
                    // Token and cost attribution (ticket 6549b6a7)
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    cost_usd: None,
                    model_id: None,
                    // Payload telemetry (ticket 9d527ad1)
                    request_bytes: None,
                    request_chars: None,
                    response_bytes: None,
                    response_chars: None,
                    tokens_estimated: None,
                    // Tool execution failure/timeout classification (ticket 84c7757d)
                    error_message: None,
                    exit_code: None,
                    result_code: None,
                    subagent_run_id: None,
                }),
            }],
            links: SessionLinks {
                ticket_ids: vec!["ticket-1".to_string()],
                spec_ids: vec!["spec-1".to_string()],
                doc_evidence_ids: vec!["doc-1".to_string()],
                log_ids: vec!["log-1".to_string()],
                runtime_session_id: Some(
                    "03baab6c-0fdb-4ffc-8159-b83066a6283f".to_string(),
                ),
                runtime_run_id: Some(
                    "8cf1255d-7969-4ac2-905a-cbd234dc3eac".to_string(),
                ),
            },
            track_id: None,
            anchor_ticket_id: None,
            parent_session_id: None,
            spawned_session_id: None,
            emitted_handoff_ids: Vec::new(),
            picked_up_handoff_ids: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&record).unwrap();
        let reparsed: SessionRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(reparsed, record);
        assert_eq!(
            record.turns[0].model.as_deref(),
            Some("GPT-5.4"),
            "per-turn model should round-trip"
        );
        assert!(record.links.links_to_ticket("ticket-1"));
        assert!(record.links.links_to_spec("spec-1"));
    }

    #[test]
    fn existing_sessions_deserialize_without_track_fields() {
        // Real on-disk session format from pre-track schema (AC2 verification)
        let legacy_json = r#"{
            "schema_version": 1,
            "session_id": "01246bdc-dd6b-4807-bda3-38cf6aa780de",
            "source": "copilot-hook",
            "started_at": "2026-07-25T18:57:59.736Z",
            "captured_at": "2026-07-26T01:23:11.736Z",
            "metadata": {
                "workspace_slug": "default",
                "agent_id": "copilot-agent",
                "trigger": "Stop",
                "producer": "copilot-agent",
                "copilot_version": "0.58.0",
                "vscode_version": "1.130.0",
                "protocol_version": 1
            },
            "links": {}
        }"#;

        let record: SessionRecord = serde_json::from_str(legacy_json).unwrap();

        // All four track fields must deserialize to None
        assert_eq!(
            record.track_id, None,
            "track_id must be None for legacy sessions"
        );
        assert_eq!(
            record.anchor_ticket_id, None,
            "anchor_ticket_id must be None for legacy sessions"
        );
        assert_eq!(
            record.parent_session_id, None,
            "parent_session_id must be None for legacy sessions"
        );
        assert_eq!(
            record.spawned_session_id, None,
            "spawned_session_id must be None for legacy sessions"
        );

        // Round-trip must not emit the track fields
        let reserialized = serde_json::to_string(&record).unwrap();
        assert!(
            !reserialized.contains("track_id"),
            "track_id must not appear in JSON when None"
        );
        assert!(
            !reserialized.contains("anchor_ticket_id"),
            "anchor_ticket_id must not appear in JSON when None"
        );
        assert!(
            !reserialized.contains("parent_session_id"),
            "parent_session_id must not appear in JSON when None"
        );
        assert!(
            !reserialized.contains("spawned_session_id"),
            "spawned_session_id must not appear in JSON when None"
        );
    }

    #[test]
    fn existing_sessions_deserialize_without_provisioning_metadata() {
        let legacy_json = r#"{"schema_version":1,"session_id":"93d9261e-93e8-4ae3-9f4c-02c17e7c8568","source":"copilot-hook","started_at":"2026-08-10T17:57:22.617Z","captured_at":"2026-08-10T18:53:14.528Z","metadata":{"workspace_slug":"default","agent_id":"copilot-agent","trigger":"Stop","producer":"copilot-agent","copilot_version":"0.60.0","vscode_version":"1.132.0","protocol_version":1,"worktree":{"path":"C:/Users/linus/git/context-engine","branch":"main","allocation_mode":"new","status":"active"}},"links":{}}"#;

        let record: SessionRecord = serde_json::from_str(legacy_json).unwrap();

        assert_eq!(record.metadata.provisioning, None);
    }
}
