use chrono::TimeZone;
use pretty_assertions::assert_eq;

use crate::{
    PromptPackOptions,
    SessionRole,
    peek_prompt_pack,
};

use super::{
    SessionCaptureRequest,
    copilot_payload_from_transcript_reader,
};

#[path = "tests/capture_request.rs"]
mod capture_request;

#[test]
fn transcript_reader_maps_visible_messages_into_payload() {
    let transcript = r#"{"id":"evt-start","type":"session.start","timestamp":"2026-06-02T23:06:54.049Z","data":{"sessionId":"session-123","producer":"copilot-agent","copilotVersion":"0.55.0","vscodeVersion":"1.127.0","version":1,"startTime":"2026-06-02T23:06:54.049Z"}}
{"id":"evt-1","parentId":"evt-start","type":"user.message","timestamp":"2026-06-02T23:07:00.000Z","data":{"content":"Hello"}}
{"id":"evt-2","parentId":"evt-1","type":"assistant.message","timestamp":"2026-06-02T23:07:05.000Z","data":{"messageId":"m-1","turnId":"t-1","reasoningText":"r","toolRequests":[{"name":"read_file"}],"content":"World"}}
{"id":"evt-3","type":"tool.execution_complete","timestamp":"2026-06-02T23:07:07.000Z","data":{"toolCallId":"call-1","toolName":"read_file","arguments":{"a":1},"success":true}}"#;

    let payload = copilot_payload_from_transcript_reader(
        std::io::Cursor::new(transcript),
        "context-engine",
        Some("stop".to_string()),
    )
    .unwrap();

    assert_eq!(payload.session_id, "session-123");
    assert_eq!(payload.workspace_slug, "context-engine");
    assert_eq!(payload.agent_id.as_deref(), Some("copilot-agent"));
    assert_eq!(payload.trigger.as_deref(), Some("stop"));
    assert_eq!(payload.messages.len(), 2);
    assert_eq!(payload.events.len(), 5);
    assert!(
        payload.events[2]
            .data_json
            .as_ref()
            .and_then(|json| json.get("toolRequests"))
            .is_some()
    );
    let result_event = payload
        .events
        .iter()
        .find(|event| {
            event.event_type.as_deref() == Some("tool.execution_result")
        })
        .expect("expected synthesized tool.execution_result event");
    assert_eq!(result_event.tool_name.as_deref(), Some("read_file"));
    assert_eq!(payload.messages[0].role, SessionRole::User);
    assert_eq!(payload.messages[0].content, "Hello");
    assert_eq!(payload.messages[1].content, "World");
    assert_eq!(
        payload.messages[1]
            .event_meta
            .as_ref()
            .and_then(|m| m.message_id.as_deref()),
        Some("m-1")
    );
    assert_eq!(
        payload
            .runtime
            .as_ref()
            .and_then(|r| r.copilot_version.as_deref()),
        Some("0.55.0")
    );
    assert_eq!(
        payload.captured_at,
        chrono::Utc
            .with_ymd_and_hms(2026, 6, 2, 23, 7, 5)
            .single()
            .unwrap()
    );
}

#[test]
fn transcript_reader_supports_modern_message_shape() {
    let transcript = r#"{"event":"session_start","ts":1717372014049,"data":{"session_id":"session-modern","producer":"copilot-agent"}}
{"event":"message","timestamp":"2026-06-02T23:07:00.000Z","role":"user","content":"Hello modern"}
{"event":"message","timestamp":"2026-06-02T23:07:05.000Z","data":{"role":"assistant","text":"Hi modern"}}"#;

    let payload = copilot_payload_from_transcript_reader(
        std::io::Cursor::new(transcript),
        "default",
        Some("stop".to_string()),
    )
    .unwrap();

    assert_eq!(payload.session_id, "session-modern");
    assert_eq!(payload.messages.len(), 2);
    assert_eq!(payload.messages[0].role, SessionRole::User);
    assert_eq!(payload.messages[0].content, "Hello modern");
    assert_eq!(payload.messages[1].role, SessionRole::Assistant);
    assert_eq!(payload.messages[1].content, "Hi modern");
}

