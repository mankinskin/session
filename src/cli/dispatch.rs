#[allow(unused_imports)]
use super::*;

pub(super) fn dispatch(
    config: &SessionStoreConfig,
    command: SessionCommand,
) -> Result<Value, CliRunError> {
    match command {
        SessionCommand::Init(args) => {
            let result = config.init_runtime_context(SessionRuntimeInitRequest {
                session_id: Some(args.session_id),
                predecessor_run_id: args.predecessor_run_id,
                force_new_run: args.force_new_run,
            })?;
            let handle = result.context.session_id.clone();
            to_value_with_handle(&handle, &result)
        }
        SessionCommand::Resume(args) => {
            let result =
                config.resume_workspace_context(&args.session_id, &args.predecessor_run_id)?;
            let handle = result.context.session_id.clone();
            to_value_with_handle(&handle, &result)
        }
        SessionCommand::Pin(args) => {
            let context = config.pin_runtime_entity(
                &args.session_id,
                &args.entity_urn,
                args.relation,
                args.reason,
            )?;
            to_value(&context)
        }
        SessionCommand::Unpin(args) => {
            let context = config.unpin_runtime_entity(&args.session_id, &args.entity_urn)?;
            to_value(&context)
        }
        SessionCommand::View(args) => {
            let view = config.view_runtime_context(&args.session_id)?;
            to_value(&view)
        }
        SessionCommand::RenderInstructions(args) => {
            let render = config.render_pinned_rule_instructions(&args.session_id)?;
            to_value(&json!({"render": render}))
        }
        SessionCommand::Workflow { command } => handle_workflow_command(&config, command),
        SessionCommand::WorkflowAddNode(args) => {
            handle_workflow_command(&config, WorkflowCommand::AddNode(args))
        }
        SessionCommand::WorkflowAddNodes(args) => {
            handle_workflow_command(&config, WorkflowCommand::AddNodes(args))
        }
        SessionCommand::WorkflowAddEdge(args) => {
            handle_workflow_command(&config, WorkflowCommand::AddEdge(args))
        }
        SessionCommand::WorkflowAddEdges(args) => {
            handle_workflow_command(&config, WorkflowCommand::AddEdges(args))
        }
        SessionCommand::WorkflowSetStatus(args) => {
            handle_workflow_command(&config, WorkflowCommand::SetStatus(args))
        }
        SessionCommand::WorkflowPromote(args) => {
            handle_workflow_command(&config, WorkflowCommand::Promote(args))
        }
        SessionCommand::WorkflowRenderTerminal(args) => {
            handle_workflow_command(&config, WorkflowCommand::RenderTerminal(args))
        }
        SessionCommand::WorkflowRenderMermaid(args) => {
            handle_workflow_command(&config, WorkflowCommand::RenderMermaid(args))
        }
        SessionCommand::Handoff(args) => {
            let package = handoff_package_from_args(&args)?;
            let result = config.create_handoff_result(
                &args.session_id,
                package,
                parse_validation_gates(args.validation_json.as_deref())?,
                None,
            )?;
            to_value(&result)
        }
        SessionCommand::Finish(args) => {
            let result = config.finish_workflow(
                &args.session_id,
                parse_validation_gates(args.validation_json.as_deref())?,
                args.deferred_optional_node_ids,
                None,
            )?;
            to_value(&result)
        }
        SessionCommand::CheckIn(args) => {
            let receipt = config.check_in_worktree(SessionWorktreeCheckInRequest {
                session_id: args.session_id,
                owner_id: args.owner_id,
                ticket_id: args.ticket_id,
                worktree_path: args.worktree_path,
                branch: args.branch,
                predecessor_session_id: args.predecessor_session_id,
            })?;
            to_value(&receipt)
        }
        SessionCommand::Unassign(args) => {
            let receipt = config.unassign_worktree(SessionWorktreeUnassignRequest {
                session_id: args.session_id,
                owner_id: args.owner_id,
                ticket_id: args.ticket_id,
                expected_worktree_path: args.expected_worktree_path,
                expected_branch: args.expected_branch,
                reason: args.reason,
            })?;
            to_value(&receipt)
        }
        SessionCommand::Lookup(args) => {
            let receipt = config.lookup_worktree(&args.session_id)?;
            to_value(&receipt)
        }
        SessionCommand::Query(args) => {
            let query = SessionQuery {
                session_id_prefix: args.session_id_prefix,
                conversation_id: args.conversation_id,
                agent_id: args.agent_id,
                text: args.text,
                limit: args.limit,
            };
            let result = config.query_sessions(&query)?;
            to_value(&json!({
                "count": result.sessions.len(),
                "sessions": result.sessions,
                "diagnostics": result.diagnostics,
            }))
        }
        SessionCommand::SessionsForTicket(args) => {
            let strength = parse_relation_strength(&args.strength)?;
            let sessions = config.sessions_for_ticket(&args.ticket_id, strength)?;
            to_value(&json!({
                "count": sessions.len(),
                "sessions": sessions,
            }))
        }
        SessionCommand::BackfillTicketLinks(args) => {
            let report = config.backfill_ticket_links(args.write)?;
            to_value(&report)
        }
        SessionCommand::Move(args) => move_command(config, args),
        SessionCommand::PeekRange(args) => {
            let range = config.peek_range(&args.session_id, args.start, args.end)?;
            to_value(&range)
        }
        SessionCommand::PeekSkeleton(args) => {
            let skeleton = config.peek_skeleton(&args.session_id, args.preview_chars)?;
            to_value(&skeleton)
        }
        SessionCommand::PeekPromptPack(args) => {
            let pack = config.peek_prompt_pack(
                &args.session_id,
                PromptPackOptions {
                    preview_chars: args.preview_chars,
                    summarize_threshold_chars: args.summarize_threshold_chars,
                },
            )?;
            to_value(&pack)
        }
        SessionCommand::TerminalCreate(args) => {
            let manifest = config.create_terminal_observer(SessionTerminalCreateRequest {
                session_id: args.session_id,
                label: args.label,
                cwd: args.cwd,
            })?;
            to_value(&manifest)
        }
        SessionCommand::TerminalAppendOutput(args) => {
            let event =
                config.append_terminal_output(&args.session_id, &args.terminal_id, args.output)?;
            to_value(&event)
        }
        SessionCommand::TerminalStatus(args) => {
            let manifest = config.terminal_status(&args.session_id, &args.terminal_id)?;
            to_value(&manifest)
        }
        SessionCommand::TerminalPeek(args) => {
            let result = config.peek_terminal_output(
                &args.session_id,
                &args.terminal_id,
                args.offset,
                args.limit,
            )?;
            to_value(&result)
        }
        SessionCommand::TerminalClose(args) => {
            let manifest = config.close_terminal_observer(&args.session_id, &args.terminal_id)?;
            to_value(&manifest)
        }
        SessionCommand::ToolMetrics(args) => {
            let window = ToolMetricsWindow {
                max_age_days: args.days,
                max_sessions: args.max_sessions,
            };
            let report = config.tool_metrics(window)?;

            // If export path specified, write rollup
            if let Some(export_path) = args.export {
                use session_api::tool_metrics::write_rollup;
                write_rollup(&export_path, report.clone())?;
            }

            to_value(&report)
        }
        SessionCommand::SubagentRollups(args) => {
            let rollups = config.subagent_rollups(&args.session_id)?;
            to_value(&rollups)
        }
        SessionCommand::DelegationCost(args) => {
            let report =
                config.delegation_cost_report(SessionAuditSelector::SessionId(args.session_id))?;
            to_value(&report)
        }
        SessionCommand::Grant { command } => handle_grant_command(config, command),
        SessionCommand::Escalation { command } => handle_escalation_command(config, command),
    }
}
