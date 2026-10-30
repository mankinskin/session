use std::path::PathBuf;

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use memory_kernel::workspace;
use session_api::{
    DEFAULT_SKELETON_PREVIEW_CHARS, RelationStrength, SessionError, SessionHandoffPackage,
    SessionHandoffTargetTicket, SessionHandoffUpwardContextEntry, SessionHandoffUpwardContextRole,
    SessionQuery, SessionRuntimeInitRequest, SessionStoreConfig, SessionTerminalCreateRequest,
    SessionValidationGate, SessionWorkflowEdge, SessionWorkflowEdgeKind, SessionWorkflowNodeDraft,
    SessionWorkflowNodeKind, SessionWorkflowNodePatch, SessionWorkflowNodeRequirement,
    SessionWorkflowNodeStatus, SessionWorktreeCheckInRequest, SessionWorktreeUnassignRequest,
    ToolMetricsWindow,
};

// ── Server ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SessionServer {
    store_root: PathBuf,
    tool_router: ToolRouter<Self>,
}

impl SessionServer {
    pub fn new(store_root: PathBuf) -> Self {
        Self {
            store_root,
            tool_router: Self::tool_router(),
        }
    }

    fn config(&self) -> SessionStoreConfig {
        SessionStoreConfig::new(self.store_root.clone())
    }

    fn config_for_workspace(
        &self,
        workspace_selector: &str,
    ) -> Result<SessionStoreConfig, McpError> {
        let workspace_root =
            workspace::normalize_explicit_workspace_selector(Some(workspace_selector))
                .map_err(|err| McpError::invalid_params(err.to_string(), None))?;
        Ok(SessionStoreConfig::new(
            workspace::resolve_store_root_for_initialization_from(&workspace_root, ".session"),
        ))
    }

    fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
        let text = serde_json::to_string(value)
            .map_err(|err| McpError::internal_error(format!("serialization: {err}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Serialize `value` and guarantee the Copilot session UUID is present as
    /// the prominent top-line `session_id` field required by every
    /// workflow/runtime call.
    fn json_result_with_handle<T: Serialize>(
        session_id: &str,
        value: &T,
    ) -> Result<CallToolResult, McpError> {
        let mut payload = serde_json::to_value(value)
            .map_err(|err| McpError::internal_error(format!("serialization: {err}"), None))?;
        match payload.as_object_mut() {
            Some(object) => {
                object.insert(
                    "session_id".to_string(),
                    serde_json::Value::String(session_id.to_string()),
                );
            }
            None => {
                payload = serde_json::json!({
                    "session_id": session_id,
                    "result": payload,
                });
            }
        }
        let text = serde_json::to_string(&payload)
            .map_err(|err| McpError::internal_error(format!("serialization: {err}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    fn session_err(err: SessionError) -> McpError {
        match &err {
            SessionError::NotFound { .. }
            | SessionError::MissingSessionId
            | SessionError::MissingOwnerId
            | SessionError::MissingTicketId
            | SessionError::EmptyWorktreePath
            | SessionError::EmptyWorktreeBranch
            | SessionError::InvalidSessionId(_)
            | SessionError::InvalidWorkspaceSessionId(_)
            | SessionError::InvalidEntityUrn(_)
            | SessionError::InvalidWorkspaceSlug(_)
            | SessionError::MissingWorktreeAssignment { .. }
            | SessionError::SessionOwnershipMismatch { .. }
            | SessionError::WorktreeAssignmentChanged { .. }
            | SessionError::MissingWorktreeUnassignReason
            | SessionError::WorktreeConflict { .. }
            | SessionError::CrossSessionReuseRequiresAdopt { .. }
            | SessionError::RuntimeContextNotFound { .. }
            | SessionError::FinishBlocked { .. }
            | SessionError::WorkflowGraphInvalid { .. }
            | SessionError::WorkflowDiagnosticsUnresolved { .. }
            | SessionError::Move(_) => McpError::invalid_params(err.to_string(), None),
            _ => McpError::internal_error(format!("session error: {err}"), None),
        }
    }

    fn move_plan_json(
        report: &memory_kernel::storage::move_kernel::MovePlan,
    ) -> Result<serde_json::Value, McpError> {
        Ok(serde_json::json!({
            "supported": report.supported(),
            "entity_id": report.entity_id,
            "source_workspace_root": path_display(&report.source_workspace_root)?,
            "target_workspace_root": path_display(&report.target_workspace_root)?,
            "source_store_root": path_display(&report.source_store_root)?,
            "target_store_root": path_display(&report.target_store_root)?,
            "source_git_worktree_root": path_display(&report.source_git_worktree_root)?,
            "target_git_worktree_root": path_display(&report.target_git_worktree_root)?,
            "git_worktree_topology": report.git_worktree_topology,
            "source_entity_path": path_display(&report.source_entity_path)?,
            "destination_entity_path": path_display(&report.destination_entity_path)?,
            "inbound_related_entity_ids": report.inbound_related_entity_ids,
            "outbound_related_entity_ids": report.outbound_related_entity_ids,
            "reference_visibility": report.reference_visibility,
            "active_board_entries": report.active_board_entries,
            "historical_board_entries": report.historical_board_entries,
            "active_leases": report.active_leases,
            "path_reference_files": report.path_reference_files
                .iter()
                .map(|p| path_display(p))
                .collect::<Result<Vec<_>, _>>()?,
            "blockers": report.blockers,
            "captured_at": report.captured_at,
        }))
    }

    fn move_outcome_json(
        outcome: &memory_kernel::storage::move_kernel::MoveOutcome,
    ) -> Result<serde_json::Value, McpError> {
        Ok(serde_json::json!({
            "resumed": outcome.resumed,
            "rolled_back": outcome.rolled_back,
            "journal": {
                "id": outcome.journal.id,
                "entity_id": outcome.journal.entity_id,
                "source_store_root": path_display(&outcome.journal.source_store_root)?,
                "target_store_root": path_display(&outcome.journal.target_store_root)?,
                "source_entity_path": path_display(&outcome.journal.source_entity_path)?,
                "destination_entity_path": path_display(&outcome.journal.destination_entity_path)?,
                "phase": outcome.journal.phase,
                "created_at": outcome.journal.created_at,
                "updated_at": outcome.journal.updated_at,
                "steps": outcome.journal.steps,
                "rollback_steps": outcome.journal.rollback_steps,
                "lock_paths": outcome.journal.lock_paths,
                "migrated_board_entries": outcome.journal.migrated_board_entries,
                "rewritten_path_files": outcome.journal.rewritten_path_files,
                "manual_followups": outcome.journal.manual_followups,
                "failure": outcome.journal.failure,
                "next_recovery_step": outcome.journal.next_recovery_step,
            },
        }))
    }
}

fn path_display(path: &std::path::Path) -> Result<String, McpError> {
    workspace::normalize_path_for_display_strict(path).map_err(|error| {
        McpError::invalid_params(
            format!(
                "path payload normalization failed for '{}': {error}",
                path.display()
            ),
            None,
        )
    })
}

fn parse_node_kind(value: &str) -> Result<SessionWorkflowNodeKind, McpError> {
    match value {
        "ticket" => Ok(SessionWorkflowNodeKind::Ticket),
        "validation" => Ok(SessionWorkflowNodeKind::Validation),
        "spec" => Ok(SessionWorkflowNodeKind::Spec),
        // `task` is the generic descriptive bucket. The legacy cosmetic kinds
        // are accepted as back-compat aliases so old call sites keep working.
        "task" | "action" | "decision" | "checkpoint" => Ok(SessionWorkflowNodeKind::Task),
        _ => Err(McpError::invalid_params(
            format!(
                "invalid workflow node kind: {value}. allowed values: \
                 ticket, validation, spec, task \
                 (deprecated aliases mapped to task: action, decision, checkpoint); \
                 for a custom label, use kind=task with category=\"<your-label>\""
            ),
            None,
        )),
    }
}

fn parse_requirement(value: &str) -> Result<SessionWorkflowNodeRequirement, McpError> {
    match value {
        "required" => Ok(SessionWorkflowNodeRequirement::Required),
        "optional" => Ok(SessionWorkflowNodeRequirement::Optional),
        _ => Err(McpError::invalid_params(
            format!(
                "invalid workflow requirement: {value}. allowed values: \
                 required, optional; did you mean requirement=required?"
            ),
            None,
        )),
    }
}

fn parse_edge_kind(value: &str) -> Result<SessionWorkflowEdgeKind, McpError> {
    match value {
        "depends-on" | "depends_on" => Ok(SessionWorkflowEdgeKind::DependsOn),
        "order" => Ok(SessionWorkflowEdgeKind::Order),
        _ => Err(McpError::invalid_params(
            format!(
                "invalid workflow edge kind: {value}. allowed values: \
                 depends-on (alias depends_on), order; \
                 did you mean kind=depends-on?"
            ),
            None,
        )),
    }
}

fn parse_node_status(value: &str) -> Result<SessionWorkflowNodeStatus, McpError> {
    match value {
        "pending" => Ok(SessionWorkflowNodeStatus::Pending),
        "in-progress" | "in_progress" => Ok(SessionWorkflowNodeStatus::InProgress),
        "blocked" => Ok(SessionWorkflowNodeStatus::Blocked),
        "done" => Ok(SessionWorkflowNodeStatus::Done),
        "deferred" => Ok(SessionWorkflowNodeStatus::Deferred),
        _ => Err(McpError::invalid_params(
            format!(
                "invalid workflow status: {value}. allowed values: \
                 pending, in-progress (alias in_progress), blocked, done, deferred; \
                 did you mean status=in-progress?"
            ),
            None,
        )),
    }
}

fn indexed_mcp_error(collection: &str, index: usize, error: McpError) -> McpError {
    McpError::invalid_params(format!("{collection}[{index}]: {}", error.message), None)
}

/// Build the self-describing session capability catalog.
///
/// This is the discoverable entry point for the durable-workflow lifecycle so
/// agents do not have to source-dive to learn the canonical flow or the legal
/// enum values for workflow mutations. It lists the ordered lifecycle steps
/// (`runtime_init` → `pin`/`view` → `workflow_*` → `render_*` → `handoff`/
/// `finish`), the handle every workflow call requires, and the enum-valued
/// parameters mirrored from the `session-api` enums.
fn session_capability_catalog() -> serde_json::Value {
    serde_json::json!({
        "surface": "session-mcp",
        "worktree_assignment": {
            "default": "main",
            "tools": ["session_check_in", "session_lookup", "session_unassign"],
            "unassign": {
                "workspace": "Explicit owning main checkout or its session store.",
                "preconditions": ["owner_id", "ticket_id", "expected_worktree_path", "expected_branch", "reason"],
                "supports_deleted_checkout": true,
                "deletes_checkout_content": false,
                "result": ["main_checkout", "previous_assignment", "unassigned_at", "reason", "audit_path"],
            },
        },
        "handle": {
            "field": "session_id",
            "note": "Required by every workflow/runtime tool. Returned as a \
                     top-line field by session_runtime_init/session_runtime_resume \
                     and echoed by every workflow/runtime tool result.",
        },
        "lifecycle": {
            "name": "durable-session-workflow",
            "nested_roots_supported": true,
            "steps": [
                {"order": 1, "tool": "session_runtime_init",
                 "purpose": "Initialize or resume durable runtime context; returns the session_id handle."},
                {"order": 2, "tool": "session_runtime_pin",
                 "purpose": "Pin a ticket/spec/rule entity URN into runtime context."},
                {"order": 2, "tool": "session_runtime_view",
                 "purpose": "Read headers-only pinned-context view."},
                {"order": 3, "tool": "session_workflow_add_node",
                 "purpose": "Add a durable workflow node (see node kind/requirement enums)."},
                {"order": 3, "tool": "session_workflow_add_nodes",
                 "purpose": "Atomically add multiple durable workflow nodes."},
                {"order": 3, "tool": "session_workflow_add_edge",
                 "purpose": "Link two workflow nodes (see edge kind enum)."},
                {"order": 3, "tool": "session_workflow_add_edges",
                 "purpose": "Atomically add multiple workflow edges."},
                {"order": 3, "tool": "session_workflow_set_status",
                 "purpose": "Update a node status (see status enum)."},
                {"order": 3, "tool": "session_workflow_update_node",
                 "purpose": "Repair surface: patch fields on an existing wedged node in place."},
                {"order": 3, "tool": "session_workflow_remove_node",
                 "purpose": "Repair surface: delete a wedged node and its edges."},
                {"order": 4, "tool": "session_workflow_render_terminal",
                 "purpose": "Render the workflow graph as terminal text."},
                {"order": 4, "tool": "session_workflow_render_mermaid",
                 "purpose": "Render the workflow graph as Mermaid."},
                {"order": 5, "tool": "session_handoff",
                 "purpose": "Persist a structured handoff record."},
                {"order": 5, "tool": "session_finish",
                 "purpose": "Finish the workflow, enforcing required node and validation gates."},
            ],
        },
        "observer_terminal": {
            "tools": [
                "session_terminal_create",
                "session_terminal_status",
                "session_terminal_peek",
                "session_terminal_close"
            ],
            "note": "Observer tools never accept terminal input or shell commands. Human UI owns input; agents read bounded output only.",
        },
        "enums": {
            "workflow_node_kind": {
                "tool": "session_workflow_add_node",
                "param": "kind",
                "behavioral": ["ticket", "validation", "spec"],
                "descriptive": ["task"],
                "deprecated_aliases_mapped_to_task": ["action", "decision", "checkpoint"],
                "note": "Behavioral kinds gate finish and carry side-data: \
                         ticket->ticket_urn, spec->spec_urn, validation->validation_spec_id. \
                         Descriptive nuance belongs in the open `category` field or `title`.",
            },
            "workflow_node_requirement": {
                "tool": "session_workflow_add_node",
                "param": "requirement",
                "values": ["required", "optional"],
            },
            "workflow_edge_kind": {
                "tool": "session_workflow_add_edge",
                "param": "kind",
                "values": ["depends-on", "order"],
                "aliases": {"depends_on": "depends-on"},
            },
            "workflow_node_status": {
                "tool": "session_workflow_set_status",
                "param": "status",
                "values": ["pending", "in-progress", "blocked", "done", "deferred"],
                "aliases": {"in_progress": "in-progress"},
            },
        },
    })
}

// ── Tool implementations ──────────────────────────────────────────────────────

impl SessionServer {
    fn tool_router() -> ToolRouter<Self> {
        Self::lifecycle_router()
            + Self::workflow_router()
            + Self::observation_router()
            + Self::governance_router()
            + Self::move_router()
    }
}

// ── MCP handler trait ─────────────────────────────────────────────────────────

#[tool_handler]
impl ServerHandler for SessionServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "session-mcp provides direct access to the session store. Use named tools for session worktree check-in, lookup, query, move, and transcript peeking. Call session_capabilities to discover the durable-workflow lifecycle (runtime_init -> pin/view -> workflow_* -> render_* -> handoff/finish) and the legal workflow enum values."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// ── Server startup ────────────────────────────────────────────────────────────

pub async fn run_mcp_server(
    store_root: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = SessionServer::new(store_root);

    tracing::info!("Starting session-mcp server on stdio (direct store access)");

    let service = server.serve(stdio()).await.inspect_err(|err| {
        eprintln!("Server error: {err:?}");
    })?;

    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
#[path = "server/tests.rs"]
mod tests;

#[path = "server/workflow_inputs.rs"]
mod workflow_inputs;
#[allow(unused_imports)]
pub use workflow_inputs::*;

#[path = "server/governance_inputs.rs"]
mod governance_inputs;
#[allow(unused_imports)]
pub use governance_inputs::*;

#[path = "server/lifecycle_inputs.rs"]
mod lifecycle_inputs;
#[allow(unused_imports)]
pub use lifecycle_inputs::*;

#[path = "server/observation_inputs.rs"]
mod observation_inputs;
#[allow(unused_imports)]
pub use observation_inputs::*;

#[path = "server/workflow_tools.rs"]
mod workflow_tools;

#[path = "server/lifecycle_tools.rs"]
mod lifecycle_tools;

#[path = "server/observation_tools.rs"]
mod observation_tools;

#[path = "server/governance_tools.rs"]
mod governance_tools;

#[path = "server/move_tools.rs"]
mod move_tools;
