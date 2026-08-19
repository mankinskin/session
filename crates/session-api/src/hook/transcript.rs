use std::{
    collections::HashMap,
    fs::File,
    io::{
        BufRead,
        BufReader,
    },
    path::Path,
};

use chrono::{
    DateTime,
    Utc,
};

use super::{
    CopilotHookMessage,
    CopilotHookPayload,
    CopilotRuntimeMetadata,
    SessionError,
    SessionRole,
    ToolExecutionContext,
    TranscriptEventEnvelope,
    build_tool_execution_result_event,
    capture_tool_execution_context,
    deserialize_transcript_event,
    hydrate_tool_execution_complete,
};

/// Hook-invocation-scoped tool output size, carried in the PostToolUse hook
/// stdin payload (`tool_response`) rather than the transcript file itself
/// (ticket 44119807: the transcript carries no tool result payload, so this
/// is the highest-fidelity source available and overrides any lower-fidelity
/// value derived while parsing the transcript, e.g. a spill-file byte stat).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResponseOverride {
    pub tool_call_id: String,
    pub output_chars: u64,
    /// Provenance for `output_chars` (e.g. "hook_payload", "spill_file").
    pub output_source: String,
}

pub fn copilot_payload_from_transcript_path(
    transcript_path: impl AsRef<Path>,
    workspace_slug: impl Into<String>,
    trigger: Option<String>,
) -> Result<CopilotHookPayload, SessionError> {
    copilot_payload_from_transcript_path_with_tool_response_override(
        transcript_path,
        workspace_slug,
        trigger,
        None,
    )
}

pub fn copilot_payload_from_transcript_path_with_tool_response_override(
    transcript_path: impl AsRef<Path>,
    workspace_slug: impl Into<String>,
    trigger: Option<String>,
    tool_response_override: Option<ToolResponseOverride>,
) -> Result<CopilotHookPayload, SessionError> {
    let transcript_path = transcript_path.as_ref();
    let file =
        File::open(transcript_path).map_err(|source| SessionError::Io {
            path: transcript_path.to_path_buf(),
            source,
        })?;
    let reader = BufReader::new(file);

    copilot_payload_from_transcript_reader_with_path(
        reader,
        transcript_path,
        workspace_slug.into(),
        trigger,
        tool_response_override,
    )
}

pub fn copilot_payload_from_transcript_reader<R: BufRead>(
    reader: R,
    workspace_slug: impl Into<String>,
    trigger: Option<String>,
) -> Result<CopilotHookPayload, SessionError> {
    copilot_payload_from_transcript_reader_with_path(
        reader,
        Path::new("<copilot-transcript>"),
        workspace_slug.into(),
        trigger,
        None,
    )
}

fn copilot_payload_from_transcript_reader_with_path<R: BufRead>(
    reader: R,
    transcript_path: &Path,
    workspace_slug: String,
    trigger: Option<String>,
    tool_response_override: Option<ToolResponseOverride>,
) -> Result<CopilotHookPayload, SessionError> {
    let mut session_id = None;
    let mut agent_id = None;
    let mut captured_at = None;
    let mut started_at = None;
    let mut runtime = CopilotRuntimeMetadata {
        producer: None,
        copilot_version: None,
        vscode_version: None,
        protocol_version: None,
    };
    let mut messages = vec![];
    let mut events = vec![];
    let mut tool_execution_contexts: HashMap<String, ToolExecutionContext> =
        HashMap::new();
    // Sub-agent span attribution (ticket b7c61f0e): maps an event's own
    // `event_id` to the `tool_call_id` of the nearest enclosing `runSubagent`
    // invocation, derived from true `parent_event_id` ancestry as each event
    // is consumed in order. This correctly attributes nested and parallel
    // sub-agent spans without the double-counting that naive event-index
    // overlap produces, because every event has exactly one ancestor chain
    // regardless of how spans interleave in the flat event stream.
    let mut span_owner_by_event_id: HashMap<String, Option<String>> =
        HashMap::new();

    for line in reader.lines() {
        let line = line.map_err(|source| SessionError::Io {
            path: transcript_path.to_path_buf(),
            source,
        })?;
        if line.trim().is_empty() {
            continue;
        }

        let mut event = deserialize_transcript_event(&line, transcript_path)?;
        retag_tool_only_assistant_message(&mut event);

        if let Some(context) = capture_tool_execution_context(&event) {
            if let Some(tool_call_id) = event.tool_call_id.clone() {
                tool_execution_contexts.insert(tool_call_id, context);
            }
        }

        let tool_call_key = event.tool_call_id.clone().unwrap_or_default();
        let context = tool_execution_contexts.get(tool_call_key.as_str());
        hydrate_tool_execution_complete(&mut event, context);

        let parent_owner = event
            .parent_event_id
            .as_ref()
            .and_then(|parent_id| {
                span_owner_by_event_id.get(parent_id.as_str()).cloned()
            })
            .flatten();
        let is_subagent_start = event.tool_name.as_deref()
            == Some("runSubagent")
            && matches!(
                event.event_type.as_deref(),
                Some("tool.execution_start") | Some("tool_execution_start")
            );
        event.subagent_run_id = parent_owner.clone();
        let owner_for_descendants = if is_subagent_start {
            event.tool_call_id.clone()
        } else {
            parent_owner
        };
        if let Some(event_id) = event.event_id.clone() {
            span_owner_by_event_id.insert(event_id, owner_for_descendants);
        }

        events.push(event.captured_event());
        if let Some(result_event) =
            build_tool_execution_result_event(&event, context)
        {
            events.push(result_event);
        }

        match event.event_type.as_deref() {
            Some("session.start")
            | Some("session_start")
            | Some("sessionStart") => handle_session_start_event(
                &event,
                &mut session_id,
                &mut agent_id,
                &mut started_at,
                &mut captured_at,
                &mut runtime,
            )?,
            Some("user.message") | Some("user_message") =>
                handle_message_event(
                    &event,
                    SessionRole::User,
                    &mut captured_at,
                    &mut messages,
                )?,
            Some("assistant.message") | Some("assistant_message") =>
                handle_message_event(
                    &event,
                    SessionRole::Assistant,
                    &mut captured_at,
                    &mut messages,
                )?,
            _ =>
                if let Some(role) = event.role_hint.clone() {
                    handle_message_event(
                        &event,
                        role,
                        &mut captured_at,
                        &mut messages,
                    )?;
                },
        }
    }

    let session_id = session_id.ok_or(SessionError::MissingSessionId)?;
    if messages.is_empty() {
        return Err(SessionError::EmptyTurns);
    }

    let runtime = if runtime.producer.is_none()
        && runtime.copilot_version.is_none()
        && runtime.vscode_version.is_none()
        && runtime.protocol_version.is_none()
    {
        None
    } else {
        Some(runtime)
    };

    Ok(CopilotHookPayload {
        session_id,
        workspace_slug,
        captured_at: captured_at.or(started_at).unwrap_or_else(Utc::now),
        conversation_id: None,
        agent_id,
        model: None,
        trigger,
        provisioning: None,
        messages,
        events: apply_tool_response_override(events, tool_response_override),
        runtime,
    })
}

