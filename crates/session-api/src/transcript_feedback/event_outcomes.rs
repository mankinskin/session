use std::collections::BTreeSet;

use crate::CopilotHookEvent;

pub(super) fn is_tool_execution_outcome(event_type: Option<&str>) -> bool {
    matches!(
        event_type,
        Some("tool.execution_result")
            | Some("tool_execution_result")
            | Some("tool.execution_complete")
            | Some("tool_execution_complete")
    )
}

pub(super) fn canonicalize_outcome_events(
    events: &[CopilotHookEvent]
) -> Vec<CopilotHookEvent> {
    let mut complete_tool_calls = BTreeSet::<String>::new();
    for event in events {
        if is_tool_execution_complete(event.event_type.as_deref()) {
            if let Some(tool_call_id) = event.tool_call_id.as_ref() {
                complete_tool_calls.insert(tool_call_id.clone());
            }
        }
    }

    let mut normalized = Vec::with_capacity(events.len());
    for event in events {
        if is_tool_execution_result(event.event_type.as_deref())
            && event
                .tool_call_id
                .as_ref()
                .is_some_and(|id| complete_tool_calls.contains(id))
        {
            continue;
        }
        normalized.push(event.clone());
    }

    normalized
}

fn is_tool_execution_complete(event_type: Option<&str>) -> bool {
    matches!(
        event_type,
        Some("tool.execution_complete") | Some("tool_execution_complete")
    )
}

fn is_tool_execution_result(event_type: Option<&str>) -> bool {
    matches!(
        event_type,
        Some("tool.execution_result") | Some("tool_execution_result")
    )
}
