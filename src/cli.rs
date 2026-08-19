use std::path::PathBuf;

use clap::{
    Args,
    Parser,
    Subcommand,
};
use serde::Deserialize;
use serde_json::{
    Value,
    json,
};
use uuid::Uuid;

use memory_kernel::workspace;
use session_api::{
    DEFAULT_PROMPT_SUMMARIZE_THRESHOLD_CHARS,
    DEFAULT_SKELETON_PREVIEW_CHARS,
    PromptPackOptions,
    RelationStrength,
    SessionAuditSelector,
    SessionError,
    SessionHandoffPackage,
    SessionHandoffTargetTicket,
    SessionHandoffUpwardContextEntry,
    SessionQuery,
    SessionRuntimeInitRequest,
    SessionStoreConfig,
    SessionTerminalCreateRequest,
    SessionValidationGate,
    SessionWorkflowEdge,
    SessionWorkflowEdgeKind,
    SessionWorkflowNodeDraft,
    SessionWorkflowNodeKind,
    SessionWorkflowNodeRequirement,
    SessionWorkflowNodeStatus,
    SessionWorktreeCheckInRequest,
    ToolMetricsWindow,
};

const SESSION_STORE_DIR: &str = ".session";

// ── CLI root ───────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
#[command(
    name = "session",
    about = "Session system CLI (worktree check-in, lookup, query, transcript peeking)",
    version,
    arg_required_else_help = true
)]
pub struct SessionCli {
    /// Return machine-readable JSON output.
    #[arg(long, global = true, conflicts_with = "toon")]
    pub json: bool,

    /// Return machine-readable TOON output.
    #[arg(long, global = true, conflicts_with = "json")]
    pub toon: bool,

    /// Explicit session store root (the `.session` directory).
    #[arg(long, global = true)]
    pub store_root: Option<PathBuf>,

    /// Workspace/repo root to normalize to the canonical `.session` store.
    /// Lets a tool run from an ancestor checkout target a nested workspace.
    #[arg(long = "workspace", alias = "workspace-root", global = true)]
    pub workspace_root: Option<PathBuf>,

    /// Workspace slug that scopes session storage.
    #[arg(long, global = true, default_value = "default")]
    pub workspace_slug: String,

    #[command(subcommand)]
    pub command: SessionCommand,
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// Initialize or resume durable runtime workspace context.
    Init(InitArgs),
    /// Resume an existing workspace with a new linked run id.
    Resume(ResumeArgs),
    /// Pin an entity URN into runtime context.
    Pin(PinArgs),
    /// Unpin an entity URN from runtime context.
    Unpin(UnpinArgs),
    /// Read headers-only runtime context view.
    View(ViewArgs),
    /// Render the focused instruction set from pinned rule URNs.
    RenderInstructions(ViewArgs),
    /// Canonical nested workflow commands (`session workflow <subcommand>`).
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    /// Add a workflow node.
    WorkflowAddNode(WorkflowAddNodeArgs),
    /// Atomically add workflow nodes from a JSON array.
    WorkflowAddNodes(WorkflowAddNodesArgs),
    /// Link two workflow nodes.
    WorkflowAddEdge(WorkflowAddEdgeArgs),
    /// Atomically add workflow edges from a JSON array.
    WorkflowAddEdges(WorkflowAddEdgesArgs),
    /// Update workflow node status.
    WorkflowSetStatus(WorkflowSetStatusArgs),
    /// Promote a workflow node to a ticket-backed node.
    WorkflowPromote(WorkflowPromoteArgs),
    /// Render workflow as terminal output.
    WorkflowRenderTerminal(ViewArgs),
    /// Render workflow as Mermaid flowchart output.
    WorkflowRenderMermaid(ViewArgs),
    /// Persist a structured handoff record and render handoff summary.
    Handoff(HandoffArgs),
    /// Explicitly finish a workflow with required gates.
    Finish(FinishArgs),
    /// Check a session into its authoritative worktree assignment.
    CheckIn(CheckInArgs),
    /// Look up the worktree assignment for a session.
    Lookup(LookupArgs),
    /// Query stored sessions with optional filters.
    Query(QueryArgs),
    /// Query sessions related to a ticket at a selectable relation-strength tier.
    SessionsForTicket(SessionsForTicketArgs),
    /// Backfill ticket linkage for historical sessions from structured
    /// signals only (branch shape, worktree_path shape, handoff
    /// target_tickets). Defaults to a dry run; pass `--write` to persist.
    BackfillTicketLinks(BackfillTicketLinksArgs),
    /// Move a UUID-addressed session to another workspace store.
    Move(MoveArgs),
    /// Peek a bounded window of transcript turns for a session.
    PeekRange(PeekRangeArgs),
    /// Peek a body-stripped skeleton of a session transcript.
    PeekSkeleton(PeekSkeletonArgs),
    /// Peek a prompt-facing compact view of a session transcript.
    PeekPromptPack(PeekPromptPackArgs),
    /// Create a human-owned observer terminal record.
    TerminalCreate(TerminalCreateArgs),
    /// Append process output captured by a human-owned terminal UI.
    TerminalAppendOutput(TerminalAppendOutputArgs),
    /// Read an observer terminal status.
    TerminalStatus(TerminalStatusArgs),
    /// Read a bounded window of observer terminal output.
    TerminalPeek(TerminalPeekArgs),
    /// Close an observer terminal record.
    TerminalClose(TerminalStatusArgs),
    /// Compute and report tool metrics for this store.
    ToolMetrics(ToolMetricsArgs),
    /// Compute and report per-sub-agent cost and usage rollups for a workspace session.
    SubagentRollups(SubagentRollupsArgs),
    /// Report per-dispatch tool failures and delegation-quality findings.
    DelegationCost(DelegationCostArgs),
    /// Budget-offset grant management (`session grant <subcommand>`).
    Grant {
        #[command(subcommand)]
        command: GrantCommand,
    },
    /// Upward escalation workflow (`session escalation <subcommand>`).
    Escalation {
        #[command(subcommand)]
        command: EscalationCommand,
    },
}

/// Canonical nested workflow subcommands. These mirror the flat
/// `workflow-*` variants, which are retained as compatibility aliases.
#[derive(Debug, Subcommand)]
pub enum WorkflowCommand {
    /// Add a workflow node.
    AddNode(WorkflowAddNodeArgs),
    /// Atomically add workflow nodes from a JSON array.
    AddNodes(WorkflowAddNodesArgs),
    /// Link two workflow nodes.
    AddEdge(WorkflowAddEdgeArgs),
    /// Atomically add workflow edges from a JSON array.
    AddEdges(WorkflowAddEdgesArgs),
    /// Update workflow node status.
    SetStatus(WorkflowSetStatusArgs),
    /// Promote a workflow node to a ticket-backed node.
    Promote(WorkflowPromoteArgs),
    /// Render workflow as terminal output.
    RenderTerminal(ViewArgs),
    /// Render workflow as Mermaid flowchart output.
    RenderMermaid(ViewArgs),
}

