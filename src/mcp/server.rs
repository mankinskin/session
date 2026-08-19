use std::path::PathBuf;

use rmcp::{
    ErrorData as McpError,
    ServerHandler,
    ServiceExt,
    handler::server::{
        tool::ToolRouter,
        wrapper::Parameters,
    },
    model::*,
    schemars::{
        self,
        JsonSchema,
    },
    tool,
    tool_handler,
    tool_router,
    transport::stdio,
};
use serde::{
    Deserialize,
    Serialize,
};
use uuid::Uuid;

use memory_kernel::workspace;
use session_api::{
    DEFAULT_SKELETON_PREVIEW_CHARS,
    RelationStrength,
    SessionError,
    SessionHandoffPackage,
    SessionHandoffTargetTicket,
    SessionHandoffUpwardContextEntry,
    SessionHandoffUpwardContextRole,
    SessionQuery,
    SessionRuntimeInitRequest,
    SessionTerminalCreateRequest,
    SessionStoreConfig,
    SessionValidationGate,
    SessionWorkflowEdge,
    SessionWorkflowEdgeKind,
    SessionWorkflowNodeDraft,
    SessionWorkflowNodeKind,
    SessionWorkflowNodePatch,
    SessionWorkflowNodeRequirement,
    SessionWorkflowNodeStatus,
    SessionWorktreeCheckInRequest,
    ToolMetricsWindow,
};

// ── Workflow enum schema advertisement ─────────────────────────────────────
//
// The workflow mutation input fields stay typed as `String` so the `parse_*`
// functions can accept snake_case/kebab-case aliases and return an explicit
// allowed-values error on rejection. These helper enums exist only to project
// the legal values into the generated JSON schema via `#[schemars(with = ...)]`,
// so agents can discover the enum before ever making a call. They mirror the
// `session-api` enums exactly; `workflow_enum_parity` asserts they do not drift.

/// Legal `session_workflow_add_node.kind` values (behavioral node kinds).
#[derive(JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum WorkflowNodeKindSchema {
    Ticket,
    Validation,
    Spec,
    Task,
}

/// Legal `session_workflow_add_node.requirement` values.
#[derive(JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum WorkflowRequirementSchema {
    Required,
    Optional,
}

/// Legal `session_workflow_add_edge.kind` values.
#[derive(JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum WorkflowEdgeKindSchema {
    DependsOn,
    Order,
}

/// Legal `session_workflow_set_status.status` values.
#[derive(JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum WorkflowNodeStatusSchema {
    Pending,
    InProgress,
    Blocked,
    Done,
    Deferred,
}

/// Legal `session_sessions_for_ticket.strength` values, mirroring
/// `session_api::RelationStrength` exactly (widening tiers).
#[derive(JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum RelationStrengthSchema {
    Strict,
    Linked,
    Mentioned,
}

