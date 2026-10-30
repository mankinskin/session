#[allow(unused_imports)]
use super::*;

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Copilot session UUID. Required; the CLI does not resolve one implicitly.
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub predecessor_run_id: Option<String>,
    #[arg(long, default_value_t = false)]
    pub force_new_run: bool,
}

#[derive(Debug, Args)]
pub struct ResumeArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub predecessor_run_id: String,
}

#[derive(Debug, Args)]
pub struct PinArgs {
    #[arg(long)]
    pub session_id: String,
    /// Canonical entity URN in `ce://<workspace>/<store>/<entity>` format (e.g. `ce://default/ticket/<id>`, `ce://default/dossier/<slug>`).
    #[arg(long)]
    pub entity_urn: String,
    #[arg(long)]
    pub relation: Option<String>,
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Debug, Args)]
pub struct UnpinArgs {
    #[arg(long)]
    pub session_id: String,
    /// Canonical entity URN in `ce://<workspace>/<store>/<entity>` format to unpin.
    #[arg(long)]
    pub entity_urn: String,
}

#[derive(Debug, Args)]
pub struct ViewArgs {
    #[arg(long)]
    pub session_id: String,
}

#[derive(Debug, Args)]
pub struct WorkflowAddNodeArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub node_id: Option<String>,
    /// Behavioral node kind: ticket, validation, spec, task (deprecated
    /// aliases mapped to task: action, decision, checkpoint).
    #[arg(long)]
    pub kind: String,
    /// Whether the node gates finish: required, optional.
    #[arg(long)]
    pub requirement: String,
    #[arg(long)]
    pub title: String,
    #[arg(long)]
    pub ticket_urn: Option<String>,
    /// Spec URN for a `spec` behavioral node (mirror of --ticket-urn).
    #[arg(long)]
    pub spec_urn: Option<String>,
    /// Optional non-gating ticket or spec reference for any node kind.
    #[arg(long)]
    pub anchor_urn: Option<String>,
    /// Open, free-text descriptive category. No gating logic branches on it.
    /// To model a would-be custom kind, keep --kind task and set this label,
    /// e.g. --kind task --category <your-label> (such as
    /// --kind task --category review-criterion).
    #[arg(long)]
    pub category: Option<String>,
    #[arg(long)]
    pub cached_ticket_title: Option<String>,
    #[arg(long)]
    pub validation_spec_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct WorkflowAddEdgeArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub from: String,
    #[arg(long)]
    pub to: String,
    #[arg(long)]
    pub kind: String,
}

#[derive(Debug, Args)]
pub struct WorkflowAddNodesArgs {
    #[arg(long)]
    pub session_id: String,
    /// JSON array of node drafts.
    #[arg(long, value_name = "JSON")]
    pub nodes_json: String,
}

#[derive(Debug, Args)]
pub struct WorkflowAddEdgesArgs {
    #[arg(long)]
    pub session_id: String,
    /// JSON array of edge drafts.
    #[arg(long, value_name = "JSON")]
    pub edges_json: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct WorkflowNodeDraftJson {
    #[serde(default)]
    pub(super) node_id: Option<String>,
    pub(super) kind: String,
    pub(super) requirement: String,
    pub(super) title: String,
    #[serde(default)]
    pub(super) ticket_urn: Option<String>,
    #[serde(default)]
    pub(super) spec_urn: Option<String>,
    #[serde(default)]
    pub(super) anchor_urn: Option<String>,
    #[serde(default)]
    pub(super) category: Option<String>,
    #[serde(default)]
    pub(super) cached_ticket_title: Option<String>,
    #[serde(default)]
    pub(super) validation_spec_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WorkflowEdgeDraftJson {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) kind: String,
}

#[derive(Debug, Args)]
pub struct WorkflowSetStatusArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub node_id: String,
    #[arg(long)]
    pub status: String,
    #[arg(long)]
    pub deferred_reason: Option<String>,
}

#[derive(Debug, Args)]
pub struct WorkflowPromoteArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub node_id: String,
    #[arg(long)]
    pub ticket_urn: String,
    #[arg(long)]
    pub cached_ticket_title: Option<String>,
}

#[derive(Debug, Args)]
pub struct HandoffArgs {
    #[arg(long)]
    pub session_id: String,
    /// The single goal of the next implementation unit.
    #[arg(long)]
    pub objective: Option<String>,
    /// A target ticket id or JSON object with `id`, `why`, and optional cached fields.
    #[arg(long = "target-ticket")]
    pub target_tickets: Vec<String>,
    /// Why the next implementation unit matters to the broader program.
    #[arg(long)]
    pub higher_level_objective: Option<String>,
    /// JSON ancestor entry with `entity_urn`, `title`, and role `epic`, `phase`, or `parent`.
    #[arg(long = "upward-context")]
    pub upward_context: Vec<String>,
    /// Workspace-relative file expected to be touched; repeat for multiple files.
    #[arg(long = "target-file")]
    pub target_files: Vec<String>,
    /// Resolved design choice; repeat for multiple decisions.
    #[arg(long = "decision")]
    pub decisions: Vec<String>,
    /// Explicit out-of-scope boundary; repeat for multiple non-goals.
    #[arg(long = "non-goal")]
    pub non_goals: Vec<String>,
    /// Prior finding or id needed for the next implementation unit; repeat as needed.
    #[arg(long = "context-anchor")]
    pub context_anchors: Vec<String>,
    /// Open escalation; repeat as needed.
    #[arg(long = "open-escalation")]
    pub open_escalations: Vec<String>,
    /// Known risk or fragile area.
    #[arg(long)]
    pub risk_notes: Option<String>,
    /// Id of the handoff this record supersedes.
    #[arg(long)]
    pub predecessor_handoff: Option<String>,
    /// JSON array of validation gates.
    #[arg(long)]
    pub validation_json: Option<String>,
}

#[derive(Debug, Args)]
pub struct FinishArgs {
    #[arg(long)]
    pub session_id: String,
    /// JSON array of validation gates.
    #[arg(long)]
    pub validation_json: Option<String>,
    /// Optional-node ids explicitly deferred.
    #[arg(long = "defer-node")]
    pub deferred_optional_node_ids: Vec<String>,
}
