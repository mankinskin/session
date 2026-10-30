use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub const SESSION_SCHEMA_VERSION: u32 = 1;

pub fn default_session_schema_version() -> u32 {
    SESSION_SCHEMA_VERSION
}

#[path = "model/terminal.rs"]
mod terminal;
#[path = "model/workflow.rs"]
mod workflow;

pub use handoff::{
    HandoffBacklogFilter, SessionFinishRecord, SessionFinishResult, SessionHandoffPackage,
    SessionHandoffRecord, SessionHandoffResult, SessionHandoffTargetTicket,
    SessionHandoffUpwardContextEntry, SessionHandoffUpwardContextRole,
};
pub use links::SessionLinks;
pub use pin_feedback::SessionPinFeedbackSink;
pub use terminal::{
    SessionTerminalCreateRequest, SessionTerminalEvent, SessionTerminalManifest,
    SessionTerminalPeekResult, SessionTerminalRecord, SessionTerminalStatus,
};
pub use workflow::{
    SessionTicketStateResolver, SessionValidationGate, SessionWorkflowDiagnostic,
    SessionWorkflowEdge, SessionWorkflowEdgeKind, SessionWorkflowGraph, SessionWorkflowNode,
    SessionWorkflowNodeDraft, SessionWorkflowNodeKind, SessionWorkflowNodePatch,
    SessionWorkflowNodeRequirement, SessionWorkflowNodeResolution, SessionWorkflowNodeStatus,
    SessionWorkflowSnapshot, SessionWorkflowValidationIssue, validate_workflow_graph,
};

#[path = "model/handoff.rs"]
mod handoff;
#[path = "model/links.rs"]
mod links;
#[path = "model/pin_feedback.rs"]
mod pin_feedback;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionPinnedEntityKind {
    Ticket,
    Spec,
    Rule,
    Dossier,
    Mission,
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

    pub fn runs_for_session(&self, session_id: &str) -> Vec<&SessionRunLineage> {
        self.runs
            .iter()
            .filter(|run| {
                run.captured_session_id
                    .as_deref()
                    .map_or(false, |id| id == session_id)
            })
            .collect()
    }

    pub fn session_for_run(&self, run_id: &str) -> Option<&str> {
        self.runs
            .iter()
            .find(|run| run.run_id == run_id)
            .and_then(|run| run.captured_session_id.as_deref())
    }

    pub fn find_pin_mut(&mut self, urn: &str) -> Option<&mut SessionPinnedEntity> {
        self.pinned_entities.iter_mut().find(|pin| pin.urn == urn)
    }

    pub fn remove_pin(&mut self, urn: &str) -> bool {
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
    #[serde(alias = "workspace_slug")]
    pub workspace_path: String,
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
#[path = "model/tests.rs"]
mod tests;
