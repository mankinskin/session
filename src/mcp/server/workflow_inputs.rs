#[allow(unused_imports)]
use super::*;

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
pub(super) enum WorkflowNodeKindSchema {
    Ticket,
    Validation,
    Spec,
    Task,
}

/// Legal `session_workflow_add_node.requirement` values.
#[derive(JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
pub(super) enum WorkflowRequirementSchema {
    Required,
    Optional,
}

/// Legal `session_workflow_add_edge.kind` values.
#[derive(JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
pub(super) enum WorkflowEdgeKindSchema {
    DependsOn,
    Order,
}

/// Legal `session_workflow_set_status.status` values.
#[derive(JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
pub(super) enum WorkflowNodeStatusSchema {
    Pending,
    InProgress,
    Blocked,
    Done,
    Deferred,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowAddNodeInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
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
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
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
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
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
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    pub session_id: String,
    pub edges: Vec<WorkflowEdgeDraftInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowSetStatusInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
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
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    pub session_id: String,
    pub node_id: String,
    pub ticket_urn: String,
    #[serde(default)]
    pub cached_ticket_title: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowUpdateNodeInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
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
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    pub session_id: String,
    pub node_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowRenderInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    pub session_id: String,
}
