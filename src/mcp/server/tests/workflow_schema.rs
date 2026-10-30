#[allow(unused_imports)]
use super::*;

// ── T-SCHEMA: workflow mutation schemas advertise legal enum values ──────

#[test]
pub(super) fn workflow_add_node_schema_advertises_kind_and_requirement_enums() {
    let schema = rmcp::schemars::schema_for!(WorkflowAddNodeInput);
    let json = serde_json::to_string(&schema).expect("schema json");
    for value in ["ticket", "validation", "spec", "task"] {
        assert!(
            json.contains(&format!("\"{value}\"")),
            "schema must advertise node kind `{value}`: {json}"
        );
    }
    for value in ["required", "optional"] {
        assert!(
            json.contains(&format!("\"{value}\"")),
            "schema must advertise requirement `{value}`"
        );
    }
}

#[test]
pub(super) fn workflow_edge_and_status_schemas_advertise_enums() {
    let edge = serde_json::to_string(&rmcp::schemars::schema_for!(WorkflowAddEdgeInput)).unwrap();
    assert!(edge.contains("\"depends-on\"") && edge.contains("\"order\""));

    let status =
        serde_json::to_string(&rmcp::schemars::schema_for!(WorkflowSetStatusInput)).unwrap();
    for value in ["pending", "in-progress", "blocked", "done", "deferred"] {
        assert!(
            status.contains(&format!("\"{value}\"")),
            "status schema must advertise `{value}`"
        );
    }
}

/// The advertised schema enums must match the `session-api` enums exactly.
#[test]
pub(super) fn workflow_enum_parity_with_session_api() {
    // Behavioral + descriptive node kinds serialize to the advertised set.
    let kinds = [
        (SessionWorkflowNodeKind::Ticket, "ticket"),
        (SessionWorkflowNodeKind::Validation, "validation"),
        (SessionWorkflowNodeKind::Spec, "spec"),
        (SessionWorkflowNodeKind::Task, "task"),
    ];
    for (kind, label) in kinds {
        assert_eq!(
            serde_json::to_string(&kind).unwrap(),
            format!("\"{label}\"")
        );
        assert_eq!(parse_node_kind(label).unwrap(), kind);
    }
    // Legacy aliases round-trip into the generic Task bucket.
    for alias in ["action", "decision", "checkpoint"] {
        assert_eq!(
            parse_node_kind(alias).unwrap(),
            SessionWorkflowNodeKind::Task
        );
    }
}

// ── T-ERRCONTRACT: rejections enumerate the allowed values ───────────────

#[test]
pub(super) fn invalid_workflow_values_report_allowed_set() {
    let kind_err = parse_node_kind("tickett").unwrap_err();
    assert!(kind_err.message.contains("ticket"));
    assert!(kind_err.message.contains("validation"));
    assert!(kind_err.message.contains("spec"));
    assert!(kind_err.message.contains("task"));
    assert!(
        kind_err
            .message
            .contains("kind=task with category=\"<your-label>\"")
    );

    let edge_err = parse_edge_kind("nope").unwrap_err();
    assert!(edge_err.message.contains("depends-on"));
    assert!(edge_err.message.contains("order"));
    assert!(edge_err.message.contains("did you mean kind=depends-on?"));

    let status_err = parse_node_status("doing").unwrap_err();
    assert!(status_err.message.contains("in-progress"));
    assert!(status_err.message.contains("deferred"));
    assert!(
        status_err
            .message
            .contains("did you mean status=in-progress?")
    );

    let req_err = parse_requirement("maybe").unwrap_err();
    assert!(req_err.message.contains("required"));
    assert!(req_err.message.contains("optional"));
    assert!(
        req_err
            .message
            .contains("did you mean requirement=required?")
    );
}

