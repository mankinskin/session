use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};
use serde_json::Value;

use crate::{
    SessionError,
    SessionLinks,
    SessionMetadata,
    SessionProvisioningDiagnostic,
    SessionRecord,
    SessionRole,
    SessionTurn,
    SessionTurnEventMeta,
};

mod tool_execution;
mod transcript;

use parser::{
    deserialize_transcript_event,
    json_string,
    json_timestamp,
};

pub use transcript::{
    ToolResponseOverride,
    copilot_payload_from_transcript_path,
    copilot_payload_from_transcript_path_with_tool_response_override,
    copilot_payload_from_transcript_reader,
};

#[path = "hook/parser.rs"]
mod parser;

use tool_execution::{
    ToolExecutionContext,
    build_tool_execution_result_event,
    capture_tool_execution_context,
    hydrate_tool_execution_complete,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotHookEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_success: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_requests_json: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_arguments_json: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_json: Option<Value>,
    /// Back-compat read shim: older `events.json` files may contain this field,
    /// but it is never written to new files (`skip_serializing`).
    #[serde(default, skip_serializing)]
    pub raw_event_json: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotRuntimeMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copilot_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vscode_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopilotHookMessage {
    pub role: SessionRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_meta: Option<SessionTurnEventMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopilotHookPayload {
    pub session_id: String,
    pub workspace_slug: String,
    pub captured_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provisioning: Option<SessionProvisioningDiagnostic>,
    #[serde(default)]
    pub messages: Vec<CopilotHookMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<CopilotHookEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<CopilotRuntimeMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionCaptureRequest {
    pub source: String,
    pub payload: CopilotHookPayload,
    #[serde(default)]
    pub links: SessionLinks,
}

impl SessionCaptureRequest {
    pub fn copilot(payload: CopilotHookPayload) -> Self {
        Self {
            source: "copilot-hook".to_string(),
            payload,
            links: SessionLinks::default(),
        }
    }

    pub fn into_record(self) -> Result<SessionRecord, SessionError> {
        self.into_record_and_events().map(|(record, _)| record)
    }

    pub fn into_record_and_events(
        self
    ) -> Result<(SessionRecord, Vec<CopilotHookEvent>), SessionError> {
        let payload = self.payload;
        if payload.session_id.trim().is_empty() {
            return Err(SessionError::MissingSessionId);
        }
        if payload.messages.is_empty() {
            return Err(SessionError::EmptyTurns);
        }

        let captured_at = payload.captured_at;
        let session_model = payload.model.clone();
        let turns: Vec<SessionTurn> = payload
            .messages
            .into_iter()
            .enumerate()
            .map(|(sequence, message)| {
                // Attribute the active model to model-produced turns. User and
                // tool turns leave `model` as `None` and inherit the
                // session-level model in `SessionMetadata::model`.
                let model = match message.role {
                    SessionRole::Assistant => session_model.clone(),
                    _ => None,
                };
                SessionTurn {
                    sequence,
                    role: message.role,
                    content: message.content,
                    captured_at: message.captured_at.unwrap_or(captured_at),
                    tool_name: message.tool_name,
                    model,
                    event_meta: message.event_meta,
                }
            })
            .collect();
        let started_at = turns
            .first()
            .map(|turn| turn.captured_at)
            .unwrap_or(captured_at);

        let runtime = payload.runtime.unwrap_or(CopilotRuntimeMetadata {
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
        });

        Ok((
            SessionRecord {
                schema_version: crate::SESSION_SCHEMA_VERSION,
                session_id: payload.session_id,
                source: self.source,
                started_at,
                captured_at,
                metadata: SessionMetadata {
                    workspace_slug: payload.workspace_slug,
                    conversation_id: payload.conversation_id,
                    agent_id: payload.agent_id,
                    ticket_id: None,
                    model: payload.model,
                    trigger: payload.trigger,
                    provisioning: payload.provisioning,
                    producer: runtime.producer,
                    copilot_version: runtime.copilot_version,
                    vscode_version: runtime.vscode_version,
                    protocol_version: runtime.protocol_version,
                    worktree: None,
                },
                turns,
                links: self.links,
                track_id: None,
                anchor_ticket_id: None,
                parent_session_id: None,
                spawned_session_id: None,
                emitted_handoff_ids: Vec::new(),
                picked_up_handoff_ids: Vec::new(),
            },
            payload.events,
        ))
    }
}

impl TryFrom<SessionCaptureRequest> for SessionRecord {
    type Error = SessionError;

    fn try_from(value: SessionCaptureRequest) -> Result<Self, Self::Error> {
        value.into_record()
    }
}

#[derive(Debug)]
struct TranscriptEventEnvelope {
    event_id: Option<String>,
    parent_event_id: Option<String>,
    event_type: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    data: serde_json::Value,
    role_hint: Option<SessionRole>,
    content_hint: Option<String>,
    turn_id: Option<String>,
    message_id: Option<String>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    tool_success: Option<bool>,
    reasoning_text: Option<String>,
    tool_requests_json: Option<Value>,
    tool_arguments_json: Option<Value>,
    data_json: Option<Value>,
    raw_event_json: Option<Value>,
    /// Nearest enclosing `runSubagent` span this event belongs to, resolved
    /// via `parent_event_id` ancestry during transcript parsing (ticket
    /// b7c61f0e). See [`crate::SessionTurnEventMeta::subagent_run_id`].
    subagent_run_id: Option<String>,
}

impl TranscriptEventEnvelope {
    fn event_meta(&self) -> Option<SessionTurnEventMeta> {
        // Extract token and model attribution from data_json (ticket 6549b6a7)
        let (
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            model_id,
        ) = if let Some(data) = &self.data_json {
            let usage = data.get("usage");
            let input_tokens = usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(|v| v.as_u64());
            let output_tokens = usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(|v| v.as_u64());
            let cache_read_tokens = usage
                .and_then(|u| u.get("cache_read_tokens"))
                .and_then(|v| v.as_u64());
            let cache_write_tokens = usage
                .and_then(|u| u.get("cache_write_tokens"))
                .and_then(|v| v.as_u64());
            let model_id = data
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                model_id,
            )
        } else {
            (None, None, None, None, None)
        };

        // Extract error/exit/result_code from data_json (ticket 84c7757d)
        let (error_message, exit_code, result_code) =
            if let Some(data) = &self.data_json {
                let error_message = data
                    .get("error_message")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let exit_code = data
                    .get("exit_code")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32);
                let result_code = data
                    .get("result_code")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                (error_message, exit_code, result_code)
            } else {
                (None, None, None)
            };

        let meta = SessionTurnEventMeta {
            event_id: self.event_id.clone(),
            parent_event_id: self.parent_event_id.clone(),
            event_type: self.event_type.clone(),
            turn_id: self.turn_id.clone(),
            message_id: self.message_id.clone(),
            tool_call_id: self.tool_call_id.clone(),
            tool_success: self.tool_success,
            reasoning_text: self.reasoning_text.clone(),
            tool_requests_json: self.tool_requests_json.clone(),
            tool_arguments_json: self.tool_arguments_json.clone(),
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            cost_usd: None, // Computed later in into_record_and_events
            model_id,
            request_bytes: None,
            request_chars: None,
            response_bytes: None,
            response_chars: None,
            tokens_estimated: None,
            error_message,
            exit_code,
            result_code,
            subagent_run_id: self.subagent_run_id.clone(),
        };
        if meta.event_id.is_none()
            && meta.parent_event_id.is_none()
            && meta.event_type.is_none()
            && meta.turn_id.is_none()
            && meta.message_id.is_none()
            && meta.tool_call_id.is_none()
            && meta.tool_success.is_none()
            && meta.reasoning_text.is_none()
            && meta.tool_requests_json.is_none()
            && meta.tool_arguments_json.is_none()
            && meta.input_tokens.is_none()
            && meta.output_tokens.is_none()
            && meta.cache_read_tokens.is_none()
            && meta.cache_write_tokens.is_none()
            && meta.cost_usd.is_none()
            && meta.model_id.is_none()
            && meta.error_message.is_none()
            && meta.exit_code.is_none()
            && meta.result_code.is_none()
            && meta.subagent_run_id.is_none()
        {
            None
        } else {
            Some(meta)
        }
    }

    fn captured_event(&self) -> CopilotHookEvent {
        CopilotHookEvent {
            event_id: self.event_id.clone(),
            parent_event_id: self.parent_event_id.clone(),
            event_type: self.event_type.clone(),
            captured_at: self.timestamp,
            turn_id: self.turn_id.clone(),
            message_id: self.message_id.clone(),
            tool_call_id: self.tool_call_id.clone(),
            tool_name: self.tool_name.clone(),
            tool_success: self.tool_success,
            reasoning_text: self.reasoning_text.clone(),
            tool_requests_json: self.tool_requests_json.clone(),
            tool_arguments_json: self.tool_arguments_json.clone(),
            data_json: self.data_json.clone(),
            raw_event_json: self.raw_event_json.clone(),
        }
    }
}

#[cfg(test)]
#[path = "hook/tests.rs"]
mod tests;
