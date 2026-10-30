#[allow(unused_imports)]
use super::*;

pub(super) fn render_handoff_record_terminal(record: &SessionHandoffRecord) -> String {
    let mut lines = Vec::new();
    lines.push(format!("handoff {}", record.handoff_id));
    lines.push(format!("session_id: {}", record.session_id));
    lines.push(format!("outgoing_run_id: {}", record.outgoing_run_id));
    lines.push(format!("resume: {}", record.resume_command));
    if !record.objective.is_empty() {
        lines.push(format!("objective: {}", record.objective));
    }
    if !record.target_tickets.is_empty() {
        lines.push(format!(
            "target_tickets: {}",
            record
                .target_tickets
                .iter()
                .map(|ticket| ticket.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !record.target_files.is_empty() {
        lines.push(format!("target_files: {}", record.target_files.join(", ")));
    }
    if !record.open_escalations.is_empty() {
        lines.push(format!(
            "open_escalations: {}",
            record.open_escalations.join(", ")
        ));
        lines.push("implementation_ready: false".to_string());
    } else if !record.objective.is_empty() {
        lines.push("implementation_ready: true".to_string());
    }
    lines.push("workflow:".to_string());
    lines.push(format!("  nodes: {}", record.workflow.workflow.nodes.len()));
    lines.push(format!("  edges: {}", record.workflow.workflow.edges.len()));
    let blocked = record
        .workflow
        .workflow
        .nodes
        .iter()
        .filter(|node| node.status != SessionWorkflowNodeStatus::Done)
        .count();
    lines.push(format!("  not_done_nodes: {}", blocked));
    for pin in &record.pinned_entities {
        lines.push(format!(
            "pin {} {}",
            pin.urn,
            format!("{:?}", pin.kind).to_lowercase()
        ));
    }
    for gate in &record.validation {
        lines.push(format!(
            "validation {} required={} outcome={}",
            gate.validation_spec_id,
            gate.required,
            gate.outcome.as_deref().unwrap_or("-")
        ));
    }
    for diag in &record.workflow.diagnostics {
        lines.push(format!(
            "diag {} {} {}",
            diag.node_id, diag.code, diag.message
        ));
    }
    lines.join("\n")
}

pub(super) fn render_handoff_record_markdown(
    record: &SessionHandoffRecord,
    ticket_store: Option<&TicketStore>,
) -> String {
    let mut sections = Vec::new();

    // Header
    sections.push(format!("# Handoff: {}", record.handoff_id));
    sections.push(String::new());

    if !record.higher_level_objective.is_empty() {
        sections.push(linkify_handoff_prose(
            &record.higher_level_objective,
            ticket_store,
        ));
        sections.push(String::new());
    }

    if !record.upward_context.is_empty() {
        sections.push("## Upward Context".to_string());
        let mut breadcrumb = record
            .upward_context
            .iter()
            .map(render_handoff_upward_context_entry)
            .collect::<Vec<_>>();
        if !record.target_tickets.is_empty() {
            breadcrumb.push(
                record
                    .target_tickets
                    .iter()
                    .map(|ticket| {
                        resolve_handoff_ticket_display(record, ticket, ticket_store).reference
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        sections.push(breadcrumb.join(" -> "));
        sections.push(String::new());
    }

    // Summary section
    sections.push("## Summary".to_string());
    sections.push(format!("- **Workspace Session**: `{}`", record.session_id));
    sections.push(format!("- **Outgoing Run**: `{}`", record.outgoing_run_id));
    sections.push(format!("- **Created**: {}", record.created_at.to_rfc3339()));
    if !record.objective.is_empty() {
        sections.push(format!("- **Objective**: {}", record.objective));
    }
    let implementation_ready = !record.objective.is_empty() && record.open_escalations.is_empty();
    sections.push(format!(
        "- **Implementation Ready**: {}",
        implementation_ready
    ));
    sections.push(String::new());

    // Resume command
    sections.push("## Resume Command".to_string());
    sections.push("```bash".to_string());
    sections.push(record.resume_command.clone());
    sections.push("```".to_string());
    sections.push(String::new());

    // Target Tickets
    if !record.target_tickets.is_empty() {
        sections.push("## Target Tickets".to_string());
        sections.push("| Ticket | What it does | Why |".to_string());
        sections.push("| --- | --- | --- |".to_string());
        for ticket in &record.target_tickets {
            let display = resolve_handoff_ticket_display(record, ticket, ticket_store);
            sections.push(format!(
                "| {} | {} | {} |",
                display.reference,
                markdown_table_cell(&display.what),
                markdown_table_cell(&linkify_handoff_prose(&ticket.why, ticket_store)),
            ));
        }
        sections.push(String::new());
    }

    // Target Files
    if !record.target_files.is_empty() {
        sections.push("## Target Files".to_string());
        for file in &record.target_files {
            sections.push(format!("- `{}`", file));
        }
        sections.push(String::new());
    }

    // Decisions
    if !record.decisions.is_empty() {
        sections.push("## Decisions".to_string());
        for decision in &record.decisions {
            sections.push(format!(
                "- {}",
                linkify_handoff_prose(decision, ticket_store)
            ));
        }
        sections.push(String::new());
    }

    // Non-Goals
    if !record.non_goals.is_empty() {
        sections.push("## Non-Goals".to_string());
        for non_goal in &record.non_goals {
            sections.push(format!(
                "- {}",
                linkify_handoff_prose(non_goal, ticket_store)
            ));
        }
        sections.push(String::new());
    }

    // Context Anchors
    if !record.context_anchors.is_empty() {
        sections.push("## Context Anchors".to_string());
        for anchor in &record.context_anchors {
            sections.push(format!("- {}", linkify_handoff_prose(anchor, ticket_store)));
        }
        sections.push(String::new());
    }

    // Open Escalations
    if !record.open_escalations.is_empty() {
        sections.push("## ⚠️ Open Escalations".to_string());
        for escalation in &record.open_escalations {
            sections.push(format!("- {}", escalation));
        }
        sections.push(String::new());
    }

    // Risk Notes
    if let Some(ref risk_notes) = record.risk_notes {
        sections.push("## Risk Notes".to_string());
        sections.push(linkify_handoff_prose(risk_notes, ticket_store));
        sections.push(String::new());
    }

    // Workflow
    sections.push("## Workflow".to_string());
    sections.push(format!(
        "- **Nodes**: {}",
        record.workflow.workflow.nodes.len()
    ));
    sections.push(format!(
        "- **Edges**: {}",
        record.workflow.workflow.edges.len()
    ));
    let not_done = record
        .workflow
        .workflow
        .nodes
        .iter()
        .filter(|node| node.status != SessionWorkflowNodeStatus::Done)
        .count();
    sections.push(format!("- **Not Done**: {}", not_done));
    if !record.workflow.workflow.nodes.is_empty() {
        // Blank line required so Markdown renderers don't treat the fence as list continuation.
        sections.push(String::new());
        sections.push("```mermaid".to_string());
        sections.push(render_workflow_mermaid(
            &record.workflow.workflow,
            &record.workflow.resolutions,
        ));
        sections.push("```".to_string());
    }
    sections.push(String::new());

    // Pinned Entities
    if !record.pinned_entities.is_empty() {
        sections.push("## Pinned Entities".to_string());
        for pin in &record.pinned_entities {
            sections.push(format!(
                "- `{}` ({})",
                pin.urn,
                format!("{:?}", pin.kind).to_lowercase()
            ));
        }
        sections.push(String::new());
    }

    // Validation
    if !record.validation.is_empty() {
        sections.push("## Validation".to_string());
        for gate in &record.validation {
            let outcome = gate.outcome.as_deref().unwrap_or("-");
            let required = if gate.required {
                "required"
            } else {
                "optional"
            };
            sections.push(format!(
                "- `{}`: {} ({})",
                gate.validation_spec_id, outcome, required
            ));
        }
        sections.push(String::new());
    }

    // Diagnostics
    if !record.workflow.diagnostics.is_empty() {
        sections.push("## Diagnostics".to_string());
        for diag in &record.workflow.diagnostics {
            sections.push(format!(
                "- **{}** [{}]: {}",
                diag.node_id, diag.code, diag.message
            ));
        }
        sections.push(String::new());
    }

    sections.join("\n")
}

pub(super) fn render_handoff_upward_context_entry(
    entry: &crate::SessionHandoffUpwardContextEntry,
) -> String {
    let title = parse_entity_urn(&entry.entity_urn)
        .ok()
        .filter(|parsed| parsed.kind == SessionPinnedEntityKind::Ticket)
        .map(|parsed| handoff_ticket_reference(&parsed.entity_id, &entry.title))
        .unwrap_or_else(|| entry.title.clone());
    format!("{} ({})", title, format!("{:?}", entry.role).to_lowercase())
}

pub(super) fn markdown_table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', "<br>")
}
