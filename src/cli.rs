use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use memory_kernel::workspace;
use session_api::{
    DEFAULT_PROMPT_SUMMARIZE_THRESHOLD_CHARS, DEFAULT_SKELETON_PREVIEW_CHARS, PromptPackOptions,
    RelationStrength, SessionAuditSelector, SessionError, SessionHandoffPackage,
    SessionHandoffTargetTicket, SessionHandoffUpwardContextEntry, SessionQuery,
    SessionRuntimeInitRequest, SessionStoreConfig, SessionTerminalCreateRequest,
    SessionValidationGate, SessionWorkflowEdge, SessionWorkflowEdgeKind, SessionWorkflowNodeDraft,
    SessionWorkflowNodeKind, SessionWorkflowNodeRequirement, SessionWorkflowNodeStatus,
    SessionWorktreeCheckInRequest, SessionWorktreeUnassignRequest, ToolMetricsWindow,
};

#[path = "cli/write_policy.rs"]
mod write_policy;

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

    /// Workspace/repo root that owns the canonical `.session` store.
    /// Lets a tool run from an ancestor checkout target a nested workspace.
    #[arg(long = "workspace", alias = "workspace-root", global = true)]
    pub workspace_root: Option<PathBuf>,

    /// Explicit store root retained for compatibility with older callers.
    #[arg(long = "store-root", global = true, conflicts_with = "workspace_root")]
    pub store_root: Option<PathBuf>,

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
    /// Remove an explicit assignment and return the session to main.
    Unassign(UnassignArgs),
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

pub fn run(mut cli: SessionCli) -> Result<CliOutput, CliRunError> {
    if write_policy::command_writes_store(&cli.command)
        && cli.workspace_root.is_none()
        && cli.store_root.is_none()
    {
        return Err(CliRunError::BadRequest(
            "session writes require explicit --workspace <path> or --store-root <path>".to_string(),
        ));
    }

    if write_policy::command_writes_store(&cli.command) {
        if let Some(workspace_root) = cli.workspace_root.as_ref() {
            let selector = workspace_root.to_string_lossy();
            cli.workspace_root = Some(
                workspace::normalize_explicit_workspace_selector(Some(&selector))
                    .map_err(|err| CliRunError::BadRequest(err.to_string()))?,
            );
        }
    }

    let store_root = match cli.store_root.as_deref() {
        Some(store_root) => store_root.to_path_buf(),
        None => match cli.workspace_root.as_deref() {
            Some(workspace_root) => workspace::resolve_store_root_for_initialization_from(
                workspace_root,
                SESSION_STORE_DIR,
            ),
            None => workspace::resolve_session_store_root_from(
                workspace::working_dir().as_deref(),
                SESSION_STORE_DIR,
            ),
        },
    };
    let config = SessionStoreConfig::new(store_root);

    let payload = dispatch(&config, cli.command)?;

    match machine_output_format(cli.json, cli.toon) {
        Some(format) => Ok(CliOutput::Machine(payload, format)),
        None => Ok(CliOutput::Text(render_human(&payload))),
    }
}

fn parse_relation_strength(value: &str) -> Result<RelationStrength, CliRunError> {
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

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, CliRunError> {
    serde_json::to_value(value).map_err(|err| CliRunError::Serialization(err.to_string()))
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
    serde_json::to_string_pretty(payload).unwrap_or_else(|_| format!("{payload:?}"))
}

pub fn error_output(message: &str, format: Option<MachineOutputFormat>) -> String {
    let payload = json!({"status": "error", "message": message});
    match format {
        Some(MachineOutputFormat::Json) => payload.to_string(),
        Some(MachineOutputFormat::Toon) => toon_format::encode_default(&payload)
            .unwrap_or_else(|_| format!("status: error\nmessage: {message}")),
        None => message.to_string(),
    }
}

pub fn render_machine_output(
    payload: &Value,
    format: MachineOutputFormat,
) -> Result<String, String> {
    match format {
        MachineOutputFormat::Json => {
            serde_json::to_string_pretty(payload).map_err(|err| err.to_string())
        }
        MachineOutputFormat::Toon => {
            toon_format::encode_default(payload).map_err(|err| err.to_string())
        }
    }
}

pub fn machine_output_format(as_json: bool, as_toon: bool) -> Option<MachineOutputFormat> {
    if as_json {
        Some(MachineOutputFormat::Json)
    } else if as_toon {
        Some(MachineOutputFormat::Toon)
    } else {
        None
    }
}

pub fn requested_machine_output_format_from_args() -> Option<MachineOutputFormat> {
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
#[path = "cli/tests.rs"]
mod tests;

#[path = "cli/workflow.rs"]
mod workflow;
#[allow(unused_imports)]
pub use workflow::*;

#[path = "cli/handoff.rs"]
mod handoff;
#[allow(unused_imports)]
pub use handoff::*;

#[path = "cli/governance.rs"]
mod governance;
#[allow(unused_imports)]
pub use governance::*;

#[path = "cli/moves.rs"]
mod moves;
#[allow(unused_imports)]
pub use moves::*;

#[path = "cli/runtime_args.rs"]
mod runtime_args;
#[allow(unused_imports)]
pub use runtime_args::*;

#[path = "cli/observation_args.rs"]
mod observation_args;
#[allow(unused_imports)]
pub use observation_args::*;

#[path = "cli/governance_args.rs"]
mod governance_args;
#[allow(unused_imports)]
pub use governance_args::*;

#[path = "cli/dispatch.rs"]
mod dispatch;
#[allow(unused_imports)]
pub use dispatch::*;