/// Budget-offset grant subcommands.
#[derive(Debug, Subcommand)]
pub enum GrantCommand {
    /// Create a new budget-offset grant.
    Create(GrantCreateArgs),
    /// List all grants in the store.
    List,
    /// Revoke (delete) a grant by its ID.
    Revoke(GrantRevokeArgs),
}

/// Upward escalation workflow subcommands.
#[derive(Debug, Subcommand)]
pub enum EscalationCommand {
    /// Create a new escalation record.
    Create(EscalationCreateArgs),
    /// List escalations in the store.
    List(EscalationListArgs),
    /// Get a single escalation by ID.
    Get(EscalationGetArgs),
    /// Resolve an escalation.
    Resolve(EscalationResolveArgs),
}

#[derive(Debug, Args)]
pub struct CheckInArgs {
    /// Session id to check in.
    #[arg(long)]
    pub session_id: String,
    /// Owner (agent) identity claiming the worktree.
    #[arg(long)]
    pub owner_id: String,
    /// Ticket the session is working on.
    #[arg(long)]
    pub ticket_id: String,
    /// Assigned worktree working directory.
    #[arg(long)]
    pub worktree_path: PathBuf,
    /// Branch checked out in the worktree.
    #[arg(long)]
    pub branch: String,
    /// Predecessor session id when rotating from a prior assignment.
    #[arg(long)]
    pub predecessor_session_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct LookupArgs {
    /// Session id to look up.
    #[arg(long)]
    pub session_id: String,
}

#[derive(Debug, Args)]
pub struct QueryArgs {
    /// Filter by session id prefix.
    #[arg(long)]
    pub session_id_prefix: Option<String>,
    /// Filter by conversation id.
    #[arg(long)]
    pub conversation_id: Option<String>,
    /// Filter by agent id.
    #[arg(long)]
    pub agent_id: Option<String>,
    /// Free-text filter across session content.
    #[arg(long)]
    pub text: Option<String>,
    /// Maximum number of sessions to return.
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Args)]
pub struct SessionsForTicketArgs {
    /// Ticket id to find related sessions for.
    pub ticket_id: String,
    /// Relation-strength tier: strict, linked, or mentioned.
    #[arg(long, default_value = "strict")]
    pub strength: String,
}

#[derive(Debug, Args)]
pub struct BackfillTicketLinksArgs {
    /// Persist the computed linkage. Without this flag the command only
    /// reports what would change; no session file is touched.
    #[arg(long)]
    pub write: bool,
}

#[derive(Debug, Args)]
pub struct PeekRangeArgs {
    /// Session id to peek.
    #[arg(long)]
    pub session_id: String,
    /// Inclusive start turn index (0-based).
    #[arg(long, default_value_t = 0)]
    pub start: usize,
    /// Exclusive end turn index (0-based). Defaults to the end of the transcript.
    #[arg(long)]
    pub end: Option<usize>,
}

#[derive(Debug, Args)]
pub struct PeekSkeletonArgs {
    /// Session id to peek.
    #[arg(long)]
    pub session_id: String,
    /// Maximum preview characters retained per turn.
    #[arg(long, default_value_t = DEFAULT_SKELETON_PREVIEW_CHARS)]
    pub preview_chars: usize,
}

#[derive(Debug, Args)]
pub struct PeekPromptPackArgs {
    /// Session id to peek.
    #[arg(long)]
    pub session_id: String,
    /// Maximum preview characters retained per included turn.
    #[arg(long, default_value_t = DEFAULT_SKELETON_PREVIEW_CHARS)]
    pub preview_chars: usize,
    /// Content-length threshold above which entries are summarized.
    #[arg(long, default_value_t = DEFAULT_PROMPT_SUMMARIZE_THRESHOLD_CHARS)]
    pub summarize_threshold_chars: usize,
}

#[derive(Debug, Args)]
pub struct TerminalCreateArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub label: String,
    #[arg(long)]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct TerminalAppendOutputArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub terminal_id: String,
    #[arg(long)]
    pub output: String,
}

#[derive(Debug, Args)]
pub struct TerminalStatusArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub terminal_id: String,
}

