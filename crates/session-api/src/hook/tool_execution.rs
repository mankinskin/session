use chrono::{
    DateTime,
    Utc,
};
use serde_json::Value;

use crate::CopilotHookEvent;

use super::TranscriptEventEnvelope;

#[derive(Debug, Clone)]
pub(super) struct ToolExecutionContext {
    started_at: Option<DateTime<Utc>>,
    tool_name: Option<String>,
    tool_arguments_json: Option<Value>,
}

pub(super) fn capture_tool_execution_context(
    event: &TranscriptEventEnvelope
) -> Option<ToolExecutionContext> {
    let is_start = matches!(
        event.event_type.as_deref(),
        Some("tool.execution_start") | Some("tool_execution_start")
    );
    if !is_start {
        return None;
    }
    event.tool_call_id.as_ref()?;

    Some(ToolExecutionContext {
        started_at: event.timestamp,
        tool_name: event.tool_name.clone(),
        tool_arguments_json: event.tool_arguments_json.clone(),
    })
}

pub(super) fn hydrate_tool_execution_complete(
    event: &mut TranscriptEventEnvelope,
    context: Option<&ToolExecutionContext>,
) {
    let is_complete = matches!(
        event.event_type.as_deref(),
        Some("tool.execution_complete") | Some("tool_execution_complete")
    );
    if !is_complete {
        return;
    }

    if event.tool_name.is_none() {
        event.tool_name = context.and_then(|ctx| ctx.tool_name.clone());
    }
    if event.tool_arguments_json.is_none() {
        event.tool_arguments_json =
            context.and_then(|ctx| ctx.tool_arguments_json.clone());
    }
}

