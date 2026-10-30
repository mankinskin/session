#[allow(unused_imports)]
use super::*;

/// Legal `session_sessions_for_ticket.strength` values, mirroring
/// `session_api::RelationStrength` exactly (widening tiers).
#[derive(JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
pub(super) enum RelationStrengthSchema {
    Strict,
    Linked,
    Mentioned,
}

// ── Input types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckInInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
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
pub struct UnassignInput {
    /// Explicit owning MAIN checkout or its session store, not the assigned checkout.
    pub workspace: String,
    pub session_id: String,
    /// Owner identity recorded in the assignment.
    pub owner_id: String,
    /// Ticket recorded in the assignment.
    pub ticket_id: String,
    /// Exact recorded path; it need not exist.
    pub expected_worktree_path: String,
    /// Exact recorded branch, checked before changing routing.
    pub expected_branch: String,
    /// Required audit reason for returning to main.
    pub reason: String,
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
pub struct ToolMetricsInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
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
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    /// Copilot session UUID to get rollups for.
    pub session_id: String,
}

/// No-argument input for the self-describing capability catalog.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CapabilitiesInput {}
