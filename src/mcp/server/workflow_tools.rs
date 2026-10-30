#[allow(unused_imports)]
use super::*;

#[tool_router(router = workflow_router, vis = "pub(super)")]
impl SessionServer {
    #[tool(
        name = "session_workflow_add_node",
        description = "Add a node to the durable session workflow graph. If node_id \
                       matches an existing node, this call is a no-op and the \
                       existing node is left unchanged (no error, no duplicate)."
    )]
    pub async fn session_workflow_add_node(
        &self,
        Parameters(input): Parameters<WorkflowAddNodeInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_add_node(
                &input.session_id,
                SessionWorkflowNodeDraft {
                    node_id: input.node_id,
                    kind: parse_node_kind(&input.kind)?,
                    requirement: parse_requirement(&input.requirement)?,
                    title: input.title,
                    ticket_urn: input.ticket_urn,
                    spec_urn: input.spec_urn,
                    anchor_urn: input.anchor_urn,
                    category: input.category,
                    cached_ticket_title: input.cached_ticket_title,
                    validation_spec_id: input.validation_spec_id,
                },
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_add_nodes",
        description = "Atomically add workflow nodes; errors identify nodes[index]. \
                       Any node whose node_id matches an existing node is a \
                       no-op (left unchanged, not duplicated); use \
                       session_workflow_update_node to change an existing node."
    )]
    pub async fn session_workflow_add_nodes(
        &self,
        Parameters(input): Parameters<WorkflowAddNodesInput>,
    ) -> Result<CallToolResult, McpError> {
        let drafts = input
            .nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| {
                Ok(SessionWorkflowNodeDraft {
                    node_id: node.node_id,
                    kind: parse_node_kind(&node.kind)
                        .map_err(|error| indexed_mcp_error("nodes", index, error))?,
                    requirement: parse_requirement(&node.requirement)
                        .map_err(|error| indexed_mcp_error("nodes", index, error))?,
                    title: node.title,
                    ticket_urn: node.ticket_urn,
                    spec_urn: node.spec_urn,
                    anchor_urn: node.anchor_urn,
                    category: node.category,
                    cached_ticket_title: node.cached_ticket_title,
                    validation_spec_id: node.validation_spec_id,
                })
            })
            .collect::<Result<Vec<_>, McpError>>()?;
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_add_nodes(&input.session_id, drafts)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_add_edge",
        description = "Add a directed edge between workflow nodes."
    )]
    pub async fn session_workflow_add_edge(
        &self,
        Parameters(input): Parameters<WorkflowAddEdgeInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_add_edge(
                &input.session_id,
                &input.from,
                &input.to,
                parse_edge_kind(&input.kind)?,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_add_edges",
        description = "Atomically add workflow edges; errors identify edges[index]."
    )]
    pub async fn session_workflow_add_edges(
        &self,
        Parameters(input): Parameters<WorkflowAddEdgesInput>,
    ) -> Result<CallToolResult, McpError> {
        let edges = input
            .edges
            .into_iter()
            .enumerate()
            .map(|(index, edge)| {
                Ok(SessionWorkflowEdge {
                    from: edge.from,
                    to: edge.to,
                    kind: parse_edge_kind(&edge.kind)
                        .map_err(|error| indexed_mcp_error("edges", index, error))?,
                })
            })
            .collect::<Result<Vec<_>, McpError>>()?;
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_add_edges(&input.session_id, edges)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_set_status",
        description = "Update workflow node status and optional deferred reason."
    )]
    pub async fn session_workflow_set_status(
        &self,
        Parameters(input): Parameters<WorkflowSetStatusInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_update_node_status(
                &input.session_id,
                &input.node_id,
                parse_node_status(&input.status)?,
                input.deferred_reason,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_update_node",
        description = "Patch fields on an existing workflow node in place (repair surface for a \
                       wedged node, e.g. a validation node missing validation_spec_id). Every \
                       field is optional: omit a field to leave it unchanged, set it to \
                       overwrite. The merged node is re-validated with the same rules enforced \
                       at node creation, so a patch cannot introduce a new wedge."
    )]
    pub async fn session_workflow_update_node(
        &self,
        Parameters(input): Parameters<WorkflowUpdateNodeInput>,
    ) -> Result<CallToolResult, McpError> {
        let kind = input.kind.as_deref().map(parse_node_kind).transpose()?;
        let requirement = input
            .requirement
            .as_deref()
            .map(parse_requirement)
            .transpose()?;
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_update_node(
                &input.session_id,
                &input.node_id,
                SessionWorkflowNodePatch {
                    kind,
                    requirement,
                    title: input.title,
                    ticket_urn: input.ticket_urn,
                    spec_urn: input.spec_urn,
                    anchor_urn: input.anchor_urn,
                    category: input.category,
                    cached_ticket_title: input.cached_ticket_title,
                    validation_spec_id: input.validation_spec_id,
                },
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_remove_node",
        description = "Delete a workflow node and any edges that reference it. Repair surface for \
                       a node that cannot be fixed in place (or should never have been added), \
                       instead of permanently blocking session_finish/session_handoff."
    )]
    pub async fn session_workflow_remove_node(
        &self,
        Parameters(input): Parameters<WorkflowRemoveNodeInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_remove_node(&input.session_id, &input.node_id)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_promote",
        description = "Promote a workflow node to a ticket-backed node while preserving identity."
    )]
    pub async fn session_workflow_promote(
        &self,
        Parameters(input): Parameters<WorkflowPromoteInput>,
    ) -> Result<CallToolResult, McpError> {
        let context = self
            .config_for_workspace(&input.workspace)?
            .workflow_promote_node_to_ticket(
                &input.session_id,
                &input.node_id,
                &input.ticket_urn,
                input.cached_ticket_title,
            )
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &context)
    }

    #[tool(
        name = "session_workflow_render_terminal",
        description = "Render the workflow graph as deterministic terminal text."
    )]
    pub async fn session_workflow_render_terminal(
        &self,
        Parameters(input): Parameters<WorkflowRenderInput>,
    ) -> Result<CallToolResult, McpError> {
        let render = self
            .config_for_workspace(&input.workspace)?
            .workflow_render_terminal(&input.session_id, None)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &serde_json::json!({"render": render}))
    }

    #[tool(
        name = "session_workflow_render_mermaid",
        description = "Render the workflow graph as deterministic Mermaid flowchart text."
    )]
    pub async fn session_workflow_render_mermaid(
        &self,
        Parameters(input): Parameters<WorkflowRenderInput>,
    ) -> Result<CallToolResult, McpError> {
        let render = self
            .config_for_workspace(&input.workspace)?
            .workflow_render_mermaid(&input.session_id, None)
            .map_err(Self::session_err)?;
        Self::json_result_with_handle(&input.session_id, &serde_json::json!({"render": render}))
    }
}
