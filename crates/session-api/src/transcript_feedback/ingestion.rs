use std::str::FromStr;

use feedback_api::{
    EntityUrn,
    FeedbackEntry,
    FeedbackNoteKind,
    FeedbackProvenance,
    FeedbackRating,
    FeedbackSource,
};
use serde_json::Value;

use super::{
    ExplicitIngestionArgs,
    FeedbackSignalKind,
    StructuredFeedbackSignal,
    event_outcomes::{
        canonicalize_outcome_events,
        is_tool_execution_outcome,
    },
};
use crate::CopilotHookEvent;

const FEEDBACK_INGEST_TOOL_SUFFIX: &str = "feedback_ingest";

/// Extract explicit feedback-ingestion signals from captured tool outcomes.
pub fn mine_explicit_ingestion_signals(
    events: &[CopilotHookEvent]
) -> Vec<StructuredFeedbackSignal> {
    canonicalize_outcome_events(events)
        .iter()
        .filter_map(detect_explicit_ingestion)
        .collect()
}

/// Recover an ingestion signal only when its live call did not persist an
/// entry and its captured arguments identify a valid feedback target.
pub fn recover_feedback_entry_from_signal(
    signal: &StructuredFeedbackSignal,
    fallback_session_id: Option<String>,
) -> Result<Option<FeedbackEntry>, String> {
    if signal.kind != FeedbackSignalKind::ExplicitIngestion
        || signal.tool_success == Some(true)
    {
        return Ok(None);
    }
    let Some(ingestion) = signal.ingestion.as_ref() else {
        return Ok(None);
    };
    let (Some(target_raw), Some(source_raw)) =
        (ingestion.target.as_deref(), ingestion.source.as_deref())
    else {
        return Ok(None);
    };

    let source = FeedbackSource::from_str(source_raw)?;
    let target = EntityUrn::from_str(target_raw)?;
    let rating = ingestion
        .rating
        .as_deref()
        .map(FeedbackRating::from_str)
        .transpose()?;
    let note_kind = ingestion
        .note_kind
        .as_deref()
        .map(FeedbackNoteKind::from_str)
        .transpose()?;
    let provenance = FeedbackProvenance::from_session_turn(
        ingestion.session_id.clone().or(fallback_session_id),
        ingestion.author.clone(),
        None,
        signal.sequence,
        signal.tool_call_id.clone(),
    )?;

    FeedbackEntry::new(
        source,
        target,
        rating,
        ingestion.note.clone(),
        note_kind,
        provenance,
    )
    .map(Some)
}

fn detect_explicit_ingestion(
    event: &CopilotHookEvent
) -> Option<StructuredFeedbackSignal> {
    if !is_tool_execution_outcome(event.event_type.as_deref()) {
        return None;
    }
    let tool_name = event.tool_name.as_deref()?;
    if !tool_name.ends_with(FEEDBACK_INGEST_TOOL_SUFFIX) {
        return None;
    }

    let arguments = event.tool_arguments_json.as_ref();
    Some(StructuredFeedbackSignal {
        kind: FeedbackSignalKind::ExplicitIngestion,
        sequence: None,
        tool_name: Some(tool_name.to_string()),
        tool_call_id: event.tool_call_id.clone(),
        event_id: event.event_id.clone(),
        tool_success: event.tool_success,
        ingestion: Some(ExplicitIngestionArgs {
            target: json_str(arguments, "target"),
            source: json_str(arguments, "source"),
            rating: json_str(arguments, "rating"),
            note: json_str(arguments, "note"),
            note_kind: json_str(arguments, "note_kind"),
            session_id: json_str(arguments, "session_id"),
            author: json_str(arguments, "author"),
        }),
        mapping: None,
    })
}

fn json_str(
    value: Option<&Value>,
    key: &str,
) -> Option<String> {
    value?.get(key)?.as_str().map(str::to_string)
}