#[test]
fn transcript_reader_destringifies_nested_json_payloads() {
    let transcript = r#"{"id":"evt-start","type":"session.start","timestamp":"2026-06-02T23:06:54.049Z","data":"{\"sessionId\":\"session-json\",\"producer\":\"copilot-agent\"}"}
{"id":"evt-1","type":"assistant.message","timestamp":"2026-06-02T23:07:05.000Z","data":{"messageId":"m-1","content":"World","arguments":"{\"path\":\"src/lib.rs\",\"line\":42}","toolRequests":"[{\"name\":\"read_file\"}]"}}"#;

    let payload = copilot_payload_from_transcript_reader(
        std::io::Cursor::new(transcript),
        "default",
        Some("stop".to_string()),
    )
    .unwrap();

    assert_eq!(payload.session_id, "session-json");
    assert_eq!(payload.events.len(), 2);

    let event_meta = payload.messages[0].event_meta.as_ref().unwrap();
    assert_eq!(
        event_meta
            .tool_arguments_json
            .as_ref()
            .and_then(|value| value.get("path"))
            .and_then(serde_json::Value::as_str),
        Some("src/lib.rs")
    );
    assert_eq!(
        event_meta
            .tool_requests_json
            .as_ref()
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.first())
            .and_then(|value| value.get("name"))
            .and_then(serde_json::Value::as_str),
        Some("read_file")
    );

    assert!(
        payload.events[1]
            .data_json
            .as_ref()
            .and_then(|value| value.get("arguments"))
            .and_then(|value| value.get("line"))
            .and_then(serde_json::Value::as_i64)
            == Some(42)
    );
}

#[test]
fn transcript_reader_retags_tool_only_assistant_messages() {
    let transcript = r#"{"id":"evt-start","type":"session.start","timestamp":"2026-06-02T23:06:54.049Z","data":{"sessionId":"session-tool-plan","producer":"copilot-agent"}}
{"id":"evt-0","type":"user.message","timestamp":"2026-06-02T23:07:00.000Z","data":{"content":"check status"}}
{"id":"evt-1","type":"assistant.message","timestamp":"2026-06-02T23:07:05.000Z","data":{"messageId":"m-1","content":"","toolRequests":[{"name":"get_terminal_output","arguments":{"id":"term-1"}}],"reasoningText":""}}"#;

    let payload = copilot_payload_from_transcript_reader(
        std::io::Cursor::new(transcript),
        "default",
        Some("stop".to_string()),
    )
    .unwrap();

    assert_eq!(payload.messages.len(), 1);
    assert!(payload.events.iter().any(|event| {
        event.event_type.as_deref() == Some("assistant.tool_plan")
    }));
    assert!(!payload.events.iter().any(|event| {
        event.event_type.as_deref() == Some("assistant.message")
            && event
                .data_json
                .as_ref()
                .and_then(|json| json.get("content"))
                .and_then(serde_json::Value::as_str)
                .map(|content| content.trim().is_empty())
                .unwrap_or(false)
    }));
}

#[test]
fn transcript_reader_does_not_mark_ambiguous_sync_terminal_without_signals() {
    let transcript = r#"{"id":"evt-start","type":"session.start","timestamp":"2026-06-02T23:06:54.049Z","data":{"sessionId":"session-terminal","producer":"copilot-agent"}}
{"id":"evt-1","type":"user.message","timestamp":"2026-06-02T23:07:00.000Z","data":{"content":"Run sync command"}}
{"id":"evt-2","type":"tool.execution_start","timestamp":"2026-06-02T23:07:01.000Z","data":{"toolCallId":"call-rt-1","toolName":"run_in_terminal","arguments":{"mode":"sync","command":"cargo test"}}}
{"id":"evt-3","type":"tool.execution_complete","timestamp":"2026-06-02T23:07:03.000Z","data":{"toolCallId":"call-rt-1","success":true}}"#;

    let payload = copilot_payload_from_transcript_reader(
        std::io::Cursor::new(transcript),
        "default",
        Some("stop".to_string()),
    )
    .unwrap();

    let result_event = payload
        .events
        .iter()
        .find(|event| {
            event.event_type.as_deref() == Some("tool.execution_result")
        })
        .expect("expected tool.execution_result event");
    assert_eq!(result_event.tool_name.as_deref(), Some("run_in_terminal"));
    assert_eq!(
        result_event
            .data_json
            .as_ref()
            .and_then(|json| json.get("blocker"))
            .and_then(serde_json::Value::as_str),
        None
    );
    assert_eq!(
        result_event
            .data_json
            .as_ref()
            .and_then(|json| json.get("lifecycle_state"))
            .and_then(serde_json::Value::as_str),
        None
    );
}

