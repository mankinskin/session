#[allow(unused_imports)]
use super::*;

pub(super) fn sort_workflow_graph(graph: &mut crate::SessionWorkflowGraph) {
    graph
        .nodes
        .sort_by(|left, right| left.node_id.cmp(&right.node_id));
    graph.edges.sort_by(|left, right| {
        left.from
            .cmp(&right.from)
            .then_with(|| left.to.cmp(&right.to))
            .then_with(|| format!("{:?}", left.kind).cmp(&format!("{:?}", right.kind)))
    });
}

pub(super) fn workflow_status_label(status: SessionWorkflowNodeStatus) -> &'static str {
    match status {
        SessionWorkflowNodeStatus::Pending => "pending",
        SessionWorkflowNodeStatus::InProgress => "in-progress",
        SessionWorkflowNodeStatus::Blocked => "blocked",
        SessionWorkflowNodeStatus::Done => "done",
        SessionWorkflowNodeStatus::Deferred => "deferred",
    }
}

pub(super) fn mermaid_node_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('n');
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

pub(super) fn escape_mermaid_label(label: &str) -> String {
    label
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

pub(super) fn render_workflow_mermaid(
    workflow: &crate::SessionWorkflowGraph,
    resolutions: &[crate::SessionWorkflowNodeResolution],
) -> String {
    let live_states = resolutions
        .iter()
        .map(|item| (item.node_id.clone(), item.live_ticket_state.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut lines = vec!["flowchart TD".to_string()];
    for node in &workflow.nodes {
        let req = match node.requirement {
            crate::SessionWorkflowNodeRequirement::Required => "req",
            crate::SessionWorkflowNodeRequirement::Optional => "opt",
        };
        let live = live_states
            .get(&node.node_id)
            .and_then(|state| state.as_deref())
            .unwrap_or("-");
        let label = format!(
            "{} |{}| |{}| |ticket:{}|",
            node.title,
            req,
            workflow_status_label(node.status),
            live
        );
        lines.push(format!(
            "  {}[\"{}\"]",
            mermaid_node_id(&node.node_id),
            escape_mermaid_label(&label)
        ));
    }

    for edge in &workflow.edges {
        let arrow = match edge.kind {
            SessionWorkflowEdgeKind::DependsOn => "-->|depends_on|",
            SessionWorkflowEdgeKind::Order => "-->|order|",
        };
        lines.push(format!(
            "  {} {} {}",
            mermaid_node_id(&edge.from),
            arrow,
            mermaid_node_id(&edge.to)
        ));
    }

    lines.join("\n")
}