/// Merge the hook-payload output size into the matching terminal tool event,
/// overwriting any lower-fidelity `output_chars`/`output_source` already set
/// while parsing the transcript (e.g. a spill-file stat).
fn apply_tool_response_override(
    mut events: Vec<super::CopilotHookEvent>,
    tool_response_override: Option<ToolResponseOverride>,
) -> Vec<super::CopilotHookEvent> {
    let Some(override_value) = tool_response_override else {
        return events;
    };
    for event in events.iter_mut() {
        let is_terminal = matches!(
            event.event_type.as_deref(),
            Some("tool.execution_complete")
                | Some("tool_execution_complete")
                | Some("tool.execution_result")
                | Some("tool_execution_result")
        );
        if !is_terminal {
            continue;
        }
        if event.tool_call_id.as_deref()
            != Some(override_value.tool_call_id.as_str())
        {
            continue;
        }
        let data = event.data_json.get_or_insert_with(|| {
            serde_json::Value::Object(Default::default())
        });
        if let Some(map) = data.as_object_mut() {
            map.insert(
                "output_chars".to_string(),
                serde_json::Value::from(override_value.output_chars),
            );
            map.insert(
                "output_source".to_string(),
                serde_json::Value::String(override_value.output_source.clone()),
            );
        }
        break;
    }
    events
}

fn handle_session_start_event(
    event: &TranscriptEventEnvelope,
    session_id: &mut Option<String>,
    agent_id: &mut Option<String>,
    started_at: &mut Option<DateTime<Utc>>,
    captured_at: &mut Option<DateTime<Utc>>,
    runtime: &mut CopilotRuntimeMetadata,
) -> Result<(), SessionError> {
    let data = &event.data;
    let session_id_value =
        super::json_string(data, &["sessionId", "session_id", "id"]);
    let producer_value =
        super::json_string(data, &["producer", "agentId", "agent_id"]);
    let start_time_value =
        super::json_timestamp(data, &["startTime", "start_time"]);

    if session_id.is_none() {
        *session_id = session_id_value;
    }
    if agent_id.is_none() {
        *agent_id = producer_value.clone();
    }
    if started_at.is_none() {
        *started_at = start_time_value.or(event.timestamp);
    }
    if captured_at.is_none() {
        *captured_at = event.timestamp;
    }
    if runtime.producer.is_none() {
        runtime.producer = producer_value;
    }
    if runtime.copilot_version.is_none() {
        runtime.copilot_version =
            super::json_string(data, &["copilotVersion", "copilot_version"]);
    }
    if runtime.vscode_version.is_none() {
        runtime.vscode_version =
            super::json_string(data, &["vscodeVersion", "vscode_version"]);
    }
    if runtime.protocol_version.is_none() {
        runtime.protocol_version = data
            .get("version")
            .or_else(|| data.get("protocolVersion"))
            .and_then(serde_json::Value::as_i64);
    }
    Ok(())
}

fn handle_message_event(
    event: &TranscriptEventEnvelope,
    role: SessionRole,
    captured_at: &mut Option<DateTime<Utc>>,
    messages: &mut Vec<CopilotHookMessage>,
) -> Result<(), SessionError> {
    let content = event.content_hint.clone().unwrap_or_default();
    if content.trim().is_empty() {
        return Ok(());
    }
    let timestamp = event.timestamp.unwrap_or_else(Utc::now);
    *captured_at = Some(timestamp);
    messages.push(CopilotHookMessage {
        role,
        content,
        tool_name: event.tool_name.clone(),
        captured_at: Some(timestamp),
        event_meta: event.event_meta(),
    });
    Ok(())
}

fn retag_tool_only_assistant_message(event: &mut TranscriptEventEnvelope) {
    let is_assistant_message = matches!(
        event.event_type.as_deref(),
        Some("assistant.message") | Some("assistant_message")
    );
    if !is_assistant_message {
        return;
    }
    let has_content = event
        .content_hint
        .as_ref()
        .is_some_and(|content| !content.trim().is_empty());
    if has_content {
        return;
    }
    let has_tool_requests = event
        .tool_requests_json
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| !items.is_empty());
    if has_tool_requests {
        event.event_type = Some("assistant.tool_plan".to_string());
    }
}