// ── Input types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckInInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    /// Session id to check in.
    pub session_id: String,
    /// Owner (agent) identity claiming the worktree.
    pub owner_id: String,
    /// Ticket the session is working on.
    pub ticket_id: String,
    /// Assigned worktree working directory.
    pub worktree_path: String,
    /// Branch checked out in the worktree.
    pub branch: String,
    /// Predecessor session id when rotating from a prior assignment.
    #[serde(default)]
    pub predecessor_session_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LookupInput {
    /// Session id to look up.
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryInput {
    /// Filter by session id prefix.
    #[serde(default)]
    pub session_id_prefix: Option<String>,
    /// Filter by conversation id.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Filter by agent id.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Free-text filter across session content.
    #[serde(default)]
    pub text: Option<String>,
    /// Maximum number of sessions to return.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionsForTicketInput {
    /// Ticket id to find related sessions for.
    pub ticket_id: String,
    /// Relation-strength tier. Legal values: strict, linked, mentioned
    /// (widening: each includes the tiers before it).
    #[schemars(with = "RelationStrengthSchema")]
    pub strength: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeekRangeInput {
    /// Session id to peek.
    pub session_id: String,
    /// Inclusive start turn index (0-based).
    #[serde(default)]
    pub start: usize,
    /// Exclusive end turn index (0-based). Defaults to the end of the transcript.
    #[serde(default)]
    pub end: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeekSkeletonInput {
    /// Session id to peek.
    pub session_id: String,
    /// Maximum preview characters retained per turn.
    #[serde(default)]
    pub preview_chars: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TerminalCreateInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store.
    pub workspace: String,
    pub session_id: String,
    pub label: String,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TerminalStatusInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store.
    pub workspace: String,
    pub session_id: String,
    pub terminal_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TerminalPeekInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store.
    pub workspace: String,
    pub session_id: String,
    pub terminal_id: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionMoveInput {
    /// Session UUID to move.
    pub id: String,
    /// Destination workspace root.
    pub to_workspace_root: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionMoveJournalInput {
    /// Move journal UUID.
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeInitInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    /// Copilot session UUID. Required; the MCP tool does not resolve one implicitly.
    pub session_id: String,
    #[serde(default)]
    pub predecessor_run_id: Option<String>,
    #[serde(default)]
    pub force_new_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeResumeInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    pub predecessor_run_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimePinInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    pub entity_urn: String,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeUnpinInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    pub entity_urn: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ToolMetricsInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    /// Maximum age in days for included sessions.
    #[serde(default)]
    pub days: Option<u32>,
    /// Maximum number of sessions to include.
    #[serde(default)]
    pub max_sessions: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubagentRollupsInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    /// Copilot session UUID to get rollups for.
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrantCreateInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    /// Grant scope: session or subagent.
    pub scope: String,
    /// Budget offset to add.
    pub offset: u32,
    /// Optional model constraint (case-insensitive).
    #[serde(default)]
    pub model: Option<String>,
    /// Optional TTL in seconds from now.
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrantListInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrantRevokeInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    /// Grant ID to revoke.
    pub grant_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EscalationCreateInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    /// The blocking decision or problem statement.
    pub blocking_decision: String,
    /// Context explaining the situation.
    pub context: String,
    /// Optional requested capability or resource.
    #[serde(default)]
    pub requested_capability: Option<String>,
    /// Options considered before escalating.
    #[serde(default)]
    pub options_considered: Vec<String>,
    /// Optional session ID that created the escalation.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Optional model that created the escalation.
    #[serde(default)]
    pub from_model: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EscalationListInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    /// Optional status filter: open or resolved.
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EscalationGetInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    /// Escalation ID to retrieve.
    pub escalation_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EscalationResolveInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    /// Escalation ID to resolve.
    pub escalation_id: String,
    /// Resolution action: handled, granted-offset, escalated-to-user, spawned-session.
    pub action: String,
    /// Optional note about the resolution.
    #[serde(default)]
    pub note: Option<String>,
    /// Grant ID (required when action is granted-offset).
    #[serde(default)]
    pub grant_id: Option<String>,
    /// Spawned session ID (required when action is spawned-session).
    #[serde(default)]
    pub spawned_session_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeViewInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
}

/// No-argument input for the self-describing capability catalog.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CapabilitiesInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowAddNodeInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    #[serde(default)]
    pub node_id: Option<String>,
    /// Behavioral node kind. Legal values: ticket, validation, spec, task
    /// (deprecated aliases mapped to task: action, decision, checkpoint).
    #[schemars(with = "WorkflowNodeKindSchema")]
    pub kind: String,
    /// Whether the node gates finish. Legal values: required, optional.
    #[schemars(with = "WorkflowRequirementSchema")]
    pub requirement: String,
    pub title: String,
    #[serde(default)]
    pub ticket_urn: Option<String>,
    /// Spec URN for a `spec` behavioral node (mirror of ticket_urn).
    #[serde(default)]
    pub spec_urn: Option<String>,
    /// Optional ticket or spec URN for context on any node kind. Never gates
    /// finish; use ticket_urn/spec_urn for their matching behavioral kinds.
    #[serde(default)]
    pub anchor_urn: Option<String>,
    /// Free-text custom label for descriptive nodes. To model a would-be
    /// custom kind, keep the behavioral kind as `task` and put your label in
    /// `category` — for example `kind="task", category="<your-label>"` (such as
    /// `kind="task", category="review-criterion"`). No gating logic branches on
    /// this value.
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub cached_ticket_title: Option<String>,
    #[serde(default)]
    pub validation_spec_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowAddEdgeInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    pub from: String,
    pub to: String,
    /// Edge kind. Legal values: depends-on (alias depends_on), order.
    #[schemars(with = "WorkflowEdgeKindSchema")]
    pub kind: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowNodeDraftInput {
    #[serde(default)]
    pub node_id: Option<String>,
    #[schemars(with = "WorkflowNodeKindSchema")]
    pub kind: String,
    #[schemars(with = "WorkflowRequirementSchema")]
    pub requirement: String,
    pub title: String,
    #[serde(default)]
    pub ticket_urn: Option<String>,
    #[serde(default)]
    pub spec_urn: Option<String>,
    #[serde(default)]
    pub anchor_urn: Option<String>,
    /// Free-text custom label for descriptive nodes. To model a would-be
    /// custom kind, keep the behavioral kind as `task` and put your label in
    /// `category` — for example `kind="task", category="<your-label>"` (such as
    /// `kind="task", category="review-criterion"`). No gating logic branches on
    /// this value.
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub cached_ticket_title: Option<String>,
    #[serde(default)]
    pub validation_spec_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowAddNodesInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    pub nodes: Vec<WorkflowNodeDraftInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowEdgeDraftInput {
    pub from: String,
    pub to: String,
    #[schemars(with = "WorkflowEdgeKindSchema")]
    pub kind: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowAddEdgesInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    pub edges: Vec<WorkflowEdgeDraftInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowSetStatusInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    pub node_id: String,
    /// Node status. Legal values: pending, in-progress (alias in_progress),
    /// blocked, done, deferred.
    #[schemars(with = "WorkflowNodeStatusSchema")]
    pub status: String,
    #[serde(default)]
    pub deferred_reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowPromoteInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    pub node_id: String,
    pub ticket_urn: String,
    #[serde(default)]
    pub cached_ticket_title: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowUpdateNodeInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    pub node_id: String,
    /// Behavioral node kind to set. Omit to leave unchanged. Legal values:
    /// ticket, validation, spec, task.
    #[serde(default)]
    #[schemars(with = "Option<WorkflowNodeKindSchema>")]
    pub kind: Option<String>,
    /// Requirement to set. Omit to leave unchanged. Legal values: required, optional.
    #[serde(default)]
    #[schemars(with = "Option<WorkflowRequirementSchema>")]
    pub requirement: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub ticket_urn: Option<String>,
    #[serde(default)]
    pub spec_urn: Option<String>,
    #[serde(default)]
    pub anchor_urn: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub cached_ticket_title: Option<String>,
    #[serde(default)]
    pub validation_spec_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowRemoveNodeInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    pub node_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowRenderInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeHandoffInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    #[serde(default)]
    pub validation: Vec<ValidationGateInput>,
    /// The single goal of the next implementation unit (required for an
    /// implementation-ready package).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub objective: String,
    /// Tickets expected to be worked in the next session. Accepts legacy ticket
    /// id strings or objects with `id`, author-supplied `why`, and optional
    /// cached `state` and `acceptance_criteria`.
    #[serde(default)]
    pub target_tickets: Vec<HandoffTargetTicketInput>,
    /// Why the next implementation unit matters to the broader program.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub higher_level_objective: String,
    /// Ordered ancestor chain for the next implementation unit. Every entry
    /// supplies `entity_urn`, `title`, and `role`; legal roles are `epic`,
    /// `phase`, and `parent`.
    #[serde(default)]
    pub upward_context: Vec<HandoffUpwardContextInput>,
    /// Workspace-relative file paths expected to be touched.
    #[serde(default)]
    pub target_files: Vec<String>,
    /// Resolved design choices.
    #[serde(default)]
    pub decisions: Vec<String>,
    /// Explicit out-of-scope boundaries.
    #[serde(default)]
    pub non_goals: Vec<String>,
    /// Prior findings and ids needed so no search is required.
    #[serde(default)]
    pub context_anchors: Vec<String>,
    /// Must be empty for the package to be implementation-ready.
    #[serde(default)]
    pub open_escalations: Vec<String>,
    /// Known risks or fragile areas (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_notes: Option<String>,
    /// Id of the handoff this one supersedes (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_handoff: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum HandoffTargetTicketInput {
    Legacy(String),
    Structured {
        id: String,
        #[serde(default)]
        why: String,
        #[serde(default)]
        state: String,
        #[serde(default)]
        acceptance_criteria: Vec<String>,
    },
}

impl From<HandoffTargetTicketInput> for SessionHandoffTargetTicket {
    fn from(value: HandoffTargetTicketInput) -> Self {
        match value {
            HandoffTargetTicketInput::Legacy(id) => Self {
                id,
                why: String::new(),
                state: String::new(),
                acceptance_criteria: Vec::new(),
            },
            HandoffTargetTicketInput::Structured {
                id,
                why,
                state,
                acceptance_criteria,
            } => Self {
                id,
                why,
                state,
                acceptance_criteria,
            },
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HandoffUpwardContextInput {
    pub entity_urn: String,
    pub title: String,
    /// Legal values are `epic`, `phase`, and `parent`.
    pub role: HandoffUpwardContextRoleInput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HandoffUpwardContextRoleInput {
    Epic,
    Phase,
    Parent,
}

impl From<HandoffUpwardContextInput> for SessionHandoffUpwardContextEntry {
    fn from(value: HandoffUpwardContextInput) -> Self {
        Self {
            entity_urn: value.entity_urn,
            title: value.title,
            role: match value.role {
                HandoffUpwardContextRoleInput::Epic =>
                    SessionHandoffUpwardContextRole::Epic,
                HandoffUpwardContextRoleInput::Phase =>
                    SessionHandoffUpwardContextRole::Phase,
                HandoffUpwardContextRoleInput::Parent =>
                    SessionHandoffUpwardContextRole::Parent,
            },
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeFinishInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', '.', or '..' for entity creation.
    pub workspace: String,
    pub session_id: String,
    #[serde(default)]
    pub validation: Vec<ValidationGateInput>,
    #[serde(default)]
    pub deferred_optional_node_ids: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidationGateInput {
    pub validation_spec_id: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub outcome: Option<String>,
    /// The command that performs the validation check. Optional; when absent,
    /// `validation_spec_id` should reference a test-api ValidationSpec entry.
    #[serde(default)]
    pub command: Option<String>,
}

impl From<ValidationGateInput> for SessionValidationGate {
    fn from(value: ValidationGateInput) -> Self {
        Self {
            validation_spec_id: value.validation_spec_id,
            required: value.required,
            outcome: value.outcome,
            command: value.command,
        }
    }
}

// ── Server ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SessionServer {
    store_root: PathBuf,
    workspace_slug: String,
    tool_router: ToolRouter<Self>,
}

impl SessionServer {
    pub fn new(
        store_root: PathBuf,
        workspace_slug: String,
    ) -> Self {
        Self {
            store_root,
            workspace_slug,
            tool_router: Self::tool_router(),
        }
    }

    fn config(&self) -> SessionStoreConfig {
        SessionStoreConfig::new(
            self.store_root.clone(),
            self.workspace_slug.clone(),
        )
    }

    fn config_for_workspace(
        &self,
        workspace_selector: &str,
    ) -> Result<SessionStoreConfig, McpError> {
        let workspace_selector =
            workspace::validate_explicit_workspace_selector(Some(
                workspace_selector,
            ))
            .map_err(|err| McpError::invalid_params(err.to_string(), None))?;
        let store_root = workspace::resolve_store_root_from(
            std::path::Path::new(workspace_selector),
            ".session",
        );
        Ok(SessionStoreConfig::new(
            store_root,
            self.workspace_slug.clone(),
        ))
    }

    fn json_result<T: Serialize>(
        value: &T
    ) -> Result<CallToolResult, McpError> {
        let text = serde_json::to_string(value).map_err(|err| {
            McpError::internal_error(format!("serialization: {err}"), None)
        })?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Serialize `value` and guarantee the Copilot session UUID is present as
    /// the prominent top-line `session_id` field required by every
    /// workflow/runtime call.
    fn json_result_with_handle<T: Serialize>(
        session_id: &str,
        value: &T,
    ) -> Result<CallToolResult, McpError> {
        let mut payload = serde_json::to_value(value).map_err(|err| {
            McpError::internal_error(format!("serialization: {err}"), None)
        })?;
        match payload.as_object_mut() {
            Some(object) => {
                object.insert(
                    "session_id".to_string(),
                    serde_json::Value::String(session_id.to_string()),
                );
            },
            None => {
                payload = serde_json::json!({
                    "session_id": session_id,
                    "result": payload,
                });
            },
        }
        let text = serde_json::to_string(&payload).map_err(|err| {
            McpError::internal_error(format!("serialization: {err}"), None)
        })?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    fn session_err(err: SessionError) -> McpError {
        match &err {
            SessionError::NotFound { .. }
            | SessionError::MissingSessionId
            | SessionError::MissingOwnerId
            | SessionError::MissingTicketId
            | SessionError::EmptyWorktreePath
            | SessionError::EmptyWorktreeBranch
            | SessionError::InvalidSessionId(_)
            | SessionError::InvalidWorkspaceSessionId(_)
            | SessionError::InvalidEntityUrn(_)
            | SessionError::InvalidWorkspaceSlug(_)
            | SessionError::MissingWorktreeAssignment { .. }
            | SessionError::SessionOwnershipMismatch { .. }
            | SessionError::WorktreeConflict { .. }
            | SessionError::CrossSessionReuseRequiresAdopt { .. }
            | SessionError::RuntimeContextNotFound { .. }
            | SessionError::FinishBlocked { .. }
            | SessionError::WorkflowGraphInvalid { .. }
            | SessionError::WorkflowDiagnosticsUnresolved { .. }
            | SessionError::Move(_) =>
                McpError::invalid_params(err.to_string(), None),
            _ =>
                McpError::internal_error(format!("session error: {err}"), None),
        }
    }

    fn move_plan_json(
        report: &memory_kernel::storage::move_kernel::MovePlan
    ) -> Result<serde_json::Value, McpError> {
        Ok(serde_json::json!({
            "supported": report.supported(),
            "entity_id": report.entity_id,
            "source_workspace_root": path_display(&report.source_workspace_root)?,
            "target_workspace_root": path_display(&report.target_workspace_root)?,
            "source_store_root": path_display(&report.source_store_root)?,
            "target_store_root": path_display(&report.target_store_root)?,
            "source_git_worktree_root": path_display(&report.source_git_worktree_root)?,
            "target_git_worktree_root": path_display(&report.target_git_worktree_root)?,
            "git_worktree_topology": report.git_worktree_topology,
            "source_entity_path": path_display(&report.source_entity_path)?,
            "destination_entity_path": path_display(&report.destination_entity_path)?,
            "inbound_related_entity_ids": report.inbound_related_entity_ids,
            "outbound_related_entity_ids": report.outbound_related_entity_ids,
            "reference_visibility": report.reference_visibility,
            "active_board_entries": report.active_board_entries,
            "historical_board_entries": report.historical_board_entries,
            "active_leases": report.active_leases,
            "path_reference_files": report.path_reference_files
                .iter()
                .map(|p| path_display(p))
                .collect::<Result<Vec<_>, _>>()?,
            "blockers": report.blockers,
            "captured_at": report.captured_at,
        }))
    }

    fn move_outcome_json(
        outcome: &memory_kernel::storage::move_kernel::MoveOutcome
    ) -> Result<serde_json::Value, McpError> {
        Ok(serde_json::json!({
            "resumed": outcome.resumed,
            "rolled_back": outcome.rolled_back,
            "journal": {
                "id": outcome.journal.id,
                "entity_id": outcome.journal.entity_id,
                "source_store_root": path_display(&outcome.journal.source_store_root)?,
                "target_store_root": path_display(&outcome.journal.target_store_root)?,
                "source_entity_path": path_display(&outcome.journal.source_entity_path)?,
                "destination_entity_path": path_display(&outcome.journal.destination_entity_path)?,
                "phase": outcome.journal.phase,
                "created_at": outcome.journal.created_at,
                "updated_at": outcome.journal.updated_at,
                "steps": outcome.journal.steps,
                "rollback_steps": outcome.journal.rollback_steps,
                "lock_paths": outcome.journal.lock_paths,
                "migrated_board_entries": outcome.journal.migrated_board_entries,
                "rewritten_path_files": outcome.journal.rewritten_path_files,
                "manual_followups": outcome.journal.manual_followups,
                "failure": outcome.journal.failure,
                "next_recovery_step": outcome.journal.next_recovery_step,
            },
        }))
    }
}

fn path_display(path: &std::path::Path) -> Result<String, McpError> {
    workspace::normalize_path_for_display_strict(path).map_err(|error| {
        McpError::invalid_params(
            format!(
                "path payload normalization failed for '{}': {error}",
                path.display()
            ),
            None,
        )
    })
}

fn parse_node_kind(value: &str) -> Result<SessionWorkflowNodeKind, McpError> {
    match value {
        "ticket" => Ok(SessionWorkflowNodeKind::Ticket),
        "validation" => Ok(SessionWorkflowNodeKind::Validation),
        "spec" => Ok(SessionWorkflowNodeKind::Spec),
        // `task` is the generic descriptive bucket. The legacy cosmetic kinds
        // are accepted as back-compat aliases so old call sites keep working.
        "task" | "action" | "decision" | "checkpoint" =>
            Ok(SessionWorkflowNodeKind::Task),
        _ => Err(McpError::invalid_params(
            format!(
                "invalid workflow node kind: {value}. allowed values: \
                 ticket, validation, spec, task \
                 (deprecated aliases mapped to task: action, decision, checkpoint); \
                 for a custom label, use kind=task with category=\"<your-label>\""
            ),
            None,
        )),
    }
}

fn parse_requirement(
    value: &str
) -> Result<SessionWorkflowNodeRequirement, McpError> {
    match value {
        "required" => Ok(SessionWorkflowNodeRequirement::Required),
        "optional" => Ok(SessionWorkflowNodeRequirement::Optional),
        _ => Err(McpError::invalid_params(
            format!(
                "invalid workflow requirement: {value}. allowed values: \
                 required, optional; did you mean requirement=required?"
            ),
            None,
        )),
    }
}

fn parse_edge_kind(value: &str) -> Result<SessionWorkflowEdgeKind, McpError> {
    match value {
        "depends-on" | "depends_on" => Ok(SessionWorkflowEdgeKind::DependsOn),
        "order" => Ok(SessionWorkflowEdgeKind::Order),
        _ => Err(McpError::invalid_params(
            format!(
                "invalid workflow edge kind: {value}. allowed values: \
                 depends-on (alias depends_on), order; \
                 did you mean kind=depends-on?"
            ),
            None,
        )),
    }
}

fn parse_node_status(
    value: &str
) -> Result<SessionWorkflowNodeStatus, McpError> {
    match value {
        "pending" => Ok(SessionWorkflowNodeStatus::Pending),
        "in-progress" | "in_progress" =>
            Ok(SessionWorkflowNodeStatus::InProgress),
        "blocked" => Ok(SessionWorkflowNodeStatus::Blocked),
        "done" => Ok(SessionWorkflowNodeStatus::Done),
        "deferred" => Ok(SessionWorkflowNodeStatus::Deferred),
        _ => Err(McpError::invalid_params(
            format!(
                "invalid workflow status: {value}. allowed values: \
                 pending, in-progress (alias in_progress), blocked, done, deferred; \
                 did you mean status=in-progress?"
            ),
            None,
        )),
    }
}

fn indexed_mcp_error(
    collection: &str,
    index: usize,
    error: McpError,
) -> McpError {
    McpError::invalid_params(
        format!("{collection}[{index}]: {}", error.message),
        None,
    )
}

/// Build the self-describing session capability catalog.
///
/// This is the discoverable entry point for the durable-workflow lifecycle so
/// agents do not have to source-dive to learn the canonical flow or the legal
/// enum values for workflow mutations. It lists the ordered lifecycle steps
/// (`runtime_init` → `pin`/`view` → `workflow_*` → `render_*` → `handoff`/
/// `finish`), the handle every workflow call requires, and the enum-valued
/// parameters mirrored from the `session-api` enums.
fn session_capability_catalog() -> serde_json::Value {
    serde_json::json!({
        "surface": "session-mcp",
        "handle": {
            "field": "session_id",
            "note": "Required by every workflow/runtime tool. Returned as a \
                     top-line field by session_runtime_init/session_runtime_resume \
                     and echoed by every workflow/runtime tool result.",
        },
        "lifecycle": {
            "name": "durable-session-workflow",
            "nested_roots_supported": true,
            "steps": [
                {"order": 1, "tool": "session_runtime_init",
                 "purpose": "Initialize or resume durable runtime context; returns the session_id handle."},
                {"order": 2, "tool": "session_runtime_pin",
                 "purpose": "Pin a ticket/spec/rule entity URN into runtime context."},
                {"order": 2, "tool": "session_runtime_view",
                 "purpose": "Read headers-only pinned-context view."},
                {"order": 3, "tool": "session_workflow_add_node",
                 "purpose": "Add a durable workflow node (see node kind/requirement enums)."},
                {"order": 3, "tool": "session_workflow_add_nodes",
                 "purpose": "Atomically add multiple durable workflow nodes."},
                {"order": 3, "tool": "session_workflow_add_edge",
                 "purpose": "Link two workflow nodes (see edge kind enum)."},
                {"order": 3, "tool": "session_workflow_add_edges",
                 "purpose": "Atomically add multiple workflow edges."},
                {"order": 3, "tool": "session_workflow_set_status",
                 "purpose": "Update a node status (see status enum)."},
                {"order": 3, "tool": "session_workflow_update_node",
                 "purpose": "Repair surface: patch fields on an existing wedged node in place."},
                {"order": 3, "tool": "session_workflow_remove_node",
                 "purpose": "Repair surface: delete a wedged node and its edges."},
                {"order": 4, "tool": "session_workflow_render_terminal",
                 "purpose": "Render the workflow graph as terminal text."},
                {"order": 4, "tool": "session_workflow_render_mermaid",
                 "purpose": "Render the workflow graph as Mermaid."},
                {"order": 5, "tool": "session_handoff",
                 "purpose": "Persist a structured handoff record."},
                {"order": 5, "tool": "session_finish",
                 "purpose": "Finish the workflow, enforcing required node and validation gates."},
            ],
        },
        "observer_terminal": {
            "tools": [
                "session_terminal_create",
                "session_terminal_status",
                "session_terminal_peek",
                "session_terminal_close"
            ],
            "note": "Observer tools never accept terminal input or shell commands. Human UI owns input; agents read bounded output only.",
        },
        "enums": {
            "workflow_node_kind": {
                "tool": "session_workflow_add_node",
                "param": "kind",
                "behavioral": ["ticket", "validation", "spec"],
                "descriptive": ["task"],
                "deprecated_aliases_mapped_to_task": ["action", "decision", "checkpoint"],
                "note": "Behavioral kinds gate finish and carry side-data: \
                         ticket->ticket_urn, spec->spec_urn, validation->validation_spec_id. \
                         Descriptive nuance belongs in the open `category` field or `title`.",
            },
            "workflow_node_requirement": {
                "tool": "session_workflow_add_node",
                "param": "requirement",
                "values": ["required", "optional"],
            },
            "workflow_edge_kind": {
                "tool": "session_workflow_add_edge",
                "param": "kind",
                "values": ["depends-on", "order"],
                "aliases": {"depends_on": "depends-on"},
            },
            "workflow_node_status": {
                "tool": "session_workflow_set_status",
                "param": "status",
                "values": ["pending", "in-progress", "blocked", "done", "deferred"],
                "aliases": {"in_progress": "in-progress"},
            },
        },
    })
}

// ── Tool implementations ──────────────────────────────────────────────────────

#[tool_router]
impl SessionServer {
    #[tool(
        name = "session_capabilities",
        description = "List the canonical session lifecycle workflow, its ordered steps, and the enum-valued workflow parameters with their legal values."
    )]
    pub async fn session_capabilities(
        &self,
        Parameters(_input): Parameters<CapabilitiesInput>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&session_capability_catalog())
    }

    #[tool(
        name = "session_runtime_init",
        description = "Initialize or resume durable runtime context for an explicit Copilot session UUID."
    )]
    pub async fn session_runtime_init(
        &self,
        Parameters(input): Parameters<RuntimeInitInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .config_for_workspace(&input.workspace)?
            .init_runtime_context(SessionRuntimeInitRequest {
                session_id: Some(input.session_id),
                predecessor_run_id: input.predecessor_run_id,
                force_new_run: input.force_new_run,
            })
            .map_err(Self::session_err)?;
        let handle = result.context.session_id.clone();
        Self::json_result_with_handle(&handle, &result)
    }

    #[tool(
        name = "session_runtime_resume",
        description = "Resume an existing Copilot session UUID using predecessor run lineage."
    )]
    pub async fn session_runtime_resume(
        &self,
        Parameters(input): Parameters<RuntimeResumeInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .config_for_workspace(&input.workspace)?
            .resume_workspace_context(
                &input.session_id,
                &input.predecessor_run_id,
            )
            .map_err(Self::session_err)?;
        let handle = result.context.session_id.clone();
        Self::json_result_with_handle(&handle, &result)
    }

    #[tool(
        name = "session_runtime_pin",
        description = "Pin an entity URN into runtime workspace context."
    )]
    pub async fn session_runtime_pin(
        &self,
        Parameters(input): Parameters<RuntimePinInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .pin_runtime_entity(
                &input.session_id,
                &input.entity_urn,
                input.relation,
                input.reason,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_runtime_unpin",
        description = "Unpin an entity URN from runtime workspace context."
    )]
    pub async fn session_runtime_unpin(
        &self,
        Parameters(input): Parameters<RuntimeUnpinInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .unpin_runtime_entity(&input.session_id, &input.entity_urn)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_runtime_view",
        description = "Read headers-only runtime workspace context view."
    )]
    pub async fn session_runtime_view(
        &self,
        Parameters(input): Parameters<RuntimeViewInput>,
    ) -> Result<CallToolResult, McpError> {
        let view = self
            .config_for_workspace(&input.workspace)?
            .view_runtime_context(&input.session_id)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &view)
    }

    #[tool(
        name = "session_runtime_render_instructions",
        description = "Render a focused instruction set from the Copilot session UUID's pinned rule URNs."
    )]
    pub async fn session_runtime_render_instructions(
        &self,
        Parameters(input): Parameters<RuntimeViewInput>,
    ) -> Result<CallToolResult, McpError> {
        let render = self
            .config_for_workspace(&input.workspace)?
            .render_pinned_rule_instructions(&input.session_id)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(
            &input.session_id,
            &serde_json::json!({"render": render}),
        )
    }

    #[tool(
        name = "session_workflow_add_node",
        description = "Add a node to the durable session workflow graph. If node_id \
                       matches an existing node, this call is a no-op and the \
                       existing node is left unchanged (no error, no duplicate)."
    )]
    pub async fn session_workflow_add_node(
        &self,
        Parameters(input): Parameters<WorkflowAddNodeInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_add_node(
                &input.session_id,
                SessionWorkflowNodeDraft {
                    node_id: input.node_id,
                    kind: parse_node_kind(&input.kind)?,
                    requirement: parse_requirement(&input.requirement)?,
                    title: input.title,
                    ticket_urn: input.ticket_urn,
                    spec_urn: input.spec_urn,
                    anchor_urn: input.anchor_urn,
                    category: input.category,
                    cached_ticket_title: input.cached_ticket_title,
                    validation_spec_id: input.validation_spec_id,
                },
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_add_nodes",
        description = "Atomically add workflow nodes; errors identify nodes[index]. \
                       Any node whose node_id matches an existing node is a \
                       no-op (left unchanged, not duplicated); use \
                       session_workflow_update_node to change an existing node."
    )]
    pub async fn session_workflow_add_nodes(
        &self,
        Parameters(input): Parameters<WorkflowAddNodesInput>,
    ) -> Result<CallToolResult, McpError> {
        let drafts = input
            .nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| {
                Ok(SessionWorkflowNodeDraft {
                    node_id: node.node_id,
                    kind: parse_node_kind(&node.kind).map_err(|error| {
                        indexed_mcp_error("nodes", index, error)
                    })?,
                    requirement: parse_requirement(&node.requirement).map_err(
                        |error| indexed_mcp_error("nodes", index, error),
                    )?,
                    title: node.title,
                    ticket_urn: node.ticket_urn,
                    spec_urn: node.spec_urn,
                    anchor_urn: node.anchor_urn,
                    category: node.category,
                    cached_ticket_title: node.cached_ticket_title,
                    validation_spec_id: node.validation_spec_id,
                })
            })
            .collect::<Result<Vec<_>, McpError>>()?;
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_add_nodes(&input.session_id, drafts)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_add_edge",
        description = "Add a directed edge between workflow nodes."
    )]
    pub async fn session_workflow_add_edge(
        &self,
        Parameters(input): Parameters<WorkflowAddEdgeInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_add_edge(
                &input.session_id,
                &input.from,
                &input.to,
                parse_edge_kind(&input.kind)?,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_add_edges",
        description = "Atomically add workflow edges; errors identify edges[index]."
    )]
    pub async fn session_workflow_add_edges(
        &self,
        Parameters(input): Parameters<WorkflowAddEdgesInput>,
    ) -> Result<CallToolResult, McpError> {
        let edges = input
            .edges
            .into_iter()
            .enumerate()
            .map(|(index, edge)| {
                Ok(SessionWorkflowEdge {
                    from: edge.from,
                    to: edge.to,
                    kind: parse_edge_kind(&edge.kind).map_err(|error| {
                        indexed_mcp_error("edges", index, error)
                    })?,
                })
            })
            .collect::<Result<Vec<_>, McpError>>()?;
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_add_edges(&input.session_id, edges)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_set_status",
        description = "Update workflow node status and optional deferred reason."
    )]
    pub async fn session_workflow_set_status(
        &self,
        Parameters(input): Parameters<WorkflowSetStatusInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_update_node_status(
                &input.session_id,
                &input.node_id,
                parse_node_status(&input.status)?,
                input.deferred_reason,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_update_node",
        description = "Patch fields on an existing workflow node in place (repair surface for a \
                       wedged node, e.g. a validation node missing validation_spec_id). Every \
                       field is optional: omit a field to leave it unchanged, set it to \
                       overwrite. The merged node is re-validated with the same rules enforced \
                       at node creation, so a patch cannot introduce a new wedge."
    )]
    pub async fn session_workflow_update_node(
        &self,
        Parameters(input): Parameters<WorkflowUpdateNodeInput>,
    ) -> Result<CallToolResult, McpError> {
        let kind = input.kind.as_deref().map(parse_node_kind).transpose()?;
        let requirement = input
            .requirement
            .as_deref()
            .map(parse_requirement)
            .transpose()?;
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_update_node(
                &input.session_id,
                &input.node_id,
                SessionWorkflowNodePatch {
                    kind,
                    requirement,
                    title: input.title,
                    ticket_urn: input.ticket_urn,
                    spec_urn: input.spec_urn,
                    anchor_urn: input.anchor_urn,
                    category: input.category,
                    cached_ticket_title: input.cached_ticket_title,
                    validation_spec_id: input.validation_spec_id,
                },
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_remove_node",
        description = "Delete a workflow node and any edges that reference it. Repair surface for \
                       a node that cannot be fixed in place (or should never have been added), \
                       instead of permanently blocking session_finish/session_handoff."
    )]
    pub async fn session_workflow_remove_node(
        &self,
        Parameters(input): Parameters<WorkflowRemoveNodeInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_remove_node(&input.session_id, &input.node_id)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_promote",
        description = "Promote a workflow node to a ticket-backed node while preserving identity."
    )]
    pub async fn session_workflow_promote(
        &self,
        Parameters(input): Parameters<WorkflowPromoteInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_promote_node_to_ticket(
                &input.session_id,
                &input.node_id,
                &input.ticket_urn,
                input.cached_ticket_title,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_render_terminal",
        description = "Render the workflow graph as deterministic terminal text."
    )]
    pub async fn session_workflow_render_terminal(
        &self,
        Parameters(input): Parameters<WorkflowRenderInput>,
    ) -> Result<CallToolResult, McpError> {
        let render = self
            .config_for_workspace(&input.workspace)?
            .workflow_render_terminal(&input.session_id, None)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(
            &input.session_id,
            &serde_json::json!({"render": render}),
        )
    }

    #[tool(
        name = "session_workflow_render_mermaid",
        description = "Render the workflow graph as deterministic Mermaid flowchart text."
    )]
    pub async fn session_workflow_render_mermaid(
        &self,
        Parameters(input): Parameters<WorkflowRenderInput>,
    ) -> Result<CallToolResult, McpError> {
        let render = self
            .config_for_workspace(&input.workspace)?
            .workflow_render_mermaid(&input.session_id, None)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(
            &input.session_id,
            &serde_json::json!({"render": render}),
        )
    }

    #[tool(
        name = "session_handoff",
        description = "Persist structured handoff record before rendering handoff summary."
    )]
    pub async fn session_handoff(
        &self,
        Parameters(input): Parameters<RuntimeHandoffInput>,
    ) -> Result<CallToolResult, McpError> {
        let package = if !input.objective.is_empty()
            || !input.target_tickets.is_empty()
            || !input.target_files.is_empty()
            || !input.decisions.is_empty()
            || !input.non_goals.is_empty()
            || !input.context_anchors.is_empty()
            || !input.open_escalations.is_empty()
            || !input.higher_level_objective.is_empty()
            || !input.upward_context.is_empty()
            || input.risk_notes.is_some()
            || input.predecessor_handoff.is_some()
        {
            Some(SessionHandoffPackage {
                objective: input.objective,
                target_tickets: input
                    .target_tickets
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                higher_level_objective: input.higher_level_objective,
                upward_context: input
                    .upward_context
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                target_files: input.target_files,
                decisions: input.decisions,
                non_goals: input.non_goals,
                context_anchors: input.context_anchors,
                open_escalations: input.open_escalations,
                risk_notes: input.risk_notes,
                predecessor_handoff: input.predecessor_handoff,
            })
        } else {
            None
        };
        let result = self
            .config_for_workspace(&input.workspace)?
            .create_handoff_result(
                &input.session_id,
                package,
                input.validation.into_iter().map(Into::into).collect(),
                None,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &result)
    }

    #[tool(
        name = "session_terminal_create",
        description = "Create a session-scoped human-owned terminal observer. This tool never starts a command or sends terminal input."
    )]
    pub async fn session_terminal_create(
        &self,
        Parameters(input): Parameters<TerminalCreateInput>,
    ) -> Result<CallToolResult, McpError> {
        let manifest = self
            .config_for_workspace(&input.workspace)?
            .create_terminal_observer(SessionTerminalCreateRequest {
                session_id: input.session_id.clone(),
                label: input.label,
                cwd: input.cwd,
            })
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &manifest)
    }

    #[tool(
        name = "session_terminal_status",
        description = "Read a human-owned observer terminal status. No terminal input or command execution is available."
    )]
    pub async fn session_terminal_status(
        &self,
        Parameters(input): Parameters<TerminalStatusInput>,
    ) -> Result<CallToolResult, McpError> {
        let manifest = self
            .config_for_workspace(&input.workspace)?
            .terminal_status(&input.session_id, &input.terminal_id)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &manifest)
    }

    #[tool(
        name = "session_terminal_peek",
        description = "Read a bounded window of human-owned observer terminal output. No terminal input or command execution is available."
    )]
    pub async fn session_terminal_peek(
        &self,
        Parameters(input): Parameters<TerminalPeekInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .config_for_workspace(&input.workspace)?
            .peek_terminal_output(
                &input.session_id,
                &input.terminal_id,
                input.offset.unwrap_or(0),
                input.limit.unwrap_or(50),
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &result)
    }

    #[tool(
        name = "session_terminal_close",
        description = "Close a human-owned observer terminal record so later output cannot be appended."
    )]
    pub async fn session_terminal_close(
        &self,
        Parameters(input): Parameters<TerminalStatusInput>,
    ) -> Result<CallToolResult, McpError> {
        let manifest = self
            .config_for_workspace(&input.workspace)?
            .close_terminal_observer(&input.session_id, &input.terminal_id)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &manifest)
    }

    #[tool(
        name = "session_finish",
        description = "Explicitly finish workflow, enforcing required node and validation gates."
    )]
    pub async fn session_finish(
        &self,
        Parameters(input): Parameters<RuntimeFinishInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .config_for_workspace(&input.workspace)?
            .finish_workflow(
                &input.session_id,
                input.validation.into_iter().map(Into::into).collect(),
                input.deferred_optional_node_ids,
                None,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &result)
    }

    #[tool(
        name = "session_check_in",
        description = "Check a session into its authoritative worktree assignment and return the resolved receipt."
    )]
    pub async fn session_check_in(
        &self,
        Parameters(input): Parameters<CheckInInput>,
    ) -> Result<CallToolResult, McpError> {
        let receipt = self
            .config_for_workspace(&input.workspace)?
            .check_in_worktree(SessionWorktreeCheckInRequest {
                session_id: input.session_id,
                owner_id: input.owner_id,
                ticket_id: input.ticket_id,
                worktree_path: PathBuf::from(input.worktree_path),
                branch: input.branch,
                predecessor_session_id: input.predecessor_session_id,
            })
            .map_err(Self::session_err)?;
        Self::json_result(&receipt)
    }

    #[tool(
        name = "session_lookup",
        description = "Look up the authoritative worktree assignment for a session."
    )]
    pub async fn session_lookup(
        &self,
        Parameters(input): Parameters<LookupInput>,
    ) -> Result<CallToolResult, McpError> {
        let receipt = self
            .config()
            .lookup_worktree(&input.session_id)
            .map_err(Self::session_err)?;
        Self::json_result(&receipt)
    }

    #[tool(
        name = "session_query",
        description = "Query stored sessions with optional id-prefix, conversation, agent, text, and limit filters."
    )]
    pub async fn session_query(
        &self,
        Parameters(input): Parameters<QueryInput>,
    ) -> Result<CallToolResult, McpError> {
        let query = SessionQuery {
            session_id_prefix: input.session_id_prefix,
            conversation_id: input.conversation_id,
            agent_id: input.agent_id,
            text: input.text,
            limit: input.limit,
        };
        let sessions = self
            .config()
            .query_sessions(&query)
            .map_err(Self::session_err)?;
        Self::json_result(&serde_json::json!({
            "count": sessions.len(),
            "sessions": sessions,
        }))
    }

    #[tool(
        name = "session_sessions_for_ticket",
        description = "Query sessions related to ticket_id by ticket-relation strength tier: strict, linked, or mentioned (widening)."
    )]
    pub async fn session_sessions_for_ticket(
        &self,
        Parameters(input): Parameters<SessionsForTicketInput>,
    ) -> Result<CallToolResult, McpError> {
        let strength = match input.strength.as_str() {
            "strict" => RelationStrength::Strict,
            "linked" => RelationStrength::Linked,
            "mentioned" => RelationStrength::Mentioned,
            other => {
                return Err(McpError::invalid_params(
                    format!(
                        "invalid relation strength: {other}. allowed values: \
                         strict, linked, mentioned"
                    ),
                    None,
                ));
            },
        };
        let sessions = self
            .config()
            .sessions_for_ticket(&input.ticket_id, strength)
            .map_err(Self::session_err)?;
        Self::json_result(&serde_json::json!({
            "count": sessions.len(),
            "sessions": sessions,
        }))
    }

    #[tool(
        name = "session_peek_range",
        description = "Peek a bounded window of transcript turns for a session."
    )]
    pub async fn session_peek_range(
        &self,
        Parameters(input): Parameters<PeekRangeInput>,
    ) -> Result<CallToolResult, McpError> {
        let range = self
            .config()
            .peek_range(&input.session_id, input.start, input.end)
            .map_err(Self::session_err)?;
        Self::json_result(&range)
    }

    #[tool(
        name = "session_peek_skeleton",
        description = "Peek a body-stripped skeleton overview of a session transcript."
    )]
    pub async fn session_peek_skeleton(
        &self,
        Parameters(input): Parameters<PeekSkeletonInput>,
    ) -> Result<CallToolResult, McpError> {
        let preview_chars = input
            .preview_chars
            .unwrap_or(DEFAULT_SKELETON_PREVIEW_CHARS);
        let skeleton = self
            .config()
            .peek_skeleton(&input.session_id, preview_chars)
            .map_err(Self::session_err)?;
        Self::json_result(&skeleton)
    }

    #[tool(
        name = "session_tool_metrics",
        description = "Compute and report tool metrics for the workspace store with optional window filtering."
    )]
    pub async fn session_tool_metrics(
        &self,
        Parameters(input): Parameters<ToolMetricsInput>,
    ) -> Result<CallToolResult, McpError> {
        let window = ToolMetricsWindow {
            max_age_days: input.days,
            max_sessions: input.max_sessions,
        };
        let report = self
            .config_for_workspace(&input.workspace)?
            .tool_metrics(window)
            .map_err(Self::session_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "session_subagent_rollups",
        description = "Compute and report per-sub-agent cost and usage rollups for a Copilot session UUID."
    )]
    pub async fn session_subagent_rollups(
        &self,
        Parameters(input): Parameters<SubagentRollupsInput>,
    ) -> Result<CallToolResult, McpError> {
        let rollups = self
            .config_for_workspace(&input.workspace)?
            .subagent_rollups(&input.session_id)
            .map_err(Self::session_err)?;
        Self::json_result(&rollups)
    }

    #[tool(
        name = "session_grant_create",
        description = "Create a new budget-offset grant for the graded cost gate."
    )]
    pub async fn session_grant_create(
        &self,
        Parameters(input): Parameters<GrantCreateInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::{
            BudgetGrantScope,
            create_grant,
        };

        let scope = match input.scope.to_lowercase().as_str() {
            "session" => BudgetGrantScope::Session,
            "subagent" => BudgetGrantScope::Subagent,
            _ => {
                return Err(McpError::invalid_params(
                    format!(
                        "invalid scope: {}. allowed values: session, subagent",
                        input.scope
                    ),
                    None,
                ));
            },
        };

        let grant = create_grant(
            &self.config_for_workspace(&input.workspace)?,
            scope,
            input.offset,
            input.model,
            input.ttl_seconds,
        )
        .map_err(Self::session_err)?;

        Self::json_result(&grant)
    }

    #[tool(
        name = "session_grant_list",
        description = "List all budget-offset grants in the store."
    )]
    pub async fn session_grant_list(
        &self,
        Parameters(input): Parameters<GrantListInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::list_grants;

        let grants = list_grants(&self.config_for_workspace(&input.workspace)?)
            .map_err(Self::session_err)?;

        Self::json_result(&grants)
    }

    #[tool(
        name = "session_grant_revoke",
        description = "Revoke (delete) a budget-offset grant by its ID."
    )]
    pub async fn session_grant_revoke(
        &self,
        Parameters(input): Parameters<GrantRevokeInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::revoke_grant;

        let revoked = revoke_grant(
            &self.config_for_workspace(&input.workspace)?,
            &input.grant_id,
        )
        .map_err(Self::session_err)?;

        Self::json_result(&serde_json::json!({
            "revoked": revoked,
            "grant_id": input.grant_id,
        }))
    }

    #[tool(
        name = "session_escalation_create",
        description = "Create a new escalation record for upward problem delegation."
    )]
    pub async fn session_escalation_create(
        &self,
        Parameters(input): Parameters<EscalationCreateInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::{
            create_escalation,
            escalation_marker,
        };

        let escalation = create_escalation(
            &self.config_for_workspace(&input.workspace)?,
            input.blocking_decision,
            input.context,
            input.requested_capability,
            input.options_considered,
            input.session_id,
            input.from_model,
        )
        .map_err(Self::session_err)?;

        // Include the marker in the response
        let mut result = serde_json::to_value(&escalation).map_err(|e| {
            McpError::internal_error(format!("serialization: {e}"), None)
        })?;

        if let Some(obj) = result.as_object_mut() {
            obj.insert(
                "marker".to_string(),
                serde_json::Value::String(escalation_marker(
                    &escalation.escalation_id,
                )),
            );
        }

        Self::json_result(&result)
    }

    #[tool(
        name = "session_escalation_list",
        description = "List escalations in the store, optionally filtered by status."
    )]
    pub async fn session_escalation_list(
        &self,
        Parameters(input): Parameters<EscalationListInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::{
            EscalationStatus,
            list_escalations,
        };

        let status_filter = if let Some(status_str) = input.status {
            match status_str.to_lowercase().as_str() {
                "open" => Some(EscalationStatus::Open),
                "resolved" => Some(EscalationStatus::Resolved),
                _ => {
                    return Err(McpError::invalid_params(
                        format!(
                            "invalid status: {}. allowed values: open, resolved",
                            status_str
                        ),
                        None,
                    ));
                },
            }
        } else {
            None
        };

        let escalations = list_escalations(
            &self.config_for_workspace(&input.workspace)?,
            status_filter,
        )
        .map_err(Self::session_err)?;

        Self::json_result(&escalations)
    }

    #[tool(
        name = "session_escalation_get",
        description = "Get a single escalation by ID."
    )]
    pub async fn session_escalation_get(
        &self,
        Parameters(input): Parameters<EscalationGetInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::get_escalation;

        let escalation = get_escalation(
            &self.config_for_workspace(&input.workspace)?,
            &input.escalation_id,
        )
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("escalation not found: {}", input.escalation_id),
                None,
            )
        })?;

        Self::json_result(&escalation)
    }

    #[tool(
        name = "session_escalation_resolve",
        description = "Resolve an escalation with a resolution action and details."
    )]
    pub async fn session_escalation_resolve(
        &self,
        Parameters(input): Parameters<EscalationResolveInput>,
    ) -> Result<CallToolResult, McpError> {
        use chrono::Utc;
        use session_api::{
            EscalationAction,
            EscalationResolution,
            resolve_escalation,
        };

        let action = match input.action.to_lowercase().as_str() {
            "handled" => EscalationAction::Handled,
            "granted-offset" => EscalationAction::GrantedOffset,
            "escalated-to-user" => EscalationAction::EscalatedToUser,
            "spawned-session" => EscalationAction::SpawnedSession,
            _ => {
                return Err(McpError::invalid_params(
                    format!(
                        "invalid action: {}. allowed values: handled, granted-offset, escalated-to-user, spawned-session",
                        input.action
                    ),
                    None,
                ));
            },
        };

        let resolution = EscalationResolution {
            action,
            note: input.note,
            offset_grant_id: input.grant_id,
            spawned_session_id: input.spawned_session_id,
            resolved_at: Utc::now(),
        };

        let escalation = resolve_escalation(
            &self.config_for_workspace(&input.workspace)?,
            &input.escalation_id,
            resolution,
        )
        .map_err(Self::session_err)?;

        Self::json_result(&escalation)
    }

    #[tool(
        name = "session_move_preflight",
        description = "Read-only preflight plan for moving a session to another workspace store."
    )]
    pub async fn session_move_preflight(
        &self,
        Parameters(input): Parameters<SessionMoveInput>,
    ) -> Result<CallToolResult, McpError> {
        let session_id = input.id.parse::<Uuid>().map_err(|error| {
            McpError::invalid_params(
                format!("invalid session UUID: {error}"),
                None,
            )
        })?;
        let target_workspace_root = workspace::canonicalize_workspace_root_strict(
            std::path::Path::new(&input.to_workspace_root),
        )
        .map_err(|error| {
            McpError::invalid_params(
                format!(
                    "workspace root canonicalization failed for '{}': {error}",
                    input.to_workspace_root
                ),
                None,
            )
        })?;
        let report = self
            .config()
            .plan_move_preflight(&session_id, &target_workspace_root)
            .map_err(Self::session_err)?;

        Self::json_result(&serde_json::json!({
            "command": "move",
            "status": if report.supported() { "ok" } else { "blocked" },
            "mode": "preflight",
            "id": session_id,
            "plan": Self::move_plan_json(&report)?,
            "recovery": {"resume": "session move --resume <journal-uuid>", "rollback": "session move --rollback <journal-uuid>"},
        }))
    }

    #[tool(
        name = "session_move_apply",
        description = "Execute a supported session move to another workspace store."
    )]
    pub async fn session_move_apply(
        &self,
        Parameters(input): Parameters<SessionMoveInput>,
    ) -> Result<CallToolResult, McpError> {
        let session_id = input.id.parse::<Uuid>().map_err(|error| {
            McpError::invalid_params(
                format!("invalid session UUID: {error}"),
                None,
            )
        })?;
        let target_workspace_root = workspace::canonicalize_workspace_root_strict(
            std::path::Path::new(&input.to_workspace_root),
        )
        .map_err(|error| {
            McpError::invalid_params(
                format!(
                    "workspace root canonicalization failed for '{}': {error}",
                    input.to_workspace_root
                ),
                None,
            )
        })?;
        let report = self
            .config()
            .plan_move_preflight(&session_id, &target_workspace_root)
            .map_err(Self::session_err)?;
        if !report.supported() {
            return Err(McpError::invalid_params(
                "move preflight blocked; run session_move_preflight for details".to_string(),
                None,
            ));
        }
        let outcome = self
            .config()
            .execute_move_with_journal(&report)
            .map_err(Self::session_err)?;

        Self::json_result(&serde_json::json!({
            "command": "move",
            "status": "ok",
            "mode": "apply",
            "id": session_id,
            "plan": Self::move_plan_json(&report)?,
            "outcome": Self::move_outcome_json(&outcome)?,
            "recovery": {"resume": "session move --resume <journal-uuid>", "rollback": "session move --rollback <journal-uuid>"},
        }))
    }

    #[tool(
        name = "session_move_resume",
        description = "Resume an interrupted session move from a journal id."
    )]
    pub async fn session_move_resume(
        &self,
        Parameters(input): Parameters<SessionMoveJournalInput>,
    ) -> Result<CallToolResult, McpError> {
        let journal = input.id.parse::<Uuid>().map_err(|error| {
            McpError::invalid_params(
                format!("invalid journal id: {error}"),
                None,
            )
        })?;
        let outcome = self
            .config()
            .resume_move_with_journal(journal)
            .map_err(Self::session_err)?;

        Self::json_result(&serde_json::json!({
            "command": "move",
            "status": "ok",
            "mode": "resume",
            "journal_id": outcome.journal.id,
            "phase": outcome.journal.phase,
            "recovery": {"resume": "session move --resume <journal-uuid>", "rollback": "session move --rollback <journal-uuid>"},
        }))
    }

    #[tool(
        name = "session_move_rollback",
        description = "Roll back a session move from a journal id."
    )]
    pub async fn session_move_rollback(
        &self,
        Parameters(input): Parameters<SessionMoveJournalInput>,
    ) -> Result<CallToolResult, McpError> {
        let journal = input.id.parse::<Uuid>().map_err(|error| {
            McpError::invalid_params(
                format!("invalid journal id: {error}"),
                None,
            )
        })?;
        let outcome = self
            .config()
            .rollback_move_with_journal(journal)
            .map_err(Self::session_err)?;

        Self::json_result(&serde_json::json!({
            "command": "move",
            "status": "ok",
            "mode": "rollback",
            "journal_id": outcome.journal.id,
            "phase": outcome.journal.phase,
            "recovery": {"resume": "session move --resume <journal-uuid>", "rollback": "session move --rollback <journal-uuid>"},
        }))
    }
}

