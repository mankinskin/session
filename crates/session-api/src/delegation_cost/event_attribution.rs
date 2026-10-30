#[allow(unused_imports)]
use super::*;

/// Compute the delegation cost report from a session's raw
/// [`crate::PersistedSessionEvents`] audit trail rather than its compacted
/// `messages` turns.
///
/// Some captured sessions only ever populate `messages`
/// (`SessionRecord::turns`) with `user`/`assistant` text turns — the
/// `tool.execution_start`/`tool.execution_complete` events that carry
/// sub-agent span, tool-call, and failure signals are recorded solely in the
/// raw event stream (see `crate::hook::transcript::handle_message_event`,
/// which only ever pushes `user.message`/`assistant.message` events into
/// `messages`). This reconstructs the missing tool-call turns from that
/// event stream, attributing each to its owning sub-agent span using the
/// same `parent_event_id` ancestry rule the live capture path uses for
/// `subagent_run_id` (see `crate::hook::transcript`), then hands the result
/// to the same tested [`compute_delegation_cost_report`] used everywhere
/// else — no metric logic is duplicated here.
pub fn compute_delegation_cost_report_from_events(
    session_id: &str,
    events: &crate::PersistedSessionEvents,
) -> DelegationCostReport {
    use chrono::Utc;

    // event_id -> the subagent_run_id (tool_call_id of the nearest enclosing
    // `runSubagent` span) that descendants of that event should inherit.
    let mut span_owner_by_event_id: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut turns: Vec<crate::SessionTurn> = Vec::new();

    for (sequence, event) in events.events.iter().enumerate() {
        let parent_owner = event
            .parent_event_id
            .as_ref()
            .and_then(|parent_id| span_owner_by_event_id.get(parent_id.as_str()).cloned())
            .flatten();

        let is_subagent_start = event.tool_name.as_deref() == Some("runSubagent")
            && matches!(
                event.event_type.as_deref(),
                Some("tool.execution_start") | Some("tool_execution_start")
            );
        let owner_for_descendants = if is_subagent_start {
            event.tool_call_id.clone()
        } else {
            parent_owner.clone()
        };
        if let Some(event_id) = &event.event_id {
            span_owner_by_event_id.insert(event_id.clone(), owner_for_descendants);
        }

        let is_tool_complete = matches!(
            event.event_type.as_deref(),
            Some("tool.execution_complete") | Some("tool_execution_complete")
        );
        if !is_subagent_start && !is_tool_complete {
            continue;
        }

        let data = event.data_json.as_ref();
        let tool_name = event.tool_name.clone().or_else(|| {
            data.and_then(|d| d.get("tool_name"))
                .and_then(|v| v.as_str())
                .map(String::from)
        });
        let Some(tool_name) = tool_name else {
            continue;
        };
        let tool_arguments_json = event
            .tool_arguments_json
            .clone()
            .or_else(|| data.and_then(|d| d.get("arguments")).cloned());
        let result_code = data
            .and_then(|d| d.get("result_code"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let error_message = data
            .and_then(|d| d.get("error").or_else(|| d.get("message")))
            .and_then(|v| v.as_str())
            .map(String::from);

        turns.push(crate::SessionTurn {
            sequence,
            role: SessionRole::Tool,
            content: String::new(),
            captured_at: event.captured_at.unwrap_or_else(Utc::now),
            tool_name: Some(tool_name),
            model: None,
            event_meta: Some(crate::SessionTurnEventMeta {
                event_id: event.event_id.clone(),
                parent_event_id: event.parent_event_id.clone(),
                event_type: event.event_type.clone(),
                tool_call_id: event.tool_call_id.clone(),
                tool_success: event.tool_success,
                tool_arguments_json,
                result_code,
                error_message,
                subagent_run_id: parent_owner,
                ..Default::default()
            }),
        });
    }

    let record = crate::SessionRecord {
        schema_version: events.schema_version,
        session_id: session_id.to_string(),
        source: "events-replay".to_string(),
        started_at: events.captured_at,
        captured_at: events.captured_at,
        metadata: crate::SessionMetadata {
            workspace_path: "default".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: None,
            trigger: None,
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree: None,
        },
        turns,
        links: crate::SessionLinks::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: Vec::new(),
        picked_up_handoff_ids: Vec::new(),
    };

    compute_delegation_cost_report(&record)
}
