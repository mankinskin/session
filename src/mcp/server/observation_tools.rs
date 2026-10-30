#[allow(unused_imports)]
use super::*;

#[tool_router(router = observation_router, vis = "pub(super)")]
impl SessionServer {
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
        name = "session_unassign",
        description = "Explicitly remove a session's registered worktree assignment and return its owning main checkout. Requires the main workspace and matching owner, ticket, recorded path and branch. Supports deleted checkouts, retains an audit receipt, and never deletes checkout content."
    )]
    pub async fn session_unassign(
        &self,
        Parameters(input): Parameters<UnassignInput>,
    ) -> Result<CallToolResult, McpError> {
        let receipt = self
            .config_for_workspace(&input.workspace)?
            .unassign_worktree(SessionWorktreeUnassignRequest {
                session_id: input.session_id,
                owner_id: input.owner_id,
                ticket_id: input.ticket_id,
                expected_worktree_path: PathBuf::from(input.expected_worktree_path),
                expected_branch: input.expected_branch,
                reason: input.reason,
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
        let result = self
            .config()
            .query_sessions(&query)
            .map_err(Self::session_err)?;
        Self::json_result(&serde_json::json!({
            "count": result.sessions.len(),
            "sessions": result.sessions,
            "diagnostics": result.diagnostics,
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
            }
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
}
