use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("invalid hook input: {0}")]
    InvalidHookInput(String),

    #[error("session capture is missing a session id")]
    MissingSessionId,

    #[error(
        "runtime session id is required; no active-session fallback exists"
    )]
    MissingRuntimeSessionId,

    #[error("session capture did not include any turns")]
    EmptyTurns,

    #[error("session store root cannot be empty")]
    EmptyStoreRoot,

    #[error("session id '{0}' must be a UUID from the Copilot hook payload")]
    InvalidSessionId(String),

    #[error("terminal id '{0}' must be a UUID")]
    InvalidTerminalId(String),

    #[error(
        "terminal observer {terminal_id} was not found for session {session_id}"
    )]
    TerminalNotFound {
        session_id: String,
        terminal_id: String,
    },

    #[error("terminal observer {terminal_id} is closed")]
    TerminalClosed { terminal_id: String },

    #[error(
        "session identity `{0}` must be a UUID; use the capture or provisioning UUID"
    )]
    SessionIdentityMustBeUuid(String),

    #[error(
        "requested session identity {requested} does not match provisioned worktree identity {provisioned}"
    )]
    SessionIdentityMismatch {
        requested: String,
        provisioned: String,
    },

    #[error("workspace slug contains invalid path characters: {0}")]
    InvalidWorkspaceSlug(String),

    #[error("workspace session id contains invalid path characters: {0}")]
    InvalidWorkspaceSessionId(String),

    #[error("invalid pinned entity URN: {0}")]
    InvalidEntityUrn(String),

    #[error("session owner id cannot be empty")]
    MissingOwnerId,

    #[error("session ticket id cannot be empty")]
    MissingTicketId,

    #[error("worktree path cannot be empty")]
    EmptyWorktreePath,

    #[error("worktree branch cannot be empty")]
    EmptyWorktreeBranch,

    #[error("worktree {path} is not a registered managed worktree: {reason}")]
    InvalidManagedWorktree { path: PathBuf, reason: String },

    #[error(
        "worktree {path} is checked out on branch {actual}, not {expected}"
    )]
    WorktreeBranchMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("store path has no parent directory: {0}")]
    InvalidStorePath(PathBuf),

    #[error(
        "session {session_id} has no main-checkout worktree registry entry; migration required"
    )]
    MissingWorktreeAssignment { session_id: String },

    #[error(
        "session {session_id} worktree registry points at missing worktree {path}"
    )]
    RegisteredWorktreeMissing { session_id: String, path: PathBuf },

    #[error("session {session_id} ownership mismatch for worktree check-in")]
    SessionOwnershipMismatch { session_id: String },

    #[error(
        "worktree path {path} is already owned by active session {session_id}"
    )]
    WorktreeConflict { path: PathBuf, session_id: String },

    #[error(
        "cross-session worktree reuse requires an explicit adopt flow: predecessor {session_id} already owns {path}"
    )]
    CrossSessionReuseRequiresAdopt { session_id: String, path: PathBuf },

    #[error("failed to serialize session data for {path}: {source}")]
    Serialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to deserialize session data from {path}: {source}")]
    Deserialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("session data was not found at {path}")]
    NotFound { path: PathBuf },

    #[error("runtime context for workspace session {session_id} was not found")]
    RuntimeContextNotFound { session_id: String },

    #[error("session finish is blocked: {reason}")]
    FinishBlocked { reason: String },

    #[error(
        "workspace session {session_id} is finished and immutable; \
         a mutation was rejected"
    )]
    WorkspaceFinished { session_id: String },

    #[error(
        "concurrent mutation conflict for workspace session {session_id}: \
         another mutation holds the runtime lock"
    )]
    RuntimeMutationConflict { session_id: String },

    #[error("no persisted sessions were found under {root}")]
    NoSessionsFound { root: PathBuf },

    #[error(
        "session schema version mismatch at {path}: found {found}, expected {expected}"
    )]
    SchemaVersionMismatch {
        path: PathBuf,
        found: u32,
        expected: u32,
    },

    #[error(
        "incoming transcript conflicts with persisted session {session_id} ({existing_turns} existing, {incoming_turns} incoming)"
    )]
    TranscriptConflict {
        session_id: String,
        existing_turns: usize,
        incoming_turns: usize,
    },

    #[error("filesystem operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("session move failed: {0}")]
    Move(String),

    #[error(
        "handoff package is incomplete — missing required fields: {fields}"
    )]
    HandoffPackageIncomplete { fields: String },

    #[error(
        "handoff package references a path that does not exist under the \
         workspace root ({workspace_root}): {path}"
    )]
    HandoffPathNotFound {
        path: String,
        workspace_root: PathBuf,
    },

    #[error(
        "handoff {handoff_id} was not found in any session's handoff backlog"
    )]
    HandoffNotFound { handoff_id: String },

    #[error(
        "handoff {handoff_id} is already claimed by target session \
         {target_session_id}"
    )]
    HandoffAlreadyClaimed {
        handoff_id: String,
        target_session_id: String,
    },

    #[error(
        "workflow graph for workspace session {session_id} is \
         structurally invalid: {issues}"
    )]
    WorkflowGraphInvalid { session_id: String, issues: String },

    #[error(
        "workflow diagnostics for workspace session {session_id} \
         are unresolved: {diagnostics}"
    )]
    WorkflowDiagnosticsUnresolved {
        session_id: String,
        diagnostics: String,
    },
}
