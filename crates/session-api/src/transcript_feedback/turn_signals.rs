use crate::{
    SessionRole,
    SessionTurn,
};

use super::{
    FeedbackSignalKind,
    StructuredFeedbackSignal,
};

/// Extract structured feedback signals from a session's turns.
///
/// This is a pure, side-effect-free classification over structured metadata.
/// It performs no store writes and creates no tickets; callers decide how to
/// act on the returned signals.
pub fn mine_structured_feedback_signals(
    turns: &[SessionTurn]
) -> Vec<StructuredFeedbackSignal> {
    turns.iter().filter_map(detect_signal).collect()
}

fn detect_signal(turn: &SessionTurn) -> Option<StructuredFeedbackSignal> {
    let meta = turn.event_meta.as_ref()?;

    // Only structured tool outcomes are trusted. A `tool_success` of `false`
    // is an explicit failure flag recorded at capture time; no natural-language
    // interpretation is involved.
    if turn.role != SessionRole::Tool || meta.tool_success != Some(false) {
        return None;
    }

    Some(StructuredFeedbackSignal {
        kind: FeedbackSignalKind::FailedToolCall,
        sequence: Some(turn.sequence),
        tool_name: turn.tool_name.clone(),
        tool_call_id: meta.tool_call_id.clone(),
        event_id: meta.event_id.clone(),
        tool_success: meta.tool_success,
        ingestion: None,
        mapping: None,
    })
}