// ── MCP handler trait ─────────────────────────────────────────────────────────

#[tool_handler]
impl ServerHandler for SessionServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "session-mcp provides direct access to the session store. Use named tools for session worktree check-in, lookup, query, move, and transcript peeking. Call session_capabilities to discover the durable-workflow lifecycle (runtime_init -> pin/view -> workflow_* -> render_* -> handoff/finish) and the legal workflow enum values."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// ── Server startup ────────────────────────────────────────────────────────────

pub async fn run_mcp_server(
    store_root: PathBuf,
    workspace_slug: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = SessionServer::new(store_root, workspace_slug);

    tracing::info!(
        "Starting session-mcp server on stdio (direct store access)"
    );

    let service = server.serve(stdio()).await.inspect_err(|err| {
        eprintln!("Server error: {err:?}");
    })?;

    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::Value;
    use session_api::{
        CopilotHookMessage,
        CopilotHookPayload,
        SessionCaptureRequest,
        SessionError,
        SessionRole,
        SessionStoreConfig,
    };
    use std::process::Command;
    use tempfile::tempdir;

    use super::*;

    fn seed(
        config: &SessionStoreConfig,
        session_id: &str,
        agent: &str,
    ) {
        let payload = CopilotHookPayload {
            session_id: session_id.to_string(),
            workspace_slug: "default".to_string(),
            captured_at: Utc::now(),
            conversation_id: None,
            agent_id: Some(agent.to_string()),
            model: None,
            trigger: None,
            provisioning: None,
            messages: vec![CopilotHookMessage {
                role: SessionRole::User,
                content: "alpha body\nbeta".to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: None,
            }],
            events: vec![],
            runtime: None,
        };
        config
            .persist_capture(SessionCaptureRequest::copilot(payload))
            .expect("seed");
    }

    fn extract_json(result: rmcp::model::CallToolResult) -> Value {
        let text = result
            .content
            .iter()
            .find_map(|content| {
                if let rmcp::model::RawContent::Text(text) = &content.raw {
                    Some(text.text.clone())
                } else {
                    None
                }
            })
            .expect("text content");
        serde_json::from_str(&text).expect("parse json")
    }

    #[tokio::test]
    async fn check_in_then_lookup() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join(".session");
        let worktree = dir.path().join("wt");
        let server =
            SessionServer::new(store_root.clone(), "default".to_string());

        let receipt = server
            .session_check_in(Parameters(CheckInInput {
                workspace: store_root.display().to_string(),
                session_id: "11111111-1111-4111-8111-111111111111".to_string(),
                owner_id: "agent".to_string(),
                ticket_id: "t1".to_string(),
                worktree_path: worktree.to_string_lossy().to_string(),
                branch: "feature/x".to_string(),
                predecessor_session_id: None,
            }))
            .await
            .expect("check-in");
        assert!(!receipt.is_error.unwrap_or(false));

        let lookup = server
            .session_lookup(Parameters(LookupInput {
                session_id: "11111111-1111-4111-8111-111111111111".to_string(),
            }))
            .await
            .expect("lookup");
        assert!(!lookup.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn terminal_observer_tools_read_output_without_input_operation() {
        let dir = tempdir().unwrap();
        let session_root = dir.path().join(".session");
        let workspace = session_root.display().to_string();
        let session_id = "88888888-8888-4888-8888-888888888888";
        let server = SessionServer::new(session_root.clone(), "default".to_string());
        let config = SessionStoreConfig::new(session_root, "default");
        config
            .init_runtime_context(SessionRuntimeInitRequest {
                session_id: Some(session_id.to_string()),
                predecessor_run_id: None,
                force_new_run: false,
            })
            .unwrap();

        let created = server
            .session_terminal_create(Parameters(TerminalCreateInput {
                workspace: workspace.clone(),
                session_id: session_id.to_string(),
                label: "human terminal".to_string(),
                cwd: Some(dir.path().to_path_buf()),
            }))
            .await
            .expect("create observer");
        let created_payload = extract_json(created);
        let terminal_id = created_payload["terminal_id"]
            .as_str()
            .expect("terminal id")
            .to_string();
        config
            .append_terminal_output(session_id, &terminal_id, "human output\n".to_string())
            .unwrap();

        let peek = server
            .session_terminal_peek(Parameters(TerminalPeekInput {
                workspace: workspace.clone(),
                session_id: session_id.to_string(),
                terminal_id: terminal_id.clone(),
                offset: None,
                limit: None,
            }))
            .await
            .expect("peek observer");
        let peek_payload = extract_json(peek);
        assert_eq!(peek_payload["events"][0]["output"], "human output\n");

        let closed = server
            .session_terminal_close(Parameters(TerminalStatusInput {
                workspace,
                session_id: session_id.to_string(),
                terminal_id,
            }))
            .await
            .expect("close observer");
        let closed_payload = extract_json(closed);
        assert_eq!(closed_payload["status"], "closed");
    }

    #[tokio::test]
    async fn handoff_validates_target_file_in_selected_worktree() {
        let dir = tempdir().unwrap();
        let main_store = dir.path().join("main/.session");
        let worktree = dir.path().join("worktree");
        let target = worktree.join("handoff-fixtures/assigned-only.txt");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "assigned worktree target").unwrap();
        let workspace = worktree.display().to_string();
        let session_id = "99999999-9999-4999-8999-999999999999";
        let server = SessionServer::new(main_store, "default".to_string());

        server
            .session_check_in(Parameters(CheckInInput {
                workspace: workspace.clone(),
                session_id: session_id.to_string(),
                owner_id: "agent".to_string(),
                ticket_id: "ticket-handoff".to_string(),
                worktree_path: worktree.display().to_string(),
                branch: "agent/handoff-worktree".to_string(),
                predecessor_session_id: None,
            }))
            .await
            .expect("check in assigned worktree");
        server
            .session_runtime_init(Parameters(RuntimeInitInput {
                workspace: workspace.clone(),
                session_id: session_id.to_string(),
                predecessor_run_id: None,
                force_new_run: false,
            }))
            .await
            .expect("initialize worktree runtime");

        let handoff = server
            .session_handoff(Parameters(RuntimeHandoffInput {
                workspace,
                session_id: session_id.to_string(),
                validation: vec![],
                objective: "Validate session worktree paths".to_string(),
                target_tickets: vec![],
                higher_level_objective: String::new(),
                upward_context: vec![],
                target_files: vec![
                    "handoff-fixtures/assigned-only.txt".to_string(),
                ],
                decisions: vec![],
                non_goals: vec![],
                context_anchors: vec![],
                open_escalations: vec!["Regression package".to_string()],
                risk_notes: None,
                predecessor_handoff: None,
            }))
            .await
            .expect("handoff path in selected worktree");
        let payload = extract_json(handoff);
        assert_eq!(
            payload["record"]["target_files"][0],
            "handoff-fixtures/assigned-only.txt"
        );
    }

    #[tokio::test]
    async fn sessions_for_ticket_returns_matches_at_requested_tier() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join(".session");
        let worktree = dir.path().join("wt");
        let server =
            SessionServer::new(store_root.clone(), "default".to_string());

        server
            .session_check_in(Parameters(CheckInInput {
                workspace: store_root.display().to_string(),
                session_id: "33333333-3333-4333-8333-333333333333".to_string(),
                owner_id: "agent-ticket".to_string(),
                ticket_id: "ticket-mcp".to_string(),
                worktree_path: worktree.to_string_lossy().to_string(),
                branch: "feature/ticket-mcp".to_string(),
                predecessor_session_id: None,
            }))
            .await
            .expect("check-in");

        let result = server
            .session_sessions_for_ticket(Parameters(SessionsForTicketInput {
                ticket_id: "ticket-mcp".to_string(),
                strength: "strict".to_string(),
            }))
            .await
            .expect("sessions-for-ticket");
        assert!(!result.is_error.unwrap_or(false));
        let payload = extract_json(result);
        assert_eq!(payload["count"], 1);
        assert_eq!(
            payload["sessions"][0]["session_id"],
            "33333333-3333-4333-8333-333333333333"
        );
        assert_eq!(payload["sessions"][0]["matched_strength"], "strict");

        let unrelated = server
            .session_sessions_for_ticket(Parameters(SessionsForTicketInput {
                ticket_id: "ticket-other".to_string(),
                strength: "mentioned".to_string(),
            }))
            .await
            .expect("sessions-for-ticket unrelated");
        let unrelated_payload = extract_json(unrelated);
        assert_eq!(unrelated_payload["count"], 0);
    }

    #[tokio::test]
    async fn runtime_render_instructions_returns_only_pinned_rules() {
        let dir = tempdir().unwrap();
        let session_root = dir.path().join(".session");
        let config = SessionStoreConfig::new(&session_root, "default");
        let init = config
            .init_runtime_context(SessionRuntimeInitRequest {
                session_id: Some(
                    "11111111-1111-4111-8111-111111111111".to_string(),
                ),
                predecessor_run_id: None,
                force_new_run: false,
            })
            .expect("init runtime");
        let mut rule_store =
            rule_api::RuleStore::open_or_init(&dir.path().join(".rule"))
                .expect("rule store");
        let rule = rule_api::RuleManifest::new(
            "session/mcp/render",
            "MCP render",
            ".instructions",
            "mcp-render",
            "Pinned MCP guidance.",
        );
        let rule_id = rule_store.create(&rule, None).expect("create rule");
        config
            .pin_runtime_entity(
                &init.context.session_id,
                &format!("ce://default/rules/{rule_id}"),
                None,
                None,
            )
            .expect("pin rule");
        let server = SessionServer::new(session_root.clone(), "default".into());

        let result = server
            .session_runtime_render_instructions(Parameters(RuntimeViewInput {
                workspace: session_root.display().to_string(),
                session_id: init.context.session_id,
            }))
            .await
            .expect("render instructions");
        let payload = extract_json(result);
        assert!(
            payload["render"]
                .as_str()
                .unwrap()
                .contains("Pinned MCP guidance.")
        );
    }

    #[tokio::test]
    async fn query_and_peek() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join(".session");
        let server =
            SessionServer::new(store_root.clone(), "default".to_string());
        let config = SessionStoreConfig::new(store_root, "default".to_string());
        seed(&config, "22222222-2222-4222-8222-222222222222", "agent-2");

        let query = server
            .session_query(Parameters(QueryInput {
                session_id_prefix: None,
                conversation_id: None,
                agent_id: Some("agent-2".to_string()),
                text: None,
                limit: None,
            }))
            .await
            .expect("query");
        assert!(!query.is_error.unwrap_or(false));

        let skeleton = server
            .session_peek_skeleton(Parameters(PeekSkeletonInput {
                session_id: "22222222-2222-4222-8222-222222222222".to_string(),
                preview_chars: None,
            }))
            .await
            .expect("skeleton");
        assert!(!skeleton.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn move_preflight_and_apply_roundtrip() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        Command::new("git")
            .current_dir(&repo_root)
            .args(["init"])
            .status()
            .expect("git init")
            .success()
            .then_some(())
            .expect("git init failed");

        let source_store_root = repo_root.join(".session");
        std::fs::create_dir_all(&source_store_root).unwrap();
        let target_workspace_root = repo_root.join("target-workspace");
        std::fs::create_dir_all(target_workspace_root.join(".session"))
            .unwrap();

        let session_id = "7b3a7c62-1f3f-45d6-b8a1-f2b83e3d9f71";
        let config = SessionStoreConfig::new(
            source_store_root.clone(),
            "default".to_string(),
        );
        seed(&config, session_id, "agent-3");

        let server = SessionServer::new(
            source_store_root.clone(),
            "default".to_string(),
        );
        let preflight = server
            .session_move_preflight(Parameters(SessionMoveInput {
                id: session_id.to_string(),
                to_workspace_root: target_workspace_root
                    .to_string_lossy()
                    .to_string(),
            }))
            .await
            .expect("move preflight");
        let preflight_json = extract_json(preflight);
        assert_eq!(preflight_json["status"], "ok");
        assert_eq!(preflight_json["mode"], "preflight");
        assert!(preflight_json["plan"]["supported"].as_bool().unwrap());

        let apply = server
            .session_move_apply(Parameters(SessionMoveInput {
                id: session_id.to_string(),
                to_workspace_root: target_workspace_root
                    .to_string_lossy()
                    .to_string(),
            }))
            .await
            .expect("move apply");
        let apply_json = extract_json(apply);
        assert_eq!(apply_json["status"], "ok");
        assert_eq!(apply_json["mode"], "apply");
        assert!(apply_json["outcome"]["journal"]["id"].is_string());

        let target_config = SessionStoreConfig::new(
            target_workspace_root.join(".session"),
            "default".to_string(),
        );
        assert!(matches!(
            config.read_session(session_id),
            Err(SessionError::NotFound { .. })
        ));
        assert_eq!(
            target_config.read_session(session_id).unwrap().session_id,
            session_id
        );
    }

    // ── T-SCHEMA: workflow mutation schemas advertise legal enum values ──────

    #[test]
    fn workflow_add_node_schema_advertises_kind_and_requirement_enums() {
        let schema = rmcp::schemars::schema_for!(WorkflowAddNodeInput);
        let json = serde_json::to_string(&schema).expect("schema json");
        for value in ["ticket", "validation", "spec", "task"] {
            assert!(
                json.contains(&format!("\"{value}\"")),
                "schema must advertise node kind `{value}`: {json}"
            );
        }
        for value in ["required", "optional"] {
            assert!(
                json.contains(&format!("\"{value}\"")),
                "schema must advertise requirement `{value}`"
            );
        }
    }

    #[test]
    fn workflow_edge_and_status_schemas_advertise_enums() {
        let edge = serde_json::to_string(&rmcp::schemars::schema_for!(
            WorkflowAddEdgeInput
        ))
        .unwrap();
        assert!(edge.contains("\"depends-on\"") && edge.contains("\"order\""));

        let status = serde_json::to_string(&rmcp::schemars::schema_for!(
            WorkflowSetStatusInput
        ))
        .unwrap();
        for value in ["pending", "in-progress", "blocked", "done", "deferred"] {
            assert!(
                status.contains(&format!("\"{value}\"")),
                "status schema must advertise `{value}`"
            );
        }
    }

    /// The advertised schema enums must match the `session-api` enums exactly.
    #[test]
    fn workflow_enum_parity_with_session_api() {
        // Behavioral + descriptive node kinds serialize to the advertised set.
        let kinds = [
            (SessionWorkflowNodeKind::Ticket, "ticket"),
            (SessionWorkflowNodeKind::Validation, "validation"),
            (SessionWorkflowNodeKind::Spec, "spec"),
            (SessionWorkflowNodeKind::Task, "task"),
        ];
        for (kind, label) in kinds {
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{label}\"")
            );
            assert_eq!(parse_node_kind(label).unwrap(), kind);
        }
        // Legacy aliases round-trip into the generic Task bucket.
        for alias in ["action", "decision", "checkpoint"] {
            assert_eq!(
                parse_node_kind(alias).unwrap(),
                SessionWorkflowNodeKind::Task
            );
        }
    }

    // ── T-ERRCONTRACT: rejections enumerate the allowed values ───────────────

    #[test]
    fn invalid_workflow_values_report_allowed_set() {
        let kind_err = parse_node_kind("tickett").unwrap_err();
        assert!(kind_err.message.contains("ticket"));
        assert!(kind_err.message.contains("validation"));
        assert!(kind_err.message.contains("spec"));
        assert!(kind_err.message.contains("task"));
        assert!(
            kind_err
                .message
                .contains("kind=task with category=\"<your-label>\"")
        );

        let edge_err = parse_edge_kind("nope").unwrap_err();
        assert!(edge_err.message.contains("depends-on"));
        assert!(edge_err.message.contains("order"));
        assert!(edge_err.message.contains("did you mean kind=depends-on?"));

        let status_err = parse_node_status("doing").unwrap_err();
        assert!(status_err.message.contains("in-progress"));
        assert!(status_err.message.contains("deferred"));
        assert!(
            status_err
                .message
                .contains("did you mean status=in-progress?")
        );

        let req_err = parse_requirement("maybe").unwrap_err();
        assert!(req_err.message.contains("required"));
        assert!(req_err.message.contains("optional"));
        assert!(
            req_err
                .message
                .contains("did you mean requirement=required?")
        );
    }

    // ── T-HANDLE: init/resume + workflow tools echo session_id ──────

    #[tokio::test]
    async fn runtime_init_result_exposes_session_id_top_line() {
        let dir = tempdir().unwrap();
        let session_root = dir.path().join(".session");
        let server =
            SessionServer::new(session_root.clone(), "default".to_string());

        let result = server
            .session_runtime_init(Parameters(RuntimeInitInput {
                workspace: session_root.display().to_string(),
                session_id: "22222222-2222-4222-8222-222222222222".to_string(),
                predecessor_run_id: None,
                force_new_run: false,
            }))
            .await
            .expect("runtime init");
        let payload = extract_json(result);
        let handle =
            payload["session_id"].as_str().expect("top-line session_id");
        assert!(!handle.is_empty());
        // The handle matches the nested context handle (no drift).
        assert_eq!(payload["context"]["session_id"].as_str(), Some(handle));

        // A subsequent workflow call echoes the same handle in its result.
        let node = server
            .session_workflow_add_node(Parameters(WorkflowAddNodeInput {
                workspace: session_root.display().to_string(),
                session_id: handle.to_string(),
                node_id: Some("n1".to_string()),
                kind: "task".to_string(),
                requirement: "optional".to_string(),
                title: "descriptive".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: Some("note".to_string()),
                cached_ticket_title: None,
                validation_spec_id: None,
            }))
            .await
            .expect("add node");
        let node_payload = extract_json(node);
        assert_eq!(node_payload["session_id"].as_str(), Some(handle));
    }

    #[tokio::test]
    async fn workflow_batch_tools_are_atomic_and_report_array_index() {
        let dir = tempdir().unwrap();
        let session_root = dir.path().join(".session");
        let workspace = session_root.display().to_string();
        let server =
            SessionServer::new(session_root.clone(), "default".to_string());
        let config = server.config_for_workspace(&workspace).unwrap();
        let init = config
            .init_runtime_context(SessionRuntimeInitRequest {
                session_id: Some(
                    "33333333-3333-4333-8333-333333333333".to_string(),
                ),
                predecessor_run_id: None,
                force_new_run: false,
            })
            .unwrap();
        let session_id = init.context.session_id;
        let node = |node_id: &str, kind: &str| WorkflowNodeDraftInput {
            node_id: Some(node_id.to_string()),
            kind: kind.to_string(),
            requirement: "optional".to_string(),
            title: node_id.to_string(),
            ticket_urn: None,
            spec_urn: None,
            anchor_urn: None,
            category: None,
            cached_ticket_title: None,
            validation_spec_id: None,
        };

        let node_error = server
            .session_workflow_add_nodes(Parameters(WorkflowAddNodesInput {
                workspace: workspace.clone(),
                session_id: session_id.clone(),
                nodes: vec![node("a", "task"), node("bad", "review-criterion")],
            }))
            .await
            .unwrap_err();
        assert!(node_error.message.contains("nodes[1]"));
        assert!(
            node_error
                .message
                .contains("kind=task with category=\"<your-label>\"")
        );
        assert!(
            config
                .read_runtime_context(&session_id)
                .unwrap()
                .workflow
                .nodes
                .is_empty()
        );

        let node_result = server
            .session_workflow_add_nodes(Parameters(WorkflowAddNodesInput {
                workspace: workspace.clone(),
                session_id: session_id.clone(),
                nodes: vec![node("a", "task"), node("b", "task")],
            }))
            .await
            .unwrap();
        assert_eq!(
            extract_json(node_result)["workflow"]["nodes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let edge = |from: &str, to: &str, kind: &str| WorkflowEdgeDraftInput {
            from: from.to_string(),
            to: to.to_string(),
            kind: kind.to_string(),
        };
        let edge_error = server
            .session_workflow_add_edges(Parameters(WorkflowAddEdgesInput {
                workspace: workspace.clone(),
                session_id: session_id.clone(),
                edges: vec![
                    edge("a", "b", "depends-on"),
                    edge("b", "a", "related-to"),
                ],
            }))
            .await
            .unwrap_err();
        assert!(edge_error.message.contains("edges[1]"));
        assert!(edge_error.message.contains("did you mean kind=depends-on?"));
        assert!(
            config
                .read_runtime_context(&session_id)
                .unwrap()
                .workflow
                .edges
                .is_empty()
        );

        let edge_result = server
            .session_workflow_add_edges(Parameters(WorkflowAddEdgesInput {
                workspace,
                session_id,
                edges: vec![
                    edge("a", "b", "depends-on"),
                    edge("b", "a", "order"),
                ],
            }))
            .await
            .unwrap();
        assert_eq!(
            extract_json(edge_result)["workflow"]["edges"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    // ── T-CATALOG: session lifecycle + enums are discoverable ────────────────

    #[tokio::test]
    async fn capabilities_lists_session_lifecycle_and_enums() {
        let dir = tempdir().unwrap();
        let session_root = dir.path().join(".session");
        let server =
            SessionServer::new(session_root.clone(), "default".to_string());

        let result = server
            .session_capabilities(Parameters(CapabilitiesInput::default()))
            .await
            .expect("capabilities");
        let catalog = extract_json(result);

        assert_eq!(catalog["surface"], "session-mcp");
        assert_eq!(catalog["handle"]["field"], "session_id");

        let steps = catalog["lifecycle"]["steps"]
            .as_array()
            .expect("lifecycle steps");
        let tools: Vec<&str> =
            steps.iter().filter_map(|s| s["tool"].as_str()).collect();
        for expected in [
            "session_runtime_init",
            "session_workflow_add_node",
            "session_workflow_render_terminal",
            "session_finish",
        ] {
            assert!(
                tools.contains(&expected),
                "catalog lifecycle must list {expected}"
            );
        }

        let behavioral = catalog["enums"]["workflow_node_kind"]["behavioral"]
            .as_array()
            .expect("behavioral kinds");
        let behavioral: Vec<&str> =
            behavioral.iter().filter_map(|v| v.as_str()).collect();
        assert!(behavioral.contains(&"ticket"));
        assert!(behavioral.contains(&"validation"));
        assert!(behavioral.contains(&"spec"));
    }

    #[test]
    fn workspace_validation_rejects_ambient_aliases() {
        for value in [None, Some(""), Some("default"), Some("."), Some("..")] {
            let err = workspace::validate_explicit_workspace_selector(value)
                .expect_err("should reject ambient selector");
            let err_msg = err.to_string();
            assert!(
                err_msg.contains("invalid workspace selector"),
                "error should mention 'invalid workspace selector': {err_msg}"
            );
            assert!(
                err_msg.contains(
                    "entity creation requires an explicit workspace path"
                ),
                "error should state the requirement: {err_msg}"
            );
        }
    }
}
