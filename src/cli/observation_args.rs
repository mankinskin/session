#[allow(unused_imports)]
use super::*;

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
pub struct UnassignArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub owner_id: String,
    #[arg(long)]
    pub ticket_id: String,
    /// Exact path from the current registry receipt (may no longer exist).
    #[arg(long)]
    pub expected_worktree_path: PathBuf,
    /// Exact branch from the current registry receipt.
    #[arg(long)]
    pub expected_branch: String,
    #[arg(long)]
    pub reason: String,
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
