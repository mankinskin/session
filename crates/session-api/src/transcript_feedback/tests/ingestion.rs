use chrono::Utc;
use feedback_api::FeedbackRating;
use serde_json::Value;

use super::super::super::*;

fn feedback_ingest_result_event(
    tool_success: Option<bool>,
    arguments: Value,
) -> CopilotHookEvent {
    CopilotHookEvent {
        event_id: Some("evt-ingest-1".to_string()),
        parent_event_id: None,
        event_type: Some("tool.execution_result".to_string()),
        captured_at: Some(Utc::now()),
        turn_id: None,
        message_id: None,
        tool_call_id: Some("call-ingest-1".to_string()),
        tool_name: Some("mcp_rmcp5_feedback_ingest".to_string()),
        tool_success,
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: Some(arguments),
        data_json: None,
        raw_event_json: None,
    }
}

fn ingest_arguments() -> Value {
    serde_json::json!({
        "workspace": "c:/repo/memory-api",
        "workspace_slug": "memory-api",
        "source": "agent",
        "target": "ce://memory-api/rule/some-rule",
        "rating": "not-helpful",
        "note": "confusing wording",
        "note_kind": "note",
        "session_id": "session-ingest-1",
        "author": "copilot-gpt5"
    })
}

#[test]
fn detects_explicit_ingestion_tool_call_from_events() {
    let event = feedback_ingest_result_event(Some(false), ingest_arguments());
    let signals = mine_explicit_ingestion_signals(&[event]);

    assert_eq!(signals.len(), 1);
    let signal = &signals[0];
    assert_eq!(signal.kind, FeedbackSignalKind::ExplicitIngestion);
    assert_eq!(signal.sequence, None);
    assert_eq!(signal.tool_call_id.as_deref(), Some("call-ingest-1"));
    assert_eq!(signal.tool_success, Some(false));
    let ingestion = signal.ingestion.as_ref().expect("ingestion payload");
    assert_eq!(
        ingestion.target.as_deref(),
        Some("ce://memory-api/rule/some-rule")
    );
    assert_eq!(ingestion.rating.as_deref(), Some("not-helpful"));
    assert_eq!(ingestion.session_id.as_deref(), Some("session-ingest-1"));
}

#[test]
fn detects_explicit_ingestion_from_execution_complete_event() {
    let mut event =
        feedback_ingest_result_event(Some(false), ingest_arguments());
    event.event_type = Some("tool.execution_complete".to_string());

    let signals = mine_explicit_ingestion_signals(&[event]);

    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].kind, FeedbackSignalKind::ExplicitIngestion);
}

#[test]
fn deduplicates_explicit_ingestion_when_complete_and_result_overlap() {
    let mut complete =
        feedback_ingest_result_event(Some(false), ingest_arguments());
    complete.event_type = Some("tool.execution_complete".to_string());
    let mut result = complete.clone();
    result.event_id = Some("evt-ingest-2".to_string());
    result.event_type = Some("tool.execution_result".to_string());

    let signals = mine_explicit_ingestion_signals(&[complete, result]);

    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].kind, FeedbackSignalKind::ExplicitIngestion);
}

#[test]
fn ignores_non_ingest_tool_calls_and_non_result_events() {
    let mut other_tool =
        feedback_ingest_result_event(Some(true), ingest_arguments());
    other_tool.tool_name = Some("mcp_rmcp5_feedback_inbox".to_string());
    let mut wrong_event_type =
        feedback_ingest_result_event(Some(false), ingest_arguments());
    wrong_event_type.event_type = Some("tool.execution_start".to_string());

    let signals =
        mine_explicit_ingestion_signals(&[other_tool, wrong_event_type]);

    assert!(signals.is_empty());
}

#[test]
fn recovers_feedback_entry_for_failed_ingestion_call() {
    let event = feedback_ingest_result_event(Some(false), ingest_arguments());
    let signals = mine_explicit_ingestion_signals(&[event]);

    let entry = recover_feedback_entry_from_signal(&signals[0], None)
        .unwrap()
        .expect("recovered entry");

    assert_eq!(entry.target.to_string(), "ce://memory-api/rule/some-rule");
    assert_eq!(entry.rating, Some(FeedbackRating::NotHelpful));
    assert_eq!(entry.note_text.as_deref(), Some("confusing wording"));
    assert_eq!(
        entry.provenance.session_id.as_deref(),
        Some("session-ingest-1")
    );
    assert_eq!(
        entry.provenance.tool_call_id.as_deref(),
        Some("call-ingest-1")
    );
    assert_eq!(entry.provenance.turn_sequence, None);
}

#[test]
fn does_not_duplicate_a_successfully_persisted_ingestion_call() {
    let event = feedback_ingest_result_event(Some(true), ingest_arguments());
    let signals = mine_explicit_ingestion_signals(&[event]);

    let recovered =
        recover_feedback_entry_from_signal(&signals[0], None).unwrap();
    assert!(recovered.is_none());
}

#[test]
fn skips_recovery_when_required_arguments_are_missing() {
    let mut arguments = ingest_arguments();
    arguments.as_object_mut().unwrap().remove("target");
    let event = feedback_ingest_result_event(Some(false), arguments);
    let signals = mine_explicit_ingestion_signals(&[event]);

    let recovered =
        recover_feedback_entry_from_signal(&signals[0], None).unwrap();
    assert!(recovered.is_none());
}
