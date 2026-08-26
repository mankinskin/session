//! Classifies MCP tool names as read-only or mutating, for workspace-
//! resolution gating (see [`super::workspace_resolution`]).

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolAccess {
    Read,
    Mutation,
}

const KNOWN_READ_TOOLS: &[&str] = &[
    "fs_list_dir",
    "fs_stat",
    "get_part",
    "get_ticket",
    "get_ticket_description",
    "health",
    "health_check",
    "list_edges",
    "list_parts",
    "list_tickets",
    "list_workspaces",
    "next_tickets",
    "peek_count",
    "peek_grep",
    "peek_read",
    "peek_skeleton",
    "session_capabilities",
    "session_escalation_get",
    "session_escalation_list",
    "session_grant_list",
    "session_lookup",
    "session_peek_range",
    "session_peek_skeleton",
    "session_query",
    "session_runtime_render_instructions",
    "session_runtime_view",
    "session_sessions_for_ticket",
    "session_subagent_rollups",
    "session_terminal_peek",
    "session_terminal_status",
    "session_tool_metrics",
    "session_workflow_render_mermaid",
    "session_workflow_render_terminal",
    "spec_get",
    "spec_health",
    "spec_list",
    "spec_refs_validate",
    "spec_search",
    "spec_section_get",
    "spec_section_list",
    "spec_tree",
    "subgraph",
    "test_get_execution",
    "test_get_spec",
    "test_list_executions",
    "test_list_specs",
    "ticket_capabilities",
    "topgraph",
    "workflow",
];

/// Classifies MCP operations at the routing boundary. Unrecognized names are
/// mutations so newly added tools remain protected until explicitly reviewed.
pub(crate) fn tool_access(tool: &str) -> ToolAccess {
    if KNOWN_READ_TOOLS.contains(&tool) {
        ToolAccess::Read
    } else {
        ToolAccess::Mutation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_access_allows_registered_reads_and_guards_writes() {
        for tool in KNOWN_READ_TOOLS {
            assert_eq!(tool_access(tool), ToolAccess::Read, "{tool}");
        }

        for tool in [
            "fs_copy_file",
            "fs_delete_dir",
            "fs_delete_file",
            "fs_move_file",
            "fs_rename_file",
            "session_check_in",
            "update_ticket",
            "unknown_tool",
        ] {
            assert_eq!(tool_access(tool), ToolAccess::Mutation, "{tool}");
        }
    }
}