pub(super) fn build_tool_execution_result_event(
    event: &TranscriptEventEnvelope,
    context: Option<&ToolExecutionContext>,
) -> Option<CopilotHookEvent> {
    let is_complete = matches!(
        event.event_type.as_deref(),
        Some("tool.execution_complete") | Some("tool_execution_complete")
    );
    if !is_complete {
        return None;
    }

    let tool_call_id = event.tool_call_id.clone()?;
    let tool_name = event
        .tool_name
        .clone()
        .or_else(|| context.and_then(|ctx| ctx.tool_name.clone()));
    let tool_arguments = event
        .tool_arguments_json
        .clone()
        .or_else(|| context.and_then(|ctx| ctx.tool_arguments_json.clone()));
    let duration_ms = event
        .data
        .get("durationMs")
        .or_else(|| event.data.get("duration_ms"))
        .and_then(Value::as_i64)
        .or_else(|| {
            context.and_then(|ctx| {
                let started_at = ctx.started_at?;
                let finished_at = event.timestamp?;
                Some((finished_at - started_at).num_milliseconds())
            })
        });
    let success = event.tool_success;

    // Extract error message from multiple possible locations
    let error_message = event
        .data
        .get("error")
        .and_then(Value::as_object)
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.get("reason").and_then(Value::as_str))
        })
        .or_else(|| {
            event
                .data
                .get("errorMessage")
                .or_else(|| event.data.get("error_message"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            event
                .data
                .get("stderr")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .map(|s| s.trim().to_string());

    // Extract exit code
    let exit_code = event
        .data
        .get("exitCode")
        .or_else(|| event.data.get("exit_code"))
        .and_then(Value::as_i64)
        .map(|v| v as i32);

    // Check for sync terminal ambiguous state (potential hang)
    let has_ambiguous_state =
        event.data.get("blocker").and_then(Value::as_str).is_some()
            || event
                .data
                .get("lifecycle_state")
                .and_then(Value::as_str)
                .is_some();

    // Classify result_code
    let result_code = if has_ambiguous_state {
        "hang"
    } else if let Some(duration) = duration_ms {
        // Timeout classification: duration >= 300000ms AND not explicitly successful
        if duration >= 300_000 && success != Some(true) {
            "timeout"
        } else {
            match success {
                Some(true) => "ok",
                Some(false) => "error",
                None => "unknown",
            }
        }
    } else {
        match success {
            Some(true) => "ok",
            Some(false) => "error",
            None => "unknown",
        }
    };

    let error_type = event
        .data
        .get("error")
        .and_then(Value::as_object)
        .and_then(|error| {
            error
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| error.get("name").and_then(Value::as_str))
        })
        .map(ToString::to_string)
        .or_else(|| {
            event
                .data
                .get("errorType")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });

    let summary = extract_tool_result_summary(&event.data);
    let spill_pointer = find_spill_pointer(&event.data, summary.as_deref());
    let has_spill = spill_pointer.is_some();

    let sync_terminal_ambiguous = is_sync_terminal_completion_ambiguous(
        tool_name.as_deref(),
        tool_arguments.as_ref(),
        success,
        summary.as_deref(),
        spill_pointer.as_deref(),
        &event.data,
    );

    let mut normalized = serde_json::Map::new();
    normalized.insert(
        "toolCallId".to_string(),
        Value::String(tool_call_id.clone()),
    );
    normalized.insert(
        "result_code".to_string(),
        Value::String(result_code.to_string()),
    );
    normalized.insert("has_spill".to_string(), Value::Bool(has_spill));
    if let Some(name) = tool_name.clone() {
        normalized.insert("tool_name".to_string(), Value::String(name));
    }
    if let Some(arguments) = tool_arguments.clone() {
        normalized.insert("arguments".to_string(), arguments);
    }
    if let Some(duration_ms) = duration_ms {
        normalized.insert(
            "duration_ms".to_string(),
            Value::Number(duration_ms.into()),
        );
    }
    if let Some(summary) = summary.clone() {
        normalized.insert("summary".to_string(), Value::String(summary));
    }
    if let Some(pointer) = spill_pointer.clone() {
        // Layer 2 (ticket 44119807): stat the spill file at capture time,
        // while it is still fresh. A missing/unreadable file leaves
        // output_chars unset (unmeasured), never a fabricated zero.
        let spill_output_chars = stat_spill_output_chars(&pointer);
        normalized.insert("spill_pointer".to_string(), Value::String(pointer));
        if let Some(output_chars) = spill_output_chars {
            normalized.insert(
                "output_chars".to_string(),
                Value::Number(output_chars.into()),
            );
            normalized.insert(
                "output_source".to_string(),
                Value::String("spill_file".to_string()),
            );
        }
    }
    if let Some(error_type) = error_type {
        normalized.insert("error_type".to_string(), Value::String(error_type));
    }
    if let Some(error_message) = &error_message {
        normalized.insert(
            "error_message".to_string(),
            Value::String(error_message.clone()),
        );
    }
    if let Some(exit_code) = exit_code {
        normalized
            .insert("exit_code".to_string(), Value::Number(exit_code.into()));
    }
    if sync_terminal_ambiguous {
        normalized.insert(
            "blocker".to_string(),
            Value::String("sync-terminal-state-ambiguous".to_string()),
        );
        normalized.insert(
            "lifecycle_state".to_string(),
            Value::String("background-ambiguous".to_string()),
        );
        normalized.insert(
            "lifecycle_reason".to_string(),
            Value::String(
                "missing-deterministic-sync-completion-metadata".to_string(),
            ),
        );
    }

    Some(CopilotHookEvent {
        event_id: None,
        parent_event_id: event.event_id.clone(),
        event_type: Some("tool.execution_result".to_string()),
        captured_at: event.timestamp,
        turn_id: event.turn_id.clone(),
        message_id: event.message_id.clone(),
        tool_call_id: Some(tool_call_id),
        tool_name,
        tool_success: success,
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: tool_arguments,
        data_json: Some(Value::Object(normalized)),
        raw_event_json: None,
    })
}

fn extract_tool_result_summary(data: &Value) -> Option<String> {
    let candidates = [
        "summary", "output", "stdout", "stderr", "message", "content",
    ];
    let value = candidates
        .iter()
        .filter_map(|key| data.get(*key))
        .find_map(Value::as_str)
        .map(str::trim)
        .filter(|entry| !entry.is_empty())?;

    Some(value.chars().take(240).collect())
}

fn find_spill_pointer(
    data: &Value,
    summary: Option<&str>,
) -> Option<String> {
    if let Some(pointer) = data
        .get("spillPointer")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        return Some(pointer.to_string());
    }

    if let Some(pointer) = data
        .get("outputPath")
        .or_else(|| data.get("path"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        return Some(pointer.to_string());
    }

    if let Some(text) = summary {
        if let Some((_, right)) = text.split_once("content at:") {
            let pointer = right.trim();
            if !pointer.is_empty() {
                return Some(pointer.to_string());
            }
        }
        if let Some((_, right)) = text.split_once("saved to:") {
            let pointer = right.trim();
            if !pointer.is_empty() {
                return Some(pointer.to_string());
            }
        }
    }

    None
}

/// Stat a resolved spill pointer's byte content and return its char count.
/// Accepts either a direct file path or a directory containing `content.txt`
/// (the `chat-session-resources/<session>/<tool_call_id>__*` layout).
/// Returns `None` (unmeasured, not zero) when the file is absent or unreadable.
fn stat_spill_output_chars(pointer: &str) -> Option<u64> {
    let path = std::path::Path::new(pointer.trim());
    let candidate = if path.is_dir() {
        path.join("content.txt")
    } else {
        path.to_path_buf()
    };
    let bytes = std::fs::read(&candidate).ok()?;
    Some(String::from_utf8_lossy(&bytes).chars().count() as u64)
}

fn is_sync_terminal_completion_ambiguous(
    tool_name: Option<&str>,
    tool_arguments: Option<&Value>,
    success: Option<bool>,
    summary: Option<&str>,
    spill_pointer: Option<&str>,
    data: &Value,
) -> bool {
    if tool_name != Some("run_in_terminal") || success != Some(true) {
        return false;
    }

    let mode_is_sync = tool_arguments
        .and_then(|arguments| arguments.get("mode"))
        .and_then(Value::as_str)
        .map(|mode| mode.eq_ignore_ascii_case("sync"))
        .unwrap_or(false);
    if !mode_is_sync {
        return false;
    }

    // Only flag ambiguity when the completion payload explicitly signals
    // background/timeout/input-needed semantics. A plain sync success event
    // without these signals is treated as deterministic completion.
    if data.get("terminalId").is_some()
        || data.get("terminal_id").is_some()
        || data.get("deferredResultId").is_some()
        || data.get("deferred_result_id").is_some()
    {
        return true;
    }

    if data
        .get("needsInput")
        .or_else(|| data.get("needs_input"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }

    if data
        .get("timedOut")
        .or_else(|| data.get("timed_out"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }

    let text_signals = [
        "moved to background",
        "needs input",
        "waiting for input",
        "timed out",
    ];

    [summary, spill_pointer]
        .into_iter()
        .flatten()
        .map(|text| text.to_ascii_lowercase())
        .any(|text| text_signals.iter().any(|signal| text.contains(signal)))
}
