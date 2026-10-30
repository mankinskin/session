#[allow(unused_imports)]
use super::*;

#[tool_router(router = lifecycle_router, vis = "pub(super)")]
impl SessionServer {
    #[tool(
        name = "session_capabilities",
        description = "List the canonical session lifecycle workflow, its ordered steps, and the enum-valued workflow parameters with their legal values."
    )]
    pub async fn session_capabilities(
        &self,
        Parameters(_input): Parameters<CapabilitiesInput>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&session_capability_catalog())
    }

    #[tool(
        name = "session_runtime_init",
        description = "Initialize or resume durable runtime context for an explicit Copilot session UUID."
    )]
    pub async fn session_runtime_init(
        &self,
        Parameters(input): Parameters<RuntimeInitInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .config_for_workspace(&input.workspace)?
            .init_runtime_context(SessionRuntimeInitRequest {
                session_id: Some(input.session_id),
                predecessor_run_id: input.predecessor_run_id,
                force_new_run: input.force_new_run,
            })
            .map_err(Self::session_err)?;
        let handle = result.context.session_id.clone();
        Self::json_result_with_handle(&handle, &result)
    }

    #[tool(
        name = "session_runtime_resume",
        description = "Resume an existing Copilot session UUID using predecessor run lineage."
    )]
    pub async fn session_runtime_resume(
        &self,
        Parameters(input): Parameters<RuntimeResumeInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .config_for_workspace(&input.workspace)?
            .resume_workspace_context(&input.session_id, &input.predecessor_run_id)
            .map_err(Self::session_err)?;
        let handle = result.context.session_id.clone();
        Self::json_result_with_handle(&handle, &result)
    }

    #[tool(
        name = "session_runtime_pin",
        description = "Pin a canonical ce://<workspace>/<store>/<entity> URN into runtime context. Call session_runtime_init first with the same session_id and workspace. Raw paths and external references are not session pins."
    )]
    pub async fn session_runtime_pin(
        &self,
        Parameters(input): Parameters<RuntimePinInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .pin_runtime_entity(
                &input.session_id,
                &input.entity_urn,
                input.relation,
                input.reason,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_runtime_unpin",
        description = "Unpin an entity URN from runtime workspace context."
    )]
    pub async fn session_runtime_unpin(
        &self,
        Parameters(input): Parameters<RuntimeUnpinInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .unpin_runtime_entity(&input.session_id, &input.entity_urn)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_runtime_view",
        description = "Read headers-only runtime workspace context view."
    )]
    pub async fn session_runtime_view(
        &self,
        Parameters(input): Parameters<RuntimeViewInput>,
    ) -> Result<CallToolResult, McpError> {
        let view = self
            .config_for_workspace(&input.workspace)?
            .view_runtime_context(&input.session_id)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &view)
    }

    #[tool(
        name = "session_runtime_render_instructions",
        description = "Render a focused instruction set from the Copilot session UUID's pinned rule URNs."
    )]
    pub async fn session_runtime_render_instructions(
        &self,
        Parameters(input): Parameters<RuntimeViewInput>,
    ) -> Result<CallToolResult, McpError> {
        let render = self
            .config_for_workspace(&input.workspace)?
            .render_pinned_rule_instructions(&input.session_id)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &serde_json::json!({"render": render}))
    }

    #[tool(
        name = "session_handoff",
        description = "Persist structured handoff record before rendering handoff summary."
    )]
    pub async fn session_handoff(
        &self,
        Parameters(input): Parameters<RuntimeHandoffInput>,
    ) -> Result<CallToolResult, McpError> {
        let package = if !input.objective.is_empty()
            || !input.target_tickets.is_empty()
            || !input.target_files.is_empty()
            || !input.decisions.is_empty()
            || !input.non_goals.is_empty()
            || !input.context_anchors.is_empty()
            || !input.open_escalations.is_empty()
            || !input.higher_level_objective.is_empty()
            || !input.upward_context.is_empty()
            || input.risk_notes.is_some()
            || input.predecessor_handoff.is_some()
        {
            Some(SessionHandoffPackage {
                objective: input.objective,
                target_tickets: input.target_tickets.into_iter().map(Into::into).collect(),
                higher_level_objective: input.higher_level_objective,
                upward_context: input.upward_context.into_iter().map(Into::into).collect(),
                target_files: input.target_files,
                decisions: input.decisions,
                non_goals: input.non_goals,
                context_anchors: input.context_anchors,
                open_escalations: input.open_escalations,
                risk_notes: input.risk_notes,
                predecessor_handoff: input.predecessor_handoff,
            })
        } else {
            None
        };
        let result = self
            .config_for_workspace(&input.workspace)?
            .create_handoff_result(
                &input.session_id,
                package,
                input.validation.into_iter().map(Into::into).collect(),
                None,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &result)
    }

    #[tool(
        name = "session_terminal_create",
        description = "Create a session-scoped human-owned terminal observer. This tool never starts a command or sends terminal input."
    )]
    pub async fn session_terminal_create(
        &self,
        Parameters(input): Parameters<TerminalCreateInput>,
    ) -> Result<CallToolResult, McpError> {
        let manifest = self
            .config_for_workspace(&input.workspace)?
            .create_terminal_observer(SessionTerminalCreateRequest {
                session_id: input.session_id.clone(),
                label: input.label,
                cwd: input.cwd,
            })
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &manifest)
    }

    #[tool(
        name = "session_terminal_status",
        description = "Read a human-owned observer terminal status. No terminal input or command execution is available."
    )]
    pub async fn session_terminal_status(
        &self,
        Parameters(input): Parameters<TerminalStatusInput>,
    ) -> Result<CallToolResult, McpError> {
        let manifest = self
            .config_for_workspace(&input.workspace)?
            .terminal_status(&input.session_id, &input.terminal_id)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &manifest)
    }

    #[tool(
        name = "session_terminal_peek",
        description = "Read a bounded window of human-owned observer terminal output. No terminal input or command execution is available."
    )]
    pub async fn session_terminal_peek(
        &self,
        Parameters(input): Parameters<TerminalPeekInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .config_for_workspace(&input.workspace)?
            .peek_terminal_output(
                &input.session_id,
                &input.terminal_id,
                input.offset.unwrap_or(0),
                input.limit.unwrap_or(50),
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &result)
    }

    #[tool(
        name = "session_terminal_close",
        description = "Close a human-owned observer terminal record so later output cannot be appended."
    )]
    pub async fn session_terminal_close(
        &self,
        Parameters(input): Parameters<TerminalStatusInput>,
    ) -> Result<CallToolResult, McpError> {
        let manifest = self
            .config_for_workspace(&input.workspace)?
            .close_terminal_observer(&input.session_id, &input.terminal_id)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &manifest)
    }

    #[tool(
        name = "session_finish",
        description = "Explicitly finish workflow, enforcing required node and validation gates."
    )]
    pub async fn session_finish(
        &self,
        Parameters(input): Parameters<RuntimeFinishInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .config_for_workspace(&input.workspace)?
            .finish_workflow(
                &input.session_id,
                input.validation.into_iter().map(Into::into).collect(),
                input.deferred_optional_node_ids,
                None,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &result)
    }
}