#[tokio::test]
pub(super) async fn workflow_batch_tools_are_atomic_and_report_array_index() {
    let dir = tempdir().unwrap();
    let session_root = dir.path().join(".session");
    let workspace = session_root.display().to_string();
    let server = SessionServer::new(session_root.clone());
    let config = server.config_for_workspace(&workspace).unwrap();
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some("33333333-3333-4333-8333-333333333333".to_string()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap();
    let session_id = init.context.session_id;
    let node = |node_id: &str, kind: &str| WorkflowNodeDraftInput {
        node_id: Some(node_id.to_string()),
        kind: kind.to_string(),
        requirement: "optional".to_string(),
        title: node_id.to_string(),
        ticket_urn: None,
        spec_urn: None,
        anchor_urn: None,
        category: None,
        cached_ticket_title: None,
        validation_spec_id: None,
    };

    let node_error = server
        .session_workflow_add_nodes(Parameters(WorkflowAddNodesInput {
            workspace: workspace.clone(),
            session_id: session_id.clone(),
            nodes: vec![node("a", "task"), node("bad", "review-criterion")],
        }))
        .await
        .unwrap_err();
    assert!(node_error.message.contains("nodes[1]"));
    assert!(
        node_error
            .message
            .contains("kind=task with category=\"<your-label>\"")
    );
    assert!(
        config
            .read_runtime_context(&session_id)
            .unwrap()
            .workflow
            .nodes
            .is_empty()
    );

    let node_result = server
        .session_workflow_add_nodes(Parameters(WorkflowAddNodesInput {
            workspace: workspace.clone(),
            session_id: session_id.clone(),
            nodes: vec![node("a", "task"), node("b", "task")],
        }))
        .await
        .unwrap();
    assert_eq!(
        extract_json(node_result)["workflow"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let edge = |from: &str, to: &str, kind: &str| WorkflowEdgeDraftInput {
        from: from.to_string(),
        to: to.to_string(),
        kind: kind.to_string(),
    };
    let edge_error = server
        .session_workflow_add_edges(Parameters(WorkflowAddEdgesInput {
            workspace: workspace.clone(),
            session_id: session_id.clone(),
            edges: vec![edge("a", "b", "depends-on"), edge("b", "a", "related-to")],
        }))
        .await
        .unwrap_err();
    assert!(edge_error.message.contains("edges[1]"));
    assert!(edge_error.message.contains("did you mean kind=depends-on?"));
    assert!(
        config
            .read_runtime_context(&session_id)
            .unwrap()
            .workflow
            .edges
            .is_empty()
    );

    let edge_result = server
        .session_workflow_add_edges(Parameters(WorkflowAddEdgesInput {
            workspace,
            session_id,
            edges: vec![edge("a", "b", "depends-on"), edge("b", "a", "order")],
        }))
        .await
        .unwrap();
    assert_eq!(
        extract_json(edge_result)["workflow"]["edges"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

// ── T-CATALOG: session lifecycle + enums are discoverable ────────────────

#[tokio::test]
pub(super) async fn capabilities_lists_session_lifecycle_and_enums() {
    let dir = tempdir().unwrap();
    let session_root = dir.path().join(".session");
    let server = SessionServer::new(session_root.clone());

    let result = server
        .session_capabilities(Parameters(CapabilitiesInput::default()))
        .await
        .expect("capabilities");
    let catalog = extract_json(result);

    assert_eq!(catalog["surface"], "session-mcp");
    assert_eq!(catalog["handle"]["field"], "session_id");

    let steps = catalog["lifecycle"]["steps"]
        .as_array()
        .expect("lifecycle steps");
    let tools: Vec<&str> = steps.iter().filter_map(|s| s["tool"].as_str()).collect();
    for expected in [
        "session_runtime_init",
        "session_workflow_add_node",
        "session_workflow_render_terminal",
        "session_finish",
    ] {
        assert!(
            tools.contains(&expected),
            "catalog lifecycle must list {expected}"
        );
    }

    let behavioral = catalog["enums"]["workflow_node_kind"]["behavioral"]
        .as_array()
        .expect("behavioral kinds");
    let behavioral: Vec<&str> = behavioral.iter().filter_map(|v| v.as_str()).collect();
    assert!(behavioral.contains(&"ticket"));
    assert!(behavioral.contains(&"validation"));
    assert!(behavioral.contains(&"spec"));
}
