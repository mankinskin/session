#[allow(unused_imports)]
use super::*;

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
    /// Explicit source store root, such as a legacy `.session` directory.
    #[arg(long = "source-store-root")]
    pub source_store_root: Option<PathBuf>,
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
