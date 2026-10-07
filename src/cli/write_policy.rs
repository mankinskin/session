use super::{EscalationCommand, GrantCommand, SessionCommand, WorkflowCommand};

pub(super) fn command_writes_store(command: &SessionCommand) -> bool {
    match command {
        SessionCommand::Init(_)
        | SessionCommand::Resume(_)
        | SessionCommand::Pin(_)
        | SessionCommand::Unpin(_)
        | SessionCommand::Handoff(_)
        | SessionCommand::Finish(_)
        | SessionCommand::CheckIn(_)
        | SessionCommand::TerminalCreate(_)
        | SessionCommand::TerminalAppendOutput(_)
        | SessionCommand::TerminalClose(_) => true,
        SessionCommand::Workflow { command } => match command {
            WorkflowCommand::AddNode(_)
            | WorkflowCommand::AddNodes(_)
            | WorkflowCommand::AddEdge(_)
            | WorkflowCommand::AddEdges(_)
            | WorkflowCommand::SetStatus(_)
            | WorkflowCommand::Promote(_) => true,
            WorkflowCommand::RenderTerminal(_) | WorkflowCommand::RenderMermaid(_) => false,
        },
        SessionCommand::WorkflowAddNode(_)
        | SessionCommand::WorkflowAddNodes(_)
        | SessionCommand::WorkflowAddEdge(_)
        | SessionCommand::WorkflowAddEdges(_)
        | SessionCommand::WorkflowSetStatus(_)
        | SessionCommand::WorkflowPromote(_) => true,
        SessionCommand::WorkflowRenderTerminal(_) | SessionCommand::WorkflowRenderMermaid(_) => {
            false
        }
        SessionCommand::BackfillTicketLinks(args) => args.write,
        SessionCommand::Move(args) => {
            args.resume.is_some() || args.rollback.is_some() || !args.dry_run
        }
        SessionCommand::Grant { command } => match command {
            GrantCommand::Create(_) | GrantCommand::Revoke(_) => true,
            GrantCommand::List => false,
        },
        SessionCommand::Escalation { command } => match command {
            EscalationCommand::Create(_) | EscalationCommand::Resolve(_) => true,
            EscalationCommand::List(_) | EscalationCommand::Get(_) => false,
        },
        SessionCommand::ToolMetrics(args) => args.export.is_some(),
        SessionCommand::View(_)
        | SessionCommand::RenderInstructions(_)
        | SessionCommand::Lookup(_)
        | SessionCommand::Query(_)
        | SessionCommand::SessionsForTicket(_)
        | SessionCommand::PeekRange(_)
        | SessionCommand::PeekSkeleton(_)
        | SessionCommand::PeekPromptPack(_)
        | SessionCommand::TerminalStatus(_)
        | SessionCommand::TerminalPeek(_)
        | SessionCommand::SubagentRollups(_)
        | SessionCommand::DelegationCost(_) => false,
    }
}
