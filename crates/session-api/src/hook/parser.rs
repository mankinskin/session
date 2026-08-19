use std::path::Path;

use chrono::{
    DateTime,
    Utc,
};
use serde_json::Value;

use super::{
    SessionError,
    SessionRole,
    TranscriptEventEnvelope,
};

pub(super) fn deserialize_transcript_event(
    line: &str,
    transcript_path: &Path,
) -> Result<TranscriptEventEnvelope, SessionError> {
    let value: Value = serde_json::from_str(line).map_err(|source| {
        SessionError::Deserialize {
            path: transcript_path.to_path_buf(),
            source,
        }
    })?;
    let value = normalize_embedded_json_strings(value);
    let data = value.get("data").cloned().unwrap_or_else(|| value.clone());

    Ok(TranscriptEventEnvelope {
        event_id: json_string(&value, &["id"]),
        parent_event_id: json_string(&value, &["parentId", "parent_id"]),
        event_type: first_non_empty_string(&[
            value.get("type").and_then(Value::as_str),
            value.get("event").and_then(Value::as_str),
            value.get("name").and_then(Value::as_str),
        ])
        .map(ToString::to_string),
        timestamp: value
            .get("timestamp")
            .and_then(parse_timestamp_value)
            .or_else(|| value.get("ts").and_then(parse_timestamp_value))
            .or_else(|| data.get("timestamp").and_then(parse_timestamp_value))
            .or_else(|| data.get("ts").and_then(parse_timestamp_value)),
        role_hint: parse_role(
            value
                .get("role")
                .and_then(Value::as_str)
                .or_else(|| data.get("role").and_then(Value::as_str)),
        ),
        content_hint: first_non_empty_string(&[
            value.get("content").and_then(Value::as_str),
            value.get("text").and_then(Value::as_str),
            data.get("content").and_then(Value::as_str),
            data.get("text").and_then(Value::as_str),
        ])
        .map(ToString::to_string),
        turn_id: json_string(&data, &["turnId", "turn_id"]),
        message_id: json_string(&data, &["messageId", "message_id"]),
        tool_call_id: json_string(&data, &["toolCallId", "tool_call_id"]),
        tool_name: json_string(&data, &["toolName", "tool_name"]),
        tool_success: data.get("success").and_then(Value::as_bool),
        reasoning_text: json_string(
            &data,
            &["reasoningText", "reasoning_text"],
        ),
        tool_requests_json: json_value(&data, "toolRequests"),
        tool_arguments_json: json_value(&data, "arguments"),
        data_json: Some(data.clone()),
        raw_event_json: Some(value),
        subagent_run_id: None,
        data,
    })
}

pub(super) fn json_string(
    value: &Value,
    keys: &[&str],
) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
}

pub(super) fn json_timestamp(
    value: &Value,
    keys: &[&str],
) -> Option<DateTime<Utc>> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(parse_timestamp_value)
}

fn json_value(
    value: &Value,
    key: &str,
) -> Option<Value> {
    value.get(key).cloned()
}

fn normalize_embedded_json_strings(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(normalize_embedded_json_strings)
                .collect(),
        ),
        Value::Object(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| {
                    (key, normalize_embedded_json_strings(value))
                })
                .collect(),
        ),
        Value::String(text) => parse_stringified_json_value(&text)
            .map(normalize_embedded_json_strings)
            .unwrap_or(Value::String(text)),
        other => other,
    }
}

fn parse_stringified_json_value(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || (!trimmed.starts_with('{') && !trimmed.starts_with('['))
    {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn parse_timestamp_value(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(text) = value.as_str() {
        return DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|timestamp| timestamp.with_timezone(&Utc));
    }
    value
        .as_i64()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
}

fn parse_role(role: Option<&str>) -> Option<SessionRole> {
    match role?.trim().to_ascii_lowercase().as_str() {
        "user" => Some(SessionRole::User),
        "assistant" | "model" => Some(SessionRole::Assistant),
        "tool" => Some(SessionRole::Tool),
        "system" => Some(SessionRole::System),
        _ => None,
    }
}

fn first_non_empty_string<'a>(values: &[Option<&'a str>]) -> Option<&'a str> {
    values
        .iter()
        .flatten()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
}