#[test]
fn transcript_reader_marks_ambiguous_sync_terminal_with_background_signal() {
    let transcript = r#"{"id":"evt-start","type":"session.start","timestamp":"2026-06-02T23:06:54.049Z","data":{"sessionId":"session-terminal-bg","producer":"copilot-agent"}}
{"id":"evt-1","type":"user.message","timestamp":"2026-06-02T23:07:00.000Z","data":{"content":"Run sync command"}}
{"id":"evt-2","type":"tool.execution_start","timestamp":"2026-06-02T23:07:01.000Z","data":{"toolCallId":"call-rt-2","toolName":"run_in_terminal","arguments":{"mode":"sync","command":"cargo test"}}}
{"id":"evt-3","type":"tool.execution_complete","timestamp":"2026-06-02T23:07:03.000Z","data":{"toolCallId":"call-rt-2","success":true,"message":"Command timed out and moved to background"}}"#;

    let payload = copilot_payload_from_transcript_reader(
        std::io::Cursor::new(transcript),
        "default",
        Some("stop".to_string()),
    )
    .unwrap();

    let result_event = payload
        .events
        .iter()
        .find(|event| {
            event.event_type.as_deref() == Some("tool.execution_result")
        })
        .expect("expected tool.execution_result event");
    assert_eq!(result_event.tool_name.as_deref(), Some("run_in_terminal"));
    assert_eq!(
        result_event
            .data_json
            .as_ref()
            .and_then(|json| json.get("blocker"))
            .and_then(serde_json::Value::as_str),
        Some("sync-terminal-state-ambiguous")
    );
    assert_eq!(
        result_event
            .data_json
            .as_ref()
            .and_then(|json| json.get("lifecycle_state"))
            .and_then(serde_json::Value::as_str),
        Some("background-ambiguous")
    );
}

#[test]
fn transcript_normalization_and_prompt_pack_tool_result_consistency() {
    let transcript = r#"{"id":"evt-start","type":"session.start","timestamp":"2026-06-02T23:06:54.049Z","data":{"sessionId":"session-cross-boundary","producer":"copilot-agent"}}
{"id":"evt-1","type":"user.message","timestamp":"2026-06-02T23:07:00.000Z","data":{"content":"Continue hardening prompt compactness"}}
{"id":"evt-2","type":"assistant.message","timestamp":"2026-06-02T23:07:01.000Z","data":{"messageId":"m-1","content":"I will gather context and verify ticket drift status."}}
{"id":"evt-3","type":"assistant.message","timestamp":"2026-06-02T23:07:02.000Z","data":{"messageId":"m-2","content":"Now I am checking spec and validation anchors."}}
{"id":"evt-4","type":"assistant.message","timestamp":"2026-06-02T23:07:03.000Z","data":{"messageId":"m-3","content":"Durable finding: hook ambiguity should require explicit signals."}}
{"id":"evt-5","type":"tool.execution_start","timestamp":"2026-06-02T23:07:04.000Z","data":{"toolCallId":"call-rt-3","toolName":"run_in_terminal","arguments":{"mode":"sync","command":"cargo test"}}}
{"id":"evt-6","type":"tool.execution_complete","timestamp":"2026-06-02T23:07:05.000Z","data":{"toolCallId":"call-rt-3","success":true}}"#;

    let payload = copilot_payload_from_transcript_reader(
        std::io::Cursor::new(transcript),
        "default",
        Some("stop".to_string()),
    )
    .unwrap();

    let result_event = payload
        .events
        .iter()
        .find(|event| {
            event.event_type.as_deref() == Some("tool.execution_result")
        })
        .expect("expected synthesized tool.execution_result event");
    assert_eq!(
        result_event
            .data_json
            .as_ref()
            .and_then(|json| json.get("blocker"))
            .and_then(serde_json::Value::as_str),
        None
    );

    let record = SessionCaptureRequest::copilot(payload)
        .into_record()
        .expect("payload should map into session record");
    let pack = peek_prompt_pack(
        &record,
        PromptPackOptions {
            preview_chars: 80,
            summarize_threshold_chars: 120,
        },
    );

    assert_eq!(pack.total_turns, 4);
    assert_eq!(pack.dropped_turns, 0);
    assert_eq!(pack.entries.len(), 4);
    assert!(pack.entries.iter().any(|entry| {
        entry.preview.contains("Durable finding")
            && entry.reason == "durable-content"
    }));
}
