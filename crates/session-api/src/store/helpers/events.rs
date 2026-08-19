use super::*;

pub(super) fn captured_event_key(event: &CopilotHookEvent) -> String {
    if let Some(id) = &event.event_id {
        return format!("id:{id}");
    }
    format!(
        "type:{}|ts:{}|msg:{}|call:{}|turn:{}|tool:{}|ok:{}|reason:{}|req:{}|args:{}|data:{}",
        event.event_type.as_deref().unwrap_or(""),
        event
            .captured_at
            .map(|timestamp| timestamp.to_rfc3339())
            .unwrap_or_default(),
        event.message_id.as_deref().unwrap_or(""),
        event.tool_call_id.as_deref().unwrap_or(""),
        event.turn_id.as_deref().unwrap_or(""),
        event.tool_name.as_deref().unwrap_or(""),
        event
            .tool_success
            .map(|success| success.to_string())
            .unwrap_or_default(),
        event.reasoning_text.as_deref().unwrap_or(""),
        json_fingerprint(&event.tool_requests_json),
        json_fingerprint(&event.tool_arguments_json),
        json_fingerprint(&event.data_json),
    )
}

/// Canonicalize a batch of captured events by collapsing redundant
/// `tool.execution_result` entries that are already covered by a matching
/// `tool.execution_complete` for the same `tool_call_id`.
///
/// Events whose payloads are unique (i.e. a `tool.execution_result` with no
/// corresponding `tool.execution_complete`, or any other event type) are
/// **retained** unchanged — deduplication only collapses true structural
/// duplicates.
pub(super) fn canonicalize_captured_events(
    events: Vec<CopilotHookEvent>
) -> Vec<CopilotHookEvent> {
    let mut complete_ids = std::collections::BTreeSet::<String>::new();
    let mut results = std::collections::BTreeMap::<
        String,
        serde_json::Map<String, serde_json::Value>,
    >::new();
    for event in &events {
        if is_complete(event.event_type.as_deref()) {
            if let Some(tool_call_id) = event.tool_call_id.as_ref() {
                complete_ids.insert(tool_call_id.clone());
            }
        }
        if is_result(event.event_type.as_deref()) {
            if let (Some(tool_call_id), Some(serde_json::Value::Object(data))) =
                (event.tool_call_id.as_ref(), event.data_json.as_ref())
            {
                results
                    .entry(tool_call_id.clone())
                    .or_insert_with(|| data.clone());
            }
        }
    }

    events
        .into_iter()
        .filter_map(|mut event| {
            if is_result(event.event_type.as_deref())
                && event
                    .tool_call_id
                    .as_ref()
                    .is_some_and(|id| complete_ids.contains(id))
            {
                return None;
            }
            if is_complete(event.event_type.as_deref()) {
                if let Some(data) =
                    event.tool_call_id.as_ref().and_then(|id| results.get(id))
                {
                    merge_result_data(&mut event, data);
                }
            }
            Some(event)
        })
        .collect()
}

fn is_complete(event_type: Option<&str>) -> bool {
    matches!(
        event_type,
        Some("tool.execution_complete") | Some("tool_execution_complete")
    )
}

fn is_result(event_type: Option<&str>) -> bool {
    matches!(
        event_type,
        Some("tool.execution_result") | Some("tool_execution_result")
    )
}

fn merge_result_data(
    event: &mut CopilotHookEvent,
    result_data: &serde_json::Map<String, serde_json::Value>,
) {
    match event.data_json.as_mut() {
        Some(serde_json::Value::Object(existing)) => {
            for (key, value) in result_data {
                existing.entry(key.clone()).or_insert_with(|| value.clone());
            }
        },
        _ =>
            event.data_json =
                Some(serde_json::Value::Object(result_data.clone())),
    }
}

fn json_fingerprint(value: &Option<serde_json::Value>) -> String {
    value
        .as_ref()
        .and_then(|value| serde_json::to_string(value).ok())
        .unwrap_or_default()
}
