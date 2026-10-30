#[allow(unused_imports)]
use super::*;

/// Shared handler for canonical nested (`session workflow <subcommand>`) and
/// flat (`session workflow-<subcommand>`) command forms.
pub(super) fn handle_workflow_command(
    config: &SessionStoreConfig,
    command: WorkflowCommand,
) -> Result<Value, CliRunError> {
    match command {
        WorkflowCommand::AddNode(args) => {
            let context = config.workflow_add_node(
                &args.session_id,
                SessionWorkflowNodeDraft {
                    node_id: args.node_id,
                    kind: parse_node_kind(&args.kind)?,
                    requirement: parse_requirement(&args.requirement)?,
                    title: args.title,
                    ticket_urn: args.ticket_urn,
                    spec_urn: args.spec_urn,
                    anchor_urn: args.anchor_urn,
                    category: args.category,
                    cached_ticket_title: args.cached_ticket_title,
                    validation_spec_id: args.validation_spec_id,
                },
            )?;
            to_value(&context)
        }
        WorkflowCommand::AddNodes(args) => {
            let nodes = serde_json::from_str::<Vec<WorkflowNodeDraftJson>>(&args.nodes_json)
                .map_err(|error| {
                    CliRunError::BadRequest(format!("invalid --nodes-json payload: {error}"))
                })?
                .into_iter()
                .enumerate()
                .map(|(index, node)| {
                    Ok(SessionWorkflowNodeDraft {
                        node_id: node.node_id,
                        kind: parse_node_kind(&node.kind)
                            .map_err(|error| indexed_cli_error("nodes", index, error))?,
                        requirement: parse_requirement(&node.requirement)
                            .map_err(|error| indexed_cli_error("nodes", index, error))?,
                        title: node.title,
                        ticket_urn: node.ticket_urn,
                        spec_urn: node.spec_urn,
                        anchor_urn: node.anchor_urn,
                        category: node.category,
                        cached_ticket_title: node.cached_ticket_title,
                        validation_spec_id: node.validation_spec_id,
                    })
                })
                .collect::<Result<Vec<_>, CliRunError>>()?;
            let context = config.workflow_add_nodes(&args.session_id, nodes)?;
            to_value(&context)
        }
        WorkflowCommand::AddEdge(args) => {
            let context = config.workflow_add_edge(
                &args.session_id,
                &args.from,
                &args.to,
                parse_edge_kind(&args.kind)?,
            )?;
            to_value(&context)
        }
        WorkflowCommand::AddEdges(args) => {
            let edges = serde_json::from_str::<Vec<WorkflowEdgeDraftJson>>(&args.edges_json)
                .map_err(|error| {
                    CliRunError::BadRequest(format!("invalid --edges-json payload: {error}"))
                })?
                .into_iter()
                .enumerate()
                .map(|(index, edge)| {
                    Ok(SessionWorkflowEdge {
                        from: edge.from,
                        to: edge.to,
                        kind: parse_edge_kind(&edge.kind)
                            .map_err(|error| indexed_cli_error("edges", index, error))?,
                    })
                })
                .collect::<Result<Vec<_>, CliRunError>>()?;
            let context = config.workflow_add_edges(&args.session_id, edges)?;
            to_value(&context)
        }
        WorkflowCommand::SetStatus(args) => {
            let context = config.workflow_update_node_status(
                &args.session_id,
                &args.node_id,
                parse_node_status(&args.status)?,
                args.deferred_reason,
            )?;
            to_value(&context)
        }
        WorkflowCommand::Promote(args) => {
            let context = config.workflow_promote_node_to_ticket(
                &args.session_id,
                &args.node_id,
                &args.ticket_urn,
                args.cached_ticket_title,
            )?;
            to_value(&context)
        }
        WorkflowCommand::RenderTerminal(args) => {
            let rendered = config.workflow_render_terminal(&args.session_id, None)?;
            to_value(&json!({"render": rendered}))
        }
        WorkflowCommand::RenderMermaid(args) => {
            let rendered = config.workflow_render_mermaid(&args.session_id, None)?;
            to_value(&json!({"render": rendered}))
        }
    }
}

pub(super) fn parse_node_kind(value: &str) -> Result<SessionWorkflowNodeKind, CliRunError> {
    match value {
        "ticket" => Ok(SessionWorkflowNodeKind::Ticket),
        "validation" => Ok(SessionWorkflowNodeKind::Validation),
        "spec" => Ok(SessionWorkflowNodeKind::Spec),
        // `task` is the generic descriptive bucket; legacy cosmetic kinds are
        // accepted as back-compat aliases.
        "task" | "action" | "decision" | "checkpoint" => Ok(SessionWorkflowNodeKind::Task),
        _ => Err(CliRunError::BadRequest(format!(
            "invalid workflow node kind: {value}. allowed values: \
             ticket, validation, spec, task \
             (deprecated aliases mapped to task: action, decision, checkpoint); \
             for a custom label, use kind=task with category=\"<your-label>\""
        ))),
    }
}

pub(super) fn parse_requirement(
    value: &str,
) -> Result<SessionWorkflowNodeRequirement, CliRunError> {
    match value {
        "required" => Ok(SessionWorkflowNodeRequirement::Required),
        "optional" => Ok(SessionWorkflowNodeRequirement::Optional),
        _ => Err(CliRunError::BadRequest(format!(
            "invalid workflow requirement: {value}. allowed values: \
             required, optional; did you mean requirement=required?"
        ))),
    }
}

pub(super) fn parse_edge_kind(value: &str) -> Result<SessionWorkflowEdgeKind, CliRunError> {
    match value {
        "depends-on" | "depends_on" => Ok(SessionWorkflowEdgeKind::DependsOn),
        "order" => Ok(SessionWorkflowEdgeKind::Order),
        _ => Err(CliRunError::BadRequest(format!(
            "invalid workflow edge kind: {value}. allowed values: \
             depends-on (alias depends_on), order; \
             did you mean kind=depends-on?"
        ))),
    }
}

pub(super) fn parse_node_status(value: &str) -> Result<SessionWorkflowNodeStatus, CliRunError> {
    match value {
        "pending" => Ok(SessionWorkflowNodeStatus::Pending),
        "in-progress" | "in_progress" => Ok(SessionWorkflowNodeStatus::InProgress),
        "blocked" => Ok(SessionWorkflowNodeStatus::Blocked),
        "done" => Ok(SessionWorkflowNodeStatus::Done),
        "deferred" => Ok(SessionWorkflowNodeStatus::Deferred),
        _ => Err(CliRunError::BadRequest(format!(
            "invalid workflow status: {value}. allowed values: \
             pending, in-progress (alias in_progress), blocked, done, deferred; \
             did you mean status=in-progress?"
        ))),
    }
}

pub(super) fn indexed_cli_error(collection: &str, index: usize, error: CliRunError) -> CliRunError {
    let message = match error {
        CliRunError::BadRequest(message) => message,
        other => other.to_string(),
    };
    CliRunError::BadRequest(format!("{collection}[{index}]: {message}"))
}
