use super::*;
use std::collections::BTreeMap;

#[test]
fn composed_router_retains_every_tool_and_its_schema() {
    let expected = [
        SessionServer::session_capabilities_tool_attr(),
        SessionServer::session_runtime_init_tool_attr(),
        SessionServer::session_runtime_resume_tool_attr(),
        SessionServer::session_runtime_pin_tool_attr(),
        SessionServer::session_runtime_unpin_tool_attr(),
        SessionServer::session_runtime_view_tool_attr(),
        SessionServer::session_runtime_render_instructions_tool_attr(),
        SessionServer::session_workflow_add_node_tool_attr(),
        SessionServer::session_workflow_add_nodes_tool_attr(),
        SessionServer::session_workflow_add_edge_tool_attr(),
        SessionServer::session_workflow_add_edges_tool_attr(),
        SessionServer::session_workflow_set_status_tool_attr(),
        SessionServer::session_workflow_update_node_tool_attr(),
        SessionServer::session_workflow_remove_node_tool_attr(),
        SessionServer::session_workflow_promote_tool_attr(),
        SessionServer::session_workflow_render_terminal_tool_attr(),
        SessionServer::session_workflow_render_mermaid_tool_attr(),
        SessionServer::session_handoff_tool_attr(),
        SessionServer::session_terminal_create_tool_attr(),
        SessionServer::session_terminal_status_tool_attr(),
        SessionServer::session_terminal_peek_tool_attr(),
        SessionServer::session_terminal_close_tool_attr(),
        SessionServer::session_finish_tool_attr(),
        SessionServer::session_check_in_tool_attr(),
        SessionServer::session_unassign_tool_attr(),
        SessionServer::session_lookup_tool_attr(),
        SessionServer::session_query_tool_attr(),
        SessionServer::session_sessions_for_ticket_tool_attr(),
        SessionServer::session_peek_range_tool_attr(),
        SessionServer::session_peek_skeleton_tool_attr(),
        SessionServer::session_tool_metrics_tool_attr(),
        SessionServer::session_subagent_rollups_tool_attr(),
        SessionServer::session_grant_create_tool_attr(),
        SessionServer::session_grant_list_tool_attr(),
        SessionServer::session_grant_revoke_tool_attr(),
        SessionServer::session_escalation_create_tool_attr(),
        SessionServer::session_escalation_list_tool_attr(),
        SessionServer::session_escalation_get_tool_attr(),
        SessionServer::session_escalation_resolve_tool_attr(),
        SessionServer::session_move_preflight_tool_attr(),
        SessionServer::session_move_apply_tool_attr(),
        SessionServer::session_move_resume_tool_attr(),
        SessionServer::session_move_rollback_tool_attr(),
    ];
    let by_name = |tools: Vec<Tool>| {
        tools
            .into_iter()
            .map(|tool| (tool.name.to_string(), serde_json::to_value(tool).unwrap()))
            .collect::<BTreeMap<_, _>>()
    };
    let actual = SessionServer::tool_router().list_all();
    assert_eq!(actual.len(), expected.len());
    assert_eq!(by_name(actual), by_name(expected.into()));
}
