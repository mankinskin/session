#[allow(unused_imports)]
use super::*;

#[tool_router(router = move_router, vis = "pub(super)")]
impl SessionServer {
    #[tool(
        name = "session_move_preflight",
        description = "Read-only preflight plan for moving a session to another workspace store."
    )]
    pub async fn session_move_preflight(
        &self,
        Parameters(input): Parameters<SessionMoveInput>,
    ) -> Result<CallToolResult, McpError> {
        let session_id = input.id.parse::<Uuid>().map_err(|error| {
            McpError::invalid_params(format!("invalid session UUID: {error}"), None)
        })?;
        let target_selector =
            workspace::normalize_explicit_workspace_selector(Some(&input.to_workspace_root))
                .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
        let target_workspace_root = workspace::canonicalize_workspace_root_strict(&target_selector)
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
            McpError::invalid_params(format!("invalid session UUID: {error}"), None)
        })?;
        let target_selector =
            workspace::normalize_explicit_workspace_selector(Some(&input.to_workspace_root))
                .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
        let target_workspace_root = workspace::canonicalize_workspace_root_strict(&target_selector)
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
            McpError::invalid_params(format!("invalid journal id: {error}"), None)
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
            McpError::invalid_params(format!("invalid journal id: {error}"), None)
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
