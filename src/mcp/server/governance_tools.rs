#[allow(unused_imports)]
use super::*;

#[tool_router(router = governance_router, vis = "pub(super)")]
impl SessionServer {
    #[tool(
        name = "session_grant_create",
        description = "Create a budget-offset grant for the graded cost gate. Grants do not authorize filesystem paths or override PATH_OUTSIDE_SESSION_WORKTREE."
    )]
    pub async fn session_grant_create(
        &self,
        Parameters(input): Parameters<GrantCreateInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::{BudgetGrantScope, create_grant};

        let scope = match input.scope.to_lowercase().as_str() {
            "session" => BudgetGrantScope::Session,
            "subagent" => BudgetGrantScope::Subagent,
            _ => {
                return Err(McpError::invalid_params(
                    format!(
                        "invalid scope: {}. allowed values: session, subagent",
                        input.scope
                    ),
                    None,
                ));
            }
        };

        let grant = create_grant(
            &self.config_for_workspace(&input.workspace)?,
            scope,
            input.offset,
            input.model,
            input.ttl_seconds,
        )
        .map_err(Self::session_err)?;

        Self::json_result(&grant)
    }

    #[tool(
        name = "session_grant_list",
        description = "List budget-offset grants for the cost gate. Grants do not authorize filesystem paths."
    )]
    pub async fn session_grant_list(
        &self,
        Parameters(input): Parameters<GrantListInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::list_grants;

        let grants = list_grants(&self.config_for_workspace(&input.workspace)?)
            .map_err(Self::session_err)?;

        Self::json_result(&grants)
    }

    #[tool(
        name = "session_grant_revoke",
        description = "Revoke (delete) a budget-offset grant by its ID."
    )]
    pub async fn session_grant_revoke(
        &self,
        Parameters(input): Parameters<GrantRevokeInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::revoke_grant;

        let revoked = revoke_grant(
            &self.config_for_workspace(&input.workspace)?,
            &input.grant_id,
        )
        .map_err(Self::session_err)?;

        Self::json_result(&serde_json::json!({
            "revoked": revoked,
            "grant_id": input.grant_id,
        }))
    }

    #[tool(
        name = "session_escalation_create",
        description = "Create a new escalation record for upward problem delegation."
    )]
    pub async fn session_escalation_create(
        &self,
        Parameters(input): Parameters<EscalationCreateInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::{create_escalation, escalation_marker};

        let escalation = create_escalation(
            &self.config_for_workspace(&input.workspace)?,
            input.blocking_decision,
            input.context,
            input.requested_capability,
            input.options_considered,
            input.session_id,
            input.from_model,
        )
        .map_err(Self::session_err)?;

        // Include the marker in the response
        let mut result = serde_json::to_value(&escalation)
            .map_err(|e| McpError::internal_error(format!("serialization: {e}"), None))?;

        if let Some(obj) = result.as_object_mut() {
            obj.insert(
                "marker".to_string(),
                serde_json::Value::String(escalation_marker(&escalation.escalation_id)),
            );
        }

        Self::json_result(&result)
    }

    #[tool(
        name = "session_escalation_list",
        description = "List escalations in the store, optionally filtered by status."
    )]
    pub async fn session_escalation_list(
        &self,
        Parameters(input): Parameters<EscalationListInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::{EscalationStatus, list_escalations};

        let status_filter = if let Some(status_str) = input.status {
            match status_str.to_lowercase().as_str() {
                "open" => Some(EscalationStatus::Open),
                "resolved" => Some(EscalationStatus::Resolved),
                _ => {
                    return Err(McpError::invalid_params(
                        format!(
                            "invalid status: {}. allowed values: open, resolved",
                            status_str
                        ),
                        None,
                    ));
                }
            }
        } else {
            None
        };

        let escalations =
            list_escalations(&self.config_for_workspace(&input.workspace)?, status_filter)
                .map_err(Self::session_err)?;

        Self::json_result(&escalations)
    }

    #[tool(
        name = "session_escalation_get",
        description = "Get a single escalation by ID."
    )]
    pub async fn session_escalation_get(
        &self,
        Parameters(input): Parameters<EscalationGetInput>,
    ) -> Result<CallToolResult, McpError> {
        use session_api::get_escalation;

        let escalation = get_escalation(
            &self.config_for_workspace(&input.workspace)?,
            &input.escalation_id,
        )
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("escalation not found: {}", input.escalation_id),
                None,
            )
        })?;

        Self::json_result(&escalation)
    }

    #[tool(
        name = "session_escalation_resolve",
        description = "Resolve an escalation with a resolution action and details."
    )]
    pub async fn session_escalation_resolve(
        &self,
        Parameters(input): Parameters<EscalationResolveInput>,
    ) -> Result<CallToolResult, McpError> {
        use chrono::Utc;
        use session_api::{EscalationAction, EscalationResolution, resolve_escalation};

        let action = match input.action.to_lowercase().as_str() {
            "handled" => EscalationAction::Handled,
            "granted-offset" => EscalationAction::GrantedOffset,
            "escalated-to-user" => EscalationAction::EscalatedToUser,
            "spawned-session" => EscalationAction::SpawnedSession,
            _ => {
                return Err(McpError::invalid_params(
                    format!(
                        "invalid action: {}. allowed values: handled, granted-offset, escalated-to-user, spawned-session",
                        input.action
                    ),
                    None,
                ));
            }
        };

        let resolution = EscalationResolution {
            action,
            note: input.note,
            offset_grant_id: input.grant_id,
            spawned_session_id: input.spawned_session_id,
            resolved_at: Utc::now(),
        };

        let escalation = resolve_escalation(
            &self.config_for_workspace(&input.workspace)?,
            &input.escalation_id,
            resolution,
        )
        .map_err(Self::session_err)?;

        Self::json_result(&escalation)
    }
}
