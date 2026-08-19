use chrono::TimeZone;
use pretty_assertions::assert_eq;

use crate::{
    SessionError,
    SessionRole,
};

use super::super::{
    CopilotHookMessage,
    CopilotHookPayload,
    SessionCaptureRequest,
};

fn sample_time() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc
        .with_ymd_and_hms(2026, 6, 2, 12, 30, 0)
        .single()
        .unwrap()
}

#[test]
fn capture_request_maps_hook_payload_into_session_record() {
    let payload = CopilotHookPayload {
        session_id: "session-123".to_string(),
        workspace_slug: "context-engine".to_string(),
        captured_at: sample_time(),
        conversation_id: Some("conversation-42".to_string()),
        agent_id: Some("github-copilot-gpt-5.4".to_string()),
        model: Some("GPT-5.4".to_string()),
        trigger: Some("post-turn".to_string()),
        provisioning: None,
        messages: vec![
            CopilotHookMessage {
                role: SessionRole::User,
                content: "Create the session scaffold".to_string(),
                tool_name: None,
                captured_at: Some(sample_time()),
                event_meta: None,
            },
            CopilotHookMessage {
                role: SessionRole::Assistant,
                content: "Scaffold planned.".to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: None,
            },
        ],
        events: vec![],
        runtime: None,
    };
    let mut request = SessionCaptureRequest::copilot(payload);
    request.links.ticket_ids.push("ticket-session".to_string());

    let (record, events) = request.into_record_and_events().unwrap();

    assert_eq!(record.session_id, "session-123");
    assert!(events.is_empty());
    assert_eq!(record.source, "copilot-hook");
    assert_eq!(record.metadata.workspace_slug, "context-engine");
    assert_eq!(record.metadata.ticket_id, None);
    assert_eq!(record.metadata.worktree, None);
    assert_eq!(record.turns.len(), 2);
    assert_eq!(record.turns[0].model, None);
    assert_eq!(record.turns[1].model.as_deref(), Some("GPT-5.4"));
    assert!(record.links.links_to_ticket("ticket-session"));
    assert!(record.has_turns());
}

#[test]
fn capture_request_rejects_missing_session_id() {
    let payload = CopilotHookPayload {
        session_id: "   ".to_string(),
        workspace_slug: "context-engine".to_string(),
        captured_at: sample_time(),
        conversation_id: None,
        agent_id: None,
        model: None,
        trigger: None,
        provisioning: None,
        messages: vec![CopilotHookMessage {
            role: SessionRole::User,
            content: "hello".to_string(),
            tool_name: None,
            captured_at: None,
            event_meta: None,
        }],
        events: vec![],
        runtime: None,
    };

    let error = SessionCaptureRequest::copilot(payload)
        .into_record()
        .unwrap_err();
    assert!(matches!(error, SessionError::MissingSessionId));
}
