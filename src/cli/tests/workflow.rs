#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn parses_canonical_nested_workflow_add_node() {
    let cli = parse_cli_from([
        "session",
        "workflow",
        "add-node",
        "--session-id",
        "77777777-7777-4777-8777-777777777777",
        "--kind",
        "action",
        "--requirement",
        "required",
        "--title",
        "do the thing",
    ])
    .expect("parse nested workflow add-node");

    match cli.command {
        SessionCommand::Workflow {
            command: WorkflowCommand::AddNode(args),
        } => {
            assert_eq!(args.session_id, "77777777-7777-4777-8777-777777777777");
            assert_eq!(args.kind, "action");
            assert_eq!(args.requirement, "required");
            assert_eq!(args.title, "do the thing");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
pub(super) fn workflow_batch_commands_dispatch_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let config = SessionStoreConfig::new(dir.path().join(".session"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap();
    let session_id = init.context.session_id;

    let error = handle_workflow_command(
            &config,
            WorkflowCommand::AddNodes(WorkflowAddNodesArgs {
                session_id: session_id.clone(),
                nodes_json: r#"[
                    {"node_id":"a","kind":"task","requirement":"optional","title":"a"},
                    {"node_id":"bad","kind":"review-criterion","requirement":"optional","title":"bad"}
                ]"#
                .to_string(),
            }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("nodes[1]"));
    assert!(
        config
            .read_runtime_context(&session_id)
            .unwrap()
            .workflow
            .nodes
            .is_empty()
    );

    let nodes = handle_workflow_command(
        &config,
        WorkflowCommand::AddNodes(WorkflowAddNodesArgs {
            session_id: session_id.clone(),
            nodes_json: r#"[
                    {"node_id":"a","kind":"task","requirement":"optional","title":"a"},
                    {"node_id":"b","kind":"task","requirement":"optional","title":"b"}
                ]"#
            .to_string(),
        }),
    )
    .unwrap();
    assert_eq!(nodes["workflow"]["nodes"].as_array().unwrap().len(), 2);

    let edges = handle_workflow_command(
        &config,
        WorkflowCommand::AddEdges(WorkflowAddEdgesArgs {
            session_id,
            edges_json: r#"[
                    {"from":"a","to":"b","kind":"depends-on"},
                    {"from":"b","to":"a","kind":"order"}
                ]"#
            .to_string(),
        }),
    )
    .unwrap();
    assert_eq!(edges["workflow"]["edges"].as_array().unwrap().len(), 2);
}

#[test]
pub(super) fn parses_render_instructions() {
    let cli = parse_cli_from([
        "session",
        "render-instructions",
        "--session-id",
        "77777777-7777-4777-8777-777777777777",
    ])
    .expect("parse render-instructions");

    match cli.command {
        SessionCommand::RenderInstructions(args) => {
            assert_eq!(args.session_id, "77777777-7777-4777-8777-777777777777");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
pub(super) fn render_instructions_dispatches_focused_rule_set() {
    let dir = tempfile::tempdir().unwrap();
    let session_root = dir.path().join(".session");
    let config = SessionStoreConfig::new(&session_root);
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some("22222222-2222-4222-8222-222222222222".to_string()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap();
    let mut rule_store = rule_api::RuleStore::open_or_init(&dir.path().join(".rule")).unwrap();
    let rule = rule_api::RuleManifest::new(
        "session/cli/render",
        "CLI render",
        ".instructions",
        "cli-render",
        "Pinned CLI guidance.",
    );
    let rule_id = rule_store.create(&rule, None).unwrap();
    config
        .pin_runtime_entity(
            &init.context.session_id,
            &format!("ce://default/rules/{rule_id}"),
            None,
            None,
        )
        .unwrap();

    let payload = dispatch(
        &config,
        SessionCommand::RenderInstructions(ViewArgs {
            session_id: init.context.session_id,
        }),
    )
    .unwrap();
    assert!(
        payload["render"]
            .as_str()
            .unwrap()
            .contains("Pinned CLI guidance.")
    );
}

#[test]
pub(super) fn parses_flat_workflow_add_node_alias() {
    let cli = parse_cli_from([
        "session",
        "workflow-add-node",
        "--session-id",
        "77777777-7777-4777-8777-777777777777",
        "--kind",
        "action",
        "--requirement",
        "required",
        "--title",
        "do the thing",
    ])
    .expect("parse flat workflow-add-node alias");

    match cli.command {
        SessionCommand::WorkflowAddNode(args) => {
            assert_eq!(args.session_id, "77777777-7777-4777-8777-777777777777");
            assert_eq!(args.title, "do the thing");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
pub(super) fn parses_canonical_nested_workflow_render_terminal() {
    let cli = parse_cli_from([
        "session",
        "workflow",
        "render-terminal",
        "--session-id",
        "77777777-7777-4777-8777-777777777777",
    ])
    .expect("parse nested workflow render-terminal");

    assert!(matches!(
        cli.command,
        SessionCommand::Workflow {
            command: WorkflowCommand::RenderTerminal(_),
        }
    ));
}