#[derive(Debug, Args)]
pub struct TerminalPeekArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub terminal_id: String,
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct ToolMetricsArgs {
    /// Maximum age in days for included sessions.
    #[arg(long)]
    pub days: Option<u32>,
    /// Maximum number of sessions to include.
    #[arg(long = "max-sessions")]
    pub max_sessions: Option<usize>,
    /// Export rollup to the specified path.
    #[arg(long)]
    pub export: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct SubagentRollupsArgs {
    /// Copilot session UUID to get rollups for.
    #[arg(long)]
    pub session_id: String,
}

#[derive(Debug, Args)]
pub struct DelegationCostArgs {
    /// Copilot session UUID to inspect.
    #[arg(long)]
    pub session_id: String,
}

#[derive(Debug, Args)]
pub struct GrantCreateArgs {
    /// Grant scope: session or subagent.
    #[arg(long)]
    pub scope: String,
    /// Budget offset to add.
    #[arg(long)]
    pub offset: u32,
    /// Optional model constraint (case-insensitive).
    #[arg(long)]
    pub model: Option<String>,
    /// Optional TTL in seconds from now.
    #[arg(long = "ttl-seconds")]
    pub ttl_seconds: Option<u64>,
    /// Optional expiration timestamp (RFC3339).
    #[arg(long = "expires-at", conflicts_with = "ttl_seconds")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Args)]
pub struct GrantRevokeArgs {
    /// Grant ID to revoke.
    pub grant_id: String,
}

#[derive(Debug, Args)]
pub struct EscalationCreateArgs {
    /// The blocking decision or problem statement.
    #[arg(long)]
    pub blocking: String,
    /// Context explaining the situation.
    #[arg(long)]
    pub context: String,
    /// Optional requested capability or resource.
    #[arg(long)]
    pub requested_capability: Option<String>,
    /// Options considered (repeatable).
    #[arg(long = "option")]
    pub options_considered: Vec<String>,
    /// Optional session ID that created the escalation.
    #[arg(long)]
    pub session_id: Option<String>,
    /// Optional model that created the escalation.
    #[arg(long)]
    pub from_model: Option<String>,
}

#[derive(Debug, Args)]
pub struct EscalationListArgs {
    /// Filter by status: open or resolved.
    #[arg(long)]
    pub status: Option<String>,
}

#[derive(Debug, Args)]
pub struct EscalationGetArgs {
    /// Escalation ID to retrieve.
    pub escalation_id: String,
}

#[derive(Debug, Args)]
pub struct EscalationResolveArgs {
    /// Escalation ID to resolve.
    pub escalation_id: String,
    /// Resolution action: handled, granted-offset, escalated-to-user, spawned-session.
    #[arg(long)]
    pub action: String,
    /// Optional note about the resolution.
    #[arg(long)]
    pub note: Option<String>,
    /// Grant ID (required when action is granted-offset).
    #[arg(long = "grant-id")]
    pub grant_id: Option<String>,
    /// Spawned session ID (required when action is spawned-session).
    #[arg(long = "spawned-session-id")]
    pub spawned_session_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct MoveArgs {
    /// Session UUID to move (required unless --resume/--rollback is used).
    pub id: Option<String>,
    /// Destination workspace root.
    #[arg(long = "to-workspace-root")]
    pub to_workspace_root: Option<PathBuf>,
    /// Plan only; do not execute the move.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
    /// Resume an interrupted move from a journal UUID.
    #[arg(long)]
    pub resume: Option<String>,
    /// Roll back a move from a journal UUID.
    #[arg(long)]
    pub rollback: Option<String>,
}

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
struct WorkflowNodeDraftJson {
    #[serde(default)]
    node_id: Option<String>,
    kind: String,
    requirement: String,
    title: String,
    #[serde(default)]
    ticket_urn: Option<String>,
    #[serde(default)]
    spec_urn: Option<String>,
    #[serde(default)]
    anchor_urn: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    cached_ticket_title: Option<String>,
    #[serde(default)]
    validation_spec_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkflowEdgeDraftJson {
    from: String,
    to: String,
    kind: String,
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

// ── output helpers ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MachineOutputFormat {
    Json,
    Toon,
}

#[derive(Debug)]
pub enum CliOutput {
    Machine(Value, MachineOutputFormat),
    Text(String),
}

#[derive(Debug, thiserror::Error)]
pub enum CliRunError {
    #[error("session error: {0}")]
    Session(#[from] SessionError),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

// ── entry point ───────────────────────────────────────────────────────────────

pub fn run(cli: SessionCli) -> Result<CliOutput, CliRunError> {
    if matches!(cli.command, SessionCommand::CheckIn(_))
        && cli.store_root.is_none()
        && cli.workspace_root.is_none()
    {
        return Err(CliRunError::BadRequest(
            "entity creation requires explicit --workspace <path> or --store-root <path>".to_string(),
        ));
    }

    let store_root = workspace::resolve_requested_store_root(
        cli.store_root.as_deref(),
        cli.workspace_root.as_deref(),
        None,
        SESSION_STORE_DIR,
    );
    let config =
        SessionStoreConfig::new(store_root, cli.workspace_slug.clone());

    let payload = dispatch(&config, cli.command)?;

    match machine_output_format(cli.json, cli.toon) {
        Some(format) => Ok(CliOutput::Machine(payload, format)),
        None => Ok(CliOutput::Text(render_human(&payload))),
    }
}

fn dispatch(
    config: &SessionStoreConfig,
    command: SessionCommand,
) -> Result<Value, CliRunError> {
    match command {
        SessionCommand::Init(args) => {
            let result =
                config.init_runtime_context(SessionRuntimeInitRequest {
                    session_id: Some(args.session_id),
                    predecessor_run_id: args.predecessor_run_id,
                    force_new_run: args.force_new_run,
                })?;
            let handle = result.context.session_id.clone();
            to_value_with_handle(&handle, &result)
        },
        SessionCommand::Resume(args) => {
            let result = config.resume_workspace_context(
                &args.session_id,
                &args.predecessor_run_id,
            )?;
            let handle = result.context.session_id.clone();
            to_value_with_handle(&handle, &result)
        },
        SessionCommand::Pin(args) => {
            let context = config.pin_runtime_entity(
                &args.session_id,
                &args.entity_urn,
                args.relation,
                args.reason,
            )?;
            to_value(&context)
        },
        SessionCommand::Unpin(args) => {
            let context = config
                .unpin_runtime_entity(&args.session_id, &args.entity_urn)?;
            to_value(&context)
        },
        SessionCommand::View(args) => {
            let view = config.view_runtime_context(&args.session_id)?;
            to_value(&view)
        },
        SessionCommand::RenderInstructions(args) => {
            let render =
                config.render_pinned_rule_instructions(&args.session_id)?;
            to_value(&json!({"render": render}))
        },
        SessionCommand::Workflow { command } =>
            handle_workflow_command(&config, command),
        SessionCommand::WorkflowAddNode(args) =>
            handle_workflow_command(&config, WorkflowCommand::AddNode(args)),
        SessionCommand::WorkflowAddNodes(args) =>
            handle_workflow_command(&config, WorkflowCommand::AddNodes(args)),
        SessionCommand::WorkflowAddEdge(args) =>
            handle_workflow_command(&config, WorkflowCommand::AddEdge(args)),
        SessionCommand::WorkflowAddEdges(args) =>
            handle_workflow_command(&config, WorkflowCommand::AddEdges(args)),
        SessionCommand::WorkflowSetStatus(args) =>
            handle_workflow_command(&config, WorkflowCommand::SetStatus(args)),
        SessionCommand::WorkflowPromote(args) =>
            handle_workflow_command(&config, WorkflowCommand::Promote(args)),
        SessionCommand::WorkflowRenderTerminal(args) =>
            handle_workflow_command(
                &config,
                WorkflowCommand::RenderTerminal(args),
            ),
        SessionCommand::WorkflowRenderMermaid(args) => handle_workflow_command(
            &config,
            WorkflowCommand::RenderMermaid(args),
        ),
        SessionCommand::Handoff(args) => {
            let package = handoff_package_from_args(&args)?;
            let result = config.create_handoff_result(
                &args.session_id,
                package,
                parse_validation_gates(args.validation_json.as_deref())?,
                None,
            )?;
            to_value(&result)
        },
        SessionCommand::Finish(args) => {
            let result = config.finish_workflow(
                &args.session_id,
                parse_validation_gates(args.validation_json.as_deref())?,
                args.deferred_optional_node_ids,
                None,
            )?;
            to_value(&result)
        },
        SessionCommand::CheckIn(args) => {
            let receipt =
                config.check_in_worktree(SessionWorktreeCheckInRequest {
                    session_id: args.session_id,
                    owner_id: args.owner_id,
                    ticket_id: args.ticket_id,
                    worktree_path: args.worktree_path,
                    branch: args.branch,
                    predecessor_session_id: args.predecessor_session_id,
                })?;
            to_value(&receipt)
        },
        SessionCommand::Lookup(args) => {
            let receipt = config.lookup_worktree(&args.session_id)?;
            to_value(&receipt)
        },
        SessionCommand::Query(args) => {
            let query = SessionQuery {
                session_id_prefix: args.session_id_prefix,
                conversation_id: args.conversation_id,
                agent_id: args.agent_id,
                text: args.text,
                limit: args.limit,
            };
            let sessions = config.query_sessions(&query)?;
            to_value(&json!({
                "count": sessions.len(),
                "sessions": sessions,
            }))
        },
        SessionCommand::SessionsForTicket(args) => {
            let strength = parse_relation_strength(&args.strength)?;
            let sessions =
                config.sessions_for_ticket(&args.ticket_id, strength)?;
            to_value(&json!({
                "count": sessions.len(),
                "sessions": sessions,
            }))
        },
        SessionCommand::BackfillTicketLinks(args) => {
            let report = config.backfill_ticket_links(args.write)?;
            to_value(&report)
        },
        SessionCommand::Move(args) => move_command(config, args),
        SessionCommand::PeekRange(args) => {
            let range =
                config.peek_range(&args.session_id, args.start, args.end)?;
            to_value(&range)
        },
        SessionCommand::PeekSkeleton(args) => {
            let skeleton =
                config.peek_skeleton(&args.session_id, args.preview_chars)?;
            to_value(&skeleton)
        },
        SessionCommand::PeekPromptPack(args) => {
            let pack = config.peek_prompt_pack(
                &args.session_id,
                PromptPackOptions {
                    preview_chars: args.preview_chars,
                    summarize_threshold_chars: args.summarize_threshold_chars,
                },
            )?;
            to_value(&pack)
        },
        SessionCommand::TerminalCreate(args) => {
            let manifest = config.create_terminal_observer(
                SessionTerminalCreateRequest {
                    session_id: args.session_id,
                    label: args.label,
                    cwd: args.cwd,
                },
            )?;
            to_value(&manifest)
        },
        SessionCommand::TerminalAppendOutput(args) => {
            let event = config.append_terminal_output(
                &args.session_id,
                &args.terminal_id,
                args.output,
            )?;
            to_value(&event)
        },
        SessionCommand::TerminalStatus(args) => {
            let manifest =
                config.terminal_status(&args.session_id, &args.terminal_id)?;
            to_value(&manifest)
        },
        SessionCommand::TerminalPeek(args) => {
            let result = config.peek_terminal_output(
                &args.session_id,
                &args.terminal_id,
                args.offset,
                args.limit,
            )?;
            to_value(&result)
        },
        SessionCommand::TerminalClose(args) => {
            let manifest = config
                .close_terminal_observer(&args.session_id, &args.terminal_id)?;
            to_value(&manifest)
        },
        SessionCommand::ToolMetrics(args) => {
            let window = ToolMetricsWindow {
                max_age_days: args.days,
                max_sessions: args.max_sessions,
            };
            let report = config.tool_metrics(window)?;

            // If export path specified, write rollup
            if let Some(export_path) = args.export {
                use session_api::tool_metrics::write_rollup;
                write_rollup(&export_path, report.clone())?;
            }

            to_value(&report)
        },
        SessionCommand::SubagentRollups(args) => {
            let rollups = config.subagent_rollups(&args.session_id)?;
            to_value(&rollups)
        },
        SessionCommand::DelegationCost(args) => {
            let report = config.delegation_cost_report(
                SessionAuditSelector::SessionId(args.session_id),
            )?;
            to_value(&report)
        },
        SessionCommand::Grant { command } =>
            handle_grant_command(config, command),
        SessionCommand::Escalation { command } =>
            handle_escalation_command(config, command),
    }
}

/// Shared handler for canonical nested (`session workflow <subcommand>`) and
/// flat (`session workflow-<subcommand>`) command forms.
fn handle_workflow_command(
    config: &SessionStoreConfig,
    command: WorkflowCommand,
) -> Result<Value, CliRunError> {
    match command {
        WorkflowCommand::AddNode(args) => {
            let context = config.workflow_add_node(
                &args.session_id,
                SessionWorkflowNodeDraft {
                    node_id: args.node_id,
                    kind: parse_node_kind(&args.kind)?,
                    requirement: parse_requirement(&args.requirement)?,
                    title: args.title,
                    ticket_urn: args.ticket_urn,
                    spec_urn: args.spec_urn,
                    anchor_urn: args.anchor_urn,
                    category: args.category,
                    cached_ticket_title: args.cached_ticket_title,
                    validation_spec_id: args.validation_spec_id,
                },
            )?;
            to_value(&context)
        },
        WorkflowCommand::AddNodes(args) => {
            let nodes = serde_json::from_str::<Vec<WorkflowNodeDraftJson>>(
                &args.nodes_json,
            )
            .map_err(|error| {
                CliRunError::BadRequest(format!(
                    "invalid --nodes-json payload: {error}"
                ))
            })?
            .into_iter()
            .enumerate()
            .map(|(index, node)| {
                Ok(SessionWorkflowNodeDraft {
                    node_id: node.node_id,
                    kind: parse_node_kind(&node.kind).map_err(|error| {
                        indexed_cli_error("nodes", index, error)
                    })?,
                    requirement: parse_requirement(&node.requirement).map_err(
                        |error| indexed_cli_error("nodes", index, error),
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
            .collect::<Result<Vec<_>, CliRunError>>()?;
            let context = config.workflow_add_nodes(&args.session_id, nodes)?;
            to_value(&context)
        },
        WorkflowCommand::AddEdge(args) => {
            let context = config.workflow_add_edge(
                &args.session_id,
                &args.from,
                &args.to,
                parse_edge_kind(&args.kind)?,
            )?;
            to_value(&context)
        },
        WorkflowCommand::AddEdges(args) => {
            let edges = serde_json::from_str::<Vec<WorkflowEdgeDraftJson>>(
                &args.edges_json,
            )
            .map_err(|error| {
                CliRunError::BadRequest(format!(
                    "invalid --edges-json payload: {error}"
                ))
            })?
            .into_iter()
            .enumerate()
            .map(|(index, edge)| {
                Ok(SessionWorkflowEdge {
                    from: edge.from,
                    to: edge.to,
                    kind: parse_edge_kind(&edge.kind).map_err(|error| {
                        indexed_cli_error("edges", index, error)
                    })?,
                })
            })
            .collect::<Result<Vec<_>, CliRunError>>()?;
            let context = config.workflow_add_edges(&args.session_id, edges)?;
            to_value(&context)
        },
        WorkflowCommand::SetStatus(args) => {
            let context = config.workflow_update_node_status(
                &args.session_id,
                &args.node_id,
                parse_node_status(&args.status)?,
                args.deferred_reason,
            )?;
            to_value(&context)
        },
        WorkflowCommand::Promote(args) => {
            let context = config.workflow_promote_node_to_ticket(
                &args.session_id,
                &args.node_id,
                &args.ticket_urn,
                args.cached_ticket_title,
            )?;
            to_value(&context)
        },
        WorkflowCommand::RenderTerminal(args) => {
            let rendered =
                config.workflow_render_terminal(&args.session_id, None)?;
            to_value(&json!({"render": rendered}))
        },
        WorkflowCommand::RenderMermaid(args) => {
            let rendered =
                config.workflow_render_mermaid(&args.session_id, None)?;
            to_value(&json!({"render": rendered}))
        },
    }
}

/// Handler for grant subcommands.
fn handle_grant_command(
    config: &SessionStoreConfig,
    command: GrantCommand,
) -> Result<Value, CliRunError> {
    use chrono::{
        DateTime,
        Utc,
    };
    use session_api::{
        BudgetGrantScope,
        create_grant,
        list_grants,
        revoke_grant,
    };

    match command {
        GrantCommand::Create(args) => {
            let scope = match args.scope.to_lowercase().as_str() {
                "session" => BudgetGrantScope::Session,
                "subagent" => BudgetGrantScope::Subagent,
                _ =>
                    return Err(CliRunError::BadRequest(format!(
                        "invalid scope: {}. allowed values: session, subagent",
                        args.scope
                    ))),
            };

            // Handle expiry: prefer TTL, fall back to explicit expires_at
            let ttl_seconds = if let Some(ttl) = args.ttl_seconds {
                Some(ttl)
            } else if let Some(expires_at) = &args.expires_at {
                // Convert RFC3339 to TTL from now
                let expires = DateTime::parse_from_rfc3339(expires_at)
                    .map_err(|e| {
                        CliRunError::BadRequest(format!(
                            "invalid expires-at timestamp: {e}"
                        ))
                    })?;
                let now = Utc::now();
                let duration = expires.signed_duration_since(now);
                if duration.num_seconds() < 0 {
                    return Err(CliRunError::BadRequest(
                        "expires-at is in the past".to_string(),
                    ));
                }
                Some(duration.num_seconds() as u64)
            } else {
                None
            };

            let grant = create_grant(
                config,
                scope,
                args.offset,
                args.model,
                ttl_seconds,
            )?;
            to_value(&grant)
        },
        GrantCommand::List => {
            let grants = list_grants(config)?;
            to_value(&grants)
        },
        GrantCommand::Revoke(args) => {
            let revoked = revoke_grant(config, &args.grant_id)?;
            to_value(&json!({
                "revoked": revoked,
                "grant_id": args.grant_id,
            }))
        },
    }
}

fn handle_escalation_command(
    config: &SessionStoreConfig,
    command: EscalationCommand,
) -> Result<Value, CliRunError> {
    use chrono::Utc;
    use session_api::{
        EscalationAction,
        EscalationResolution,
        EscalationStatus,
        create_escalation,
        escalation_marker,
        get_escalation,
        list_escalations,
        resolve_escalation,
    };

    match command {
        EscalationCommand::Create(args) => {
            let escalation = create_escalation(
                config,
                args.blocking,
                args.context,
                args.requested_capability,
                args.options_considered,
                args.session_id,
                args.from_model,
            )?;

            // Include the marker in the response
            let mut result =
                serde_json::to_value(&escalation).map_err(|e| {
                    CliRunError::Serialization(format!(
                        "serialization error: {e}"
                    ))
                })?;

            if let Some(obj) = result.as_object_mut() {
                obj.insert(
                    "marker".to_string(),
                    serde_json::Value::String(escalation_marker(
                        &escalation.escalation_id,
                    )),
                );
            }

            Ok(result)
        },
        EscalationCommand::List(args) => {
            let status_filter = if let Some(status_str) = args.status {
                match status_str.to_lowercase().as_str() {
                    "open" => Some(EscalationStatus::Open),
                    "resolved" => Some(EscalationStatus::Resolved),
                    _ =>
                        return Err(CliRunError::BadRequest(format!(
                            "invalid status: {}. allowed values: open, resolved",
                            status_str
                        ))),
                }
            } else {
                None
            };

            let escalations = list_escalations(config, status_filter)?;
            to_value(&escalations)
        },
        EscalationCommand::Get(args) => {
            let escalation = get_escalation(config, &args.escalation_id)
                .ok_or_else(|| {
                    CliRunError::BadRequest(format!(
                        "escalation not found: {}",
                        args.escalation_id
                    ))
                })?;
            to_value(&escalation)
        },
        EscalationCommand::Resolve(args) => {
            let action = match args.action.to_lowercase().as_str() {
                "handled" => EscalationAction::Handled,
                "granted-offset" => EscalationAction::GrantedOffset,
                "escalated-to-user" => EscalationAction::EscalatedToUser,
                "spawned-session" => EscalationAction::SpawnedSession,
                _ =>
                    return Err(CliRunError::BadRequest(format!(
                        "invalid action: {}. allowed values: handled, granted-offset, escalated-to-user, spawned-session",
                        args.action
                    ))),
            };

            let resolution = EscalationResolution {
                action,
                note: args.note,
                offset_grant_id: args.grant_id,
                spawned_session_id: args.spawned_session_id,
                resolved_at: Utc::now(),
            };

            let escalation =
                resolve_escalation(config, &args.escalation_id, resolution)?;
            to_value(&escalation)
        },
    }
}

fn parse_validation_gates(
    raw: Option<&str>
) -> Result<Vec<SessionValidationGate>, CliRunError> {
    let Some(raw) = raw else {
        return Ok(vec![]);
    };
    serde_json::from_str::<Vec<SessionValidationGate>>(raw).map_err(|err| {
        CliRunError::BadRequest(format!(
            "invalid --validation-json payload: {err}"
        ))
    })
}

fn handoff_package_from_args(
    args: &HandoffArgs
) -> Result<Option<SessionHandoffPackage>, CliRunError> {
    let target_tickets = args
        .target_tickets
        .iter()
        .map(|raw| {
            if raw.starts_with('{') {
                serde_json::from_str(raw).map_err(|error| {
                    CliRunError::BadRequest(format!(
                        "invalid --target-ticket JSON payload: {error}"
                    ))
                })
            } else {
                Ok(SessionHandoffTargetTicket {
                    id: raw.clone(),
                    why: String::new(),
                    state: String::new(),
                    acceptance_criteria: Vec::new(),
                })
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let upward_context = args
        .upward_context
        .iter()
        .map(|raw| {
            serde_json::from_str::<SessionHandoffUpwardContextEntry>(raw)
                .map_err(|error| {
                    CliRunError::BadRequest(format!(
                        "invalid --upward-context JSON payload: {error}"
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let objective = args.objective.clone().unwrap_or_default();
    let higher_level_objective =
        args.higher_level_objective.clone().unwrap_or_default();
    let has_package = !objective.is_empty()
        || !target_tickets.is_empty()
        || !higher_level_objective.is_empty()
        || !upward_context.is_empty()
        || !args.target_files.is_empty()
        || !args.decisions.is_empty()
        || !args.non_goals.is_empty()
        || !args.context_anchors.is_empty()
        || !args.open_escalations.is_empty()
        || args.risk_notes.is_some()
        || args.predecessor_handoff.is_some();
    Ok(has_package.then_some(SessionHandoffPackage {
        objective,
        target_tickets,
        higher_level_objective,
        upward_context,
        target_files: args.target_files.clone(),
        decisions: args.decisions.clone(),
        non_goals: args.non_goals.clone(),
        context_anchors: args.context_anchors.clone(),
        open_escalations: args.open_escalations.clone(),
        risk_notes: args.risk_notes.clone(),
        predecessor_handoff: args.predecessor_handoff.clone(),
    }))
}

fn parse_relation_strength(
    value: &str
) -> Result<RelationStrength, CliRunError> {
    match value {
        "strict" => Ok(RelationStrength::Strict),
        "linked" => Ok(RelationStrength::Linked),
        "mentioned" => Ok(RelationStrength::Mentioned),
        _ => Err(CliRunError::BadRequest(format!(
            "invalid relation strength: {value}. allowed values: \
             strict, linked, mentioned"
        ))),
    }
}

fn parse_node_kind(
    value: &str
) -> Result<SessionWorkflowNodeKind, CliRunError> {
    match value {
        "ticket" => Ok(SessionWorkflowNodeKind::Ticket),
        "validation" => Ok(SessionWorkflowNodeKind::Validation),
        "spec" => Ok(SessionWorkflowNodeKind::Spec),
        // `task` is the generic descriptive bucket; legacy cosmetic kinds are
        // accepted as back-compat aliases.
        "task" | "action" | "decision" | "checkpoint" =>
            Ok(SessionWorkflowNodeKind::Task),
        _ => Err(CliRunError::BadRequest(format!(
            "invalid workflow node kind: {value}. allowed values: \
             ticket, validation, spec, task \
             (deprecated aliases mapped to task: action, decision, checkpoint); \
             for a custom label, use kind=task with category=\"<your-label>\""
        ))),
    }
}

fn parse_requirement(
    value: &str
) -> Result<SessionWorkflowNodeRequirement, CliRunError> {
    match value {
        "required" => Ok(SessionWorkflowNodeRequirement::Required),
        "optional" => Ok(SessionWorkflowNodeRequirement::Optional),
        _ => Err(CliRunError::BadRequest(format!(
            "invalid workflow requirement: {value}. allowed values: \
             required, optional; did you mean requirement=required?"
        ))),
    }
}

fn parse_edge_kind(
    value: &str
) -> Result<SessionWorkflowEdgeKind, CliRunError> {
    match value {
        "depends-on" | "depends_on" => Ok(SessionWorkflowEdgeKind::DependsOn),
        "order" => Ok(SessionWorkflowEdgeKind::Order),
        _ => Err(CliRunError::BadRequest(format!(
            "invalid workflow edge kind: {value}. allowed values: \
             depends-on (alias depends_on), order; \
             did you mean kind=depends-on?"
        ))),
    }
}

fn parse_node_status(
    value: &str
) -> Result<SessionWorkflowNodeStatus, CliRunError> {
    match value {
        "pending" => Ok(SessionWorkflowNodeStatus::Pending),
        "in-progress" | "in_progress" =>
            Ok(SessionWorkflowNodeStatus::InProgress),
        "blocked" => Ok(SessionWorkflowNodeStatus::Blocked),
        "done" => Ok(SessionWorkflowNodeStatus::Done),
        "deferred" => Ok(SessionWorkflowNodeStatus::Deferred),
        _ => Err(CliRunError::BadRequest(format!(
            "invalid workflow status: {value}. allowed values: \
             pending, in-progress (alias in_progress), blocked, done, deferred; \
             did you mean status=in-progress?"
        ))),
    }
}

fn indexed_cli_error(
    collection: &str,
    index: usize,
    error: CliRunError,
) -> CliRunError {
    let message = match error {
        CliRunError::BadRequest(message) => message,
        other => other.to_string(),
    };
    CliRunError::BadRequest(format!("{collection}[{index}]: {message}"))
}

fn move_command(
    config: &SessionStoreConfig,
    args: MoveArgs,
) -> Result<Value, CliRunError> {
    if args.resume.is_some() && args.rollback.is_some() {
        return Err(CliRunError::BadRequest(
            "move accepts only one of --resume or --rollback".to_string(),
        ));
    }

    if let Some(journal_id) = args.resume.as_deref() {
        let journal_id = journal_id.parse::<Uuid>().map_err(|error| {
            CliRunError::BadRequest(format!(
                "invalid --resume journal UUID: {error}"
            ))
        })?;
        let outcome = config.resume_move_with_journal(journal_id)?;
        return to_value(&json!({
            "command": "move",
            "status": "ok",
            "mode": "resume",
            "journal_id": outcome.journal.id,
            "phase": outcome.journal.phase,
            "recovery": recovery_hint(),
        }));
    }

    if let Some(journal_id) = args.rollback.as_deref() {
        let journal_id = journal_id.parse::<Uuid>().map_err(|error| {
            CliRunError::BadRequest(format!(
                "invalid --rollback journal UUID: {error}"
            ))
        })?;
        let outcome = config.rollback_move_with_journal(journal_id)?;
        return to_value(&json!({
            "command": "move",
            "status": "ok",
            "mode": "rollback",
            "journal_id": outcome.journal.id,
            "phase": outcome.journal.phase,
            "recovery": recovery_hint(),
        }));
    }

    let id = args.id.as_deref().ok_or_else(|| {
        CliRunError::BadRequest(
            "move requires <id> unless --resume/--rollback is used".to_string(),
        )
    })?;
    let to_workspace_root =
        args.to_workspace_root.as_deref().ok_or_else(|| {
            CliRunError::BadRequest(
                "move requires --to-workspace-root in plan/execute mode"
                    .to_string(),
            )
        })?;

    let session_id = id.parse::<Uuid>().map_err(|error| {
        CliRunError::BadRequest(format!("invalid session UUID: {error}"))
    })?;
    let target_workspace_root =
        workspace::canonicalize_workspace_root_strict(to_workspace_root)
            .map_err(|error| {
                CliRunError::BadRequest(format!(
                    "workspace root canonicalization failed for '{}': {error}",
                    to_workspace_root.display()
                ))
            })?;
    let report =
        config.plan_move_preflight(&session_id, &target_workspace_root)?;

    if args.dry_run || !report.supported() {
        return to_value(&json!({
            "command": "move",
            "status": if report.supported() { "ok" } else { "blocked" },
            "mode": "plan",
            "dry_run": true,
            "session_id": session_id,
            "plan": move_plan_json(&report)?,
            "recovery": recovery_hint(),
        }));
    }

    let outcome = config.execute_move_with_journal(&report)?;
    to_value(&json!({
        "command": "move",
        "status": "ok",
        "mode": "execute",
        "session_id": session_id,
        "plan": move_plan_json(&report)?,
        "outcome": move_outcome_json(&outcome)?,
        "recovery": recovery_hint(),
    }))
}

fn move_plan_json(
    report: &memory_kernel::storage::move_kernel::MovePlan
) -> Result<Value, CliRunError> {
    Ok(json!({
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
            .map(|path| path_display(path))
            .collect::<Result<Vec<_>, _>>()?,
        "blockers": report.blockers,
        "captured_at": report.captured_at,
    }))
}

fn move_outcome_json(
    outcome: &memory_kernel::storage::move_kernel::MoveOutcome
) -> Result<Value, CliRunError> {
    Ok(json!({
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

fn recovery_hint() -> Value {
    json!({
        "resume": "session move --resume <journal-uuid>",
        "rollback": "session move --rollback <journal-uuid>",
    })
}

fn path_display(path: &std::path::Path) -> Result<String, CliRunError> {
    workspace::normalize_path_for_display_strict(path).map_err(|error| {
        CliRunError::BadRequest(format!(
            "path payload normalization failed for '{}': {error}",
            path.display()
        ))
    })
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, CliRunError> {
    serde_json::to_value(value)
        .map_err(|err| CliRunError::Serialization(err.to_string()))
}

/// Serialize `value` and guarantee the Copilot session UUID is present as a
/// prominent top-line `session_id` field, so every workflow call receives an
/// explicit handle from init/resume output.
fn to_value_with_handle<T: serde::Serialize>(
    session_id: &str,
    value: &T,
) -> Result<Value, CliRunError> {
    let mut payload = to_value(value)?;
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "session_id".to_string(),
            Value::String(session_id.to_string()),
        );
    }
    Ok(payload)
}

fn render_human(payload: &Value) -> String {
    serde_json::to_string_pretty(payload)
        .unwrap_or_else(|_| format!("{payload:?}"))
}

pub fn error_output(
    message: &str,
    format: Option<MachineOutputFormat>,
) -> String {
    let payload = json!({"status": "error", "message": message});
    match format {
        Some(MachineOutputFormat::Json) => payload.to_string(),
        Some(MachineOutputFormat::Toon) =>
            toon_format::encode_default(&payload).unwrap_or_else(|_| {
                format!("status: error\nmessage: {message}")
            }),
        None => message.to_string(),
    }
}

pub fn render_machine_output(
    payload: &Value,
    format: MachineOutputFormat,
) -> Result<String, String> {
    match format {
        MachineOutputFormat::Json =>
            serde_json::to_string_pretty(payload).map_err(|err| err.to_string()),
        MachineOutputFormat::Toon =>
            toon_format::encode_default(payload).map_err(|err| err.to_string()),
    }
}

pub fn machine_output_format(
    as_json: bool,
    as_toon: bool,
) -> Option<MachineOutputFormat> {
    if as_json {
        Some(MachineOutputFormat::Json)
    } else if as_toon {
        Some(MachineOutputFormat::Toon)
    } else {
        None
    }
}

pub fn requested_machine_output_format_from_args() -> Option<MachineOutputFormat>
{
    machine_output_format(
        std::env::args().any(|arg| arg == "--json"),
        std::env::args().any(|arg| arg == "--toon"),
    )
}

pub fn parse_cli_from<I, T>(args: I) -> Result<SessionCli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    SessionCli::try_parse_from(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use session_api::{
        CopilotHookMessage,
        CopilotHookPayload,
        SessionCaptureRequest,
        SessionRole,
        SessionRuntimeInitRequest,
    };
    use std::process::Command;
    use tempfile::tempdir;

    #[test]
    fn parses_check_in_command() {
        let cli = parse_cli_from([
            "session",
            "check-in",
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "--owner-id",
            "agent-1",
            "--ticket-id",
            "ticket-1",
            "--worktree-path",
            "/repo/wt",
            "--branch",
            "feature/x",
        ])
        .expect("parse check-in");

        assert_eq!(cli.workspace_slug, "default");
        match cli.command {
            SessionCommand::CheckIn(args) => {
                assert_eq!(
                    args.session_id,
                    "11111111-1111-4111-8111-111111111111"
                );
                assert_eq!(args.branch, "feature/x");
                assert!(args.predecessor_session_id.is_none());
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_canonical_nested_workflow_add_node() {
        let cli = parse_cli_from([
            "session",
            "workflow",
            "add-node",
            "--session-id",
            "77777777-7777-4777-8777-777777777777",
            "--kind",
            "action",
            "--requirement",
            "required",
            "--title",
            "do the thing",
        ])
        .expect("parse nested workflow add-node");

        match cli.command {
            SessionCommand::Workflow {
                command: WorkflowCommand::AddNode(args),
            } => {
                assert_eq!(
                    args.session_id,
                    "77777777-7777-4777-8777-777777777777"
                );
                assert_eq!(args.kind, "action");
                assert_eq!(args.requirement, "required");
                assert_eq!(args.title, "do the thing");
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn workflow_batch_commands_dispatch_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let config =
            SessionStoreConfig::new(dir.path().join(".session"), "default");
        let init = config
            .init_runtime_context(SessionRuntimeInitRequest {
                session_id: Some(
                    "11111111-1111-4111-8111-111111111111".to_string(),
                ),
                predecessor_run_id: None,
                force_new_run: false,
            })
            .unwrap();
        let session_id = init.context.session_id;

        let error = handle_workflow_command(
            &config,
            WorkflowCommand::AddNodes(WorkflowAddNodesArgs {
                session_id: session_id.clone(),
                nodes_json: r#"[
                    {"node_id":"a","kind":"task","requirement":"optional","title":"a"},
                    {"node_id":"bad","kind":"review-criterion","requirement":"optional","title":"bad"}
                ]"#
                .to_string(),
            }),
        )
        .unwrap_err();
        assert!(error.to_string().contains("nodes[1]"));
        assert!(
            config
                .read_runtime_context(&session_id)
                .unwrap()
                .workflow
                .nodes
                .is_empty()
        );

        let nodes = handle_workflow_command(
            &config,
            WorkflowCommand::AddNodes(WorkflowAddNodesArgs {
                session_id: session_id.clone(),
                nodes_json: r#"[
                    {"node_id":"a","kind":"task","requirement":"optional","title":"a"},
                    {"node_id":"b","kind":"task","requirement":"optional","title":"b"}
                ]"#
                .to_string(),
            }),
        )
        .unwrap();
        assert_eq!(nodes["workflow"]["nodes"].as_array().unwrap().len(), 2);

        let edges = handle_workflow_command(
            &config,
            WorkflowCommand::AddEdges(WorkflowAddEdgesArgs {
                session_id,
                edges_json: r#"[
                    {"from":"a","to":"b","kind":"depends-on"},
                    {"from":"b","to":"a","kind":"order"}
                ]"#
                .to_string(),
            }),
        )
        .unwrap();
        assert_eq!(edges["workflow"]["edges"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn parses_render_instructions() {
        let cli = parse_cli_from([
            "session",
            "render-instructions",
            "--session-id",
            "77777777-7777-4777-8777-777777777777",
        ])
        .expect("parse render-instructions");

        match cli.command {
            SessionCommand::RenderInstructions(args) => {
                assert_eq!(
                    args.session_id,
                    "77777777-7777-4777-8777-777777777777"
                );
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn render_instructions_dispatches_focused_rule_set() {
        let dir = tempfile::tempdir().unwrap();
        let session_root = dir.path().join(".session");
        let config = SessionStoreConfig::new(&session_root, "default");
        let init = config
            .init_runtime_context(SessionRuntimeInitRequest {
                session_id: Some(
                    "22222222-2222-4222-8222-222222222222".to_string(),
                ),
                predecessor_run_id: None,
                force_new_run: false,
            })
            .unwrap();
        let mut rule_store =
            rule_api::RuleStore::open_or_init(&dir.path().join(".rule"))
                .unwrap();
        let rule = rule_api::RuleManifest::new(
            "session/cli/render",
            "CLI render",
            ".instructions",
            "cli-render",
            "Pinned CLI guidance.",
        );
        let rule_id = rule_store.create(&rule, None).unwrap();
        config
            .pin_runtime_entity(
                &init.context.session_id,
                &format!("ce://default/rules/{rule_id}"),
                None,
                None,
            )
            .unwrap();

        let payload = dispatch(
            &config,
            SessionCommand::RenderInstructions(ViewArgs {
                session_id: init.context.session_id,
            }),
        )
        .unwrap();
        assert!(
            payload["render"]
                .as_str()
                .unwrap()
                .contains("Pinned CLI guidance.")
        );
    }

    #[test]
    fn parses_flat_workflow_add_node_alias() {
        let cli = parse_cli_from([
            "session",
            "workflow-add-node",
            "--session-id",
            "77777777-7777-4777-8777-777777777777",
            "--kind",
            "action",
            "--requirement",
            "required",
            "--title",
            "do the thing",
        ])
        .expect("parse flat workflow-add-node alias");

        match cli.command {
            SessionCommand::WorkflowAddNode(args) => {
                assert_eq!(
                    args.session_id,
                    "77777777-7777-4777-8777-777777777777"
                );
                assert_eq!(args.title, "do the thing");
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_canonical_nested_workflow_render_terminal() {
        let cli = parse_cli_from([
            "session",
            "workflow",
            "render-terminal",
            "--session-id",
            "77777777-7777-4777-8777-777777777777",
        ])
        .expect("parse nested workflow render-terminal");

        assert!(matches!(
            cli.command,
            SessionCommand::Workflow {
                command: WorkflowCommand::RenderTerminal(_),
            }
        ));
    }

    #[test]
    fn parses_peek_range_defaults() {
        let cli = parse_cli_from([
            "session",
            "peek-range",
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
        ])
        .expect("parse peek-range");

        match cli.command {
            SessionCommand::PeekRange(args) => {
                assert_eq!(args.start, 0);
                assert!(args.end.is_none());
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn json_and_toon_conflict() {
        let result = parse_cli_from([
            "session",
            "--json",
            "--toon",
            "lookup",
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_move_command() {
        let cli = parse_cli_from([
            "session",
            "move",
            "7b3a7c62-1f3f-45d6-b8a1-f2b83e3d9f71",
            "--to-workspace-root",
            "/repo/target",
        ])
        .expect("parse move");

        match cli.command {
            SessionCommand::Move(args) => {
                assert_eq!(
                    args.id.as_deref(),
                    Some("7b3a7c62-1f3f-45d6-b8a1-f2b83e3d9f71")
                );
                assert_eq!(
                    args.to_workspace_root,
                    Some(PathBuf::from("/repo/target"))
                );
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_sessions_for_ticket_command() {
        let cli = parse_cli_from([
            "session",
            "sessions-for-ticket",
            "ticket-abc",
            "--strength",
            "linked",
        ])
        .expect("parse sessions-for-ticket");

        match cli.command {
            SessionCommand::SessionsForTicket(args) => {
                assert_eq!(args.ticket_id, "ticket-abc");
                assert_eq!(args.strength, "linked");
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn move_roundtrip_executes_against_target_workspace() {
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
        let payload = CopilotHookPayload {
            session_id: session_id.to_string(),
            workspace_slug: "default".to_string(),
            captured_at: Utc::now(),
            conversation_id: Some("conv-1".to_string()),
            agent_id: Some("agent-1".to_string()),
            model: None,
            trigger: None,
            provisioning: None,
            messages: vec![CopilotHookMessage {
                role: SessionRole::User,
                content: "move me".to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: None,
            }],
            events: vec![],
            runtime: None,
        };
        config
            .persist_capture(SessionCaptureRequest::copilot(payload))
            .expect("seed session");

        let cli = parse_cli_from([
            "session",
            "--json",
            "--store-root",
            source_store_root.to_string_lossy().as_ref(),
            "move",
            session_id,
            "--to-workspace-root",
            target_workspace_root.to_string_lossy().as_ref(),
        ])
        .expect("parse move");

        match run(cli).expect("run move") {
            CliOutput::Machine(value, _) => {
                assert_eq!(value["status"], "ok");
                assert_eq!(value["mode"], "execute");
                assert!(value["outcome"]["journal"]["id"].is_string());
            },
            other => panic!("unexpected output: {other:?}"),
        }

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

    #[test]
    fn handoff_reference_completeness_includes_durable_identity_fields() {
        let temp = tempdir().unwrap();
        let config = SessionStoreConfig::new(
            temp.path().join(".session"),
            "default".to_string(),
        );

        let init = config
            .init_runtime_context(SessionRuntimeInitRequest {
                session_id: Some(
                    "33333333-3333-4333-8333-333333333333".to_string(),
                ),
                predecessor_run_id: None,
                force_new_run: false,
            })
            .expect("init runtime context");
        let session_id = init.context.session_id;

        let rendered = config
            .render_handoff_terminal(
                &session_id,
                None,
                vec![session_api::SessionValidationGate {
                    validation_spec_id: "val-handoff-reference-completeness"
                        .to_string(),
                    required: true,
                    outcome: Some("passed".to_string()),
                    command: None,
                }],
                None,
            )
            .expect("render handoff");

        assert!(rendered.contains("session_id:"));
        assert!(rendered.contains("outgoing_run_id:"));
        assert!(rendered.contains("handoff "));
        assert!(rendered.contains("resume: session-cli resume --session-id"));
    }
}
