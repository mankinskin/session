//! Structured feedback signal extraction from captured sessions.
//!
//! Signals are derived **only** from structured metadata — never from
//! free-text heuristics over message content. Content-aware analysis (for
//! example embedding-based reasoning about message context) is intentionally
//! deferred. The previous implementation tokenized message text and matched
//! "confusion markers" plus a two-token keyword overlap against rule bodies,
//! which produced large volumes of false positives when run over captured
//! transcripts. That heuristic has been removed entirely.
//!
//! Two distinct structured sources are mined:
//!
//! - [`SessionTurn`] metadata ([`crate::SessionTurnEventMeta`]), for
//!   [`FeedbackSignalKind::FailedToolCall`].
//! - Captured tool-execution [`crate::CopilotHookEvent`]s, for
//!   [`FeedbackSignalKind::ExplicitIngestion`].
//!
//! These are separate sources, not an oversight: grounding against real
//! captured `.session/sessions/*` transcripts shows that tool call/result
//! pairs are recorded as session **events** (`tool.execution_start` /
//! `tool.execution_complete`; legacy captures may also include
//! `tool.execution_result`), not as `SessionTurn`s — every
//! committed session transcript has zero turns with `role: tool`. A detector
//! that only inspected `SessionTurn`s would therefore never fire on real
//! data for tool-call signals; `ExplicitIngestion` mining reads the events
//! list directly instead of guessing that turn-based metadata is populated.

mod entity_discovery;
mod event_outcomes;
mod failed_tool_calls;
mod ingestion;
mod turn_signals;

pub use entity_discovery::{
    EntityDiscoveryQueue,
    discover_entities_from_signals,
};
pub use failed_tool_calls::{
    FailedToolCallMapping,
    UnmappedReason,
    map_failed_tool_call_to_entity,
    mine_failed_tool_call_signals,
};
pub use ingestion::{
    mine_explicit_ingestion_signals,
    recover_feedback_entry_from_signal,
};
pub use turn_signals::mine_structured_feedback_signals;

/// Classification of a structured feedback signal detected in a captured
/// session.
///
/// Only signals that can be derived unambiguously from captured structured
/// metadata are represented here.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackSignalKind {
    /// A tool invocation whose captured `tool_success` flag was `false`.
    FailedToolCall,
    /// A captured `feedback_ingest` tool call, carrying its structured
    /// arguments in [`StructuredFeedbackSignal::ingestion`].
    ExplicitIngestion,
}

/// The structured arguments captured for an `ExplicitIngestion` signal,
/// copied verbatim from `tool_arguments_json` (the flat parameter object the
/// tool was invoked with). Fields are `Option` because a captured call may
/// be missing an optional argument, or the argument may not have serialized
/// as a plain string; no value here is inferred or guessed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExplicitIngestionArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

/// A backtraceable feedback signal extracted from structured session
/// metadata.
///
/// Every field is sourced from captured metadata so a downstream consumer can
/// trace the signal back to the exact turn and/or tool call that produced
/// it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StructuredFeedbackSignal {
    /// What kind of structured signal was detected.
    pub kind: FeedbackSignalKind,
    /// Turn sequence within the captured session, when the signal was
    /// derived from a `SessionTurn`. `None` for event-derived signals
    /// (`ExplicitIngestion`), which have no numbered turn to reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<usize>,
    /// Name of the tool associated with the signal, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Captured tool-call id, enabling backtracing to the originating call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Captured event id, enabling backtracing to the originating event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// The captured `tool_success` flag for the originating call, when
    /// known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_success: Option<bool>,
    /// Populated only for `ExplicitIngestion` signals: the structured
    /// arguments the `feedback_ingest` tool was invoked with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingestion: Option<ExplicitIngestionArgs>,
    /// Populated only for `FailedToolCall` signals produced by
    /// [`mine_failed_tool_call_signals`]: the outcome of mapping the failing
    /// call to a feedback target entity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping: Option<FailedToolCallMapping>,
}

#[cfg(test)]
mod tests {
    #[path = "entity_discovery.rs"]
    mod entity_discovery;
    #[path = "ingestion.rs"]
    mod ingestion;
    #[path = "turn_signals.rs"]
    mod turn_signals;

    use chrono::Utc;
    use feedback_api::{
        EntityUrn,
        FeedbackRating,
    };
    use serde_json::Value;

    use super::*;
    use crate::{
        CopilotHookEvent,
        SessionRole,
        SessionTurn,
        SessionTurnEventMeta,
    };

    fn failed_tool_call_event(
        tool_name: Option<&str>,
        arguments: Value,
    ) -> CopilotHookEvent {
        CopilotHookEvent {
            event_id: Some("evt-fail-1".to_string()),
            parent_event_id: None,
            event_type: Some("tool.execution_result".to_string()),
            captured_at: Some(Utc::now()),
            turn_id: None,
            message_id: None,
            tool_call_id: Some("call-fail-1".to_string()),
            tool_name: tool_name.map(str::to_string),
            tool_success: Some(false),
            reasoning_text: None,
            tool_requests_json: None,
            tool_arguments_json: Some(arguments),
            data_json: None,
            raw_event_json: None,
        }
    }

    #[test]
    fn maps_ticket_id_keyed_method_to_ticket_entity() {
        // Grounded against the most common real failure: `board_check_out`
        // (23 of 115 failed calls in the committed session store) takes
        // `ticket_id`.
        let mapping = map_failed_tool_call_to_entity(
            Some("mcp_rmcp6_board_check_out"),
            Some(&serde_json::json!({ "ticket_id": "abc123" })),
            "memory-api",
        );

        assert_eq!(
            mapping,
            FailedToolCallMapping::Entity {
                urn: EntityUrn::ticket("memory-api", "abc123").unwrap(),
            }
        );
    }

    #[test]
    fn detects_failed_call_from_execution_complete_event() {
        let mut event = failed_tool_call_event(
            Some("mcp_rmcp6_board_check_out"),
            serde_json::json!({ "ticket_id": "abc123" }),
        );
        event.event_type = Some("tool.execution_complete".to_string());

        let signals = mine_failed_tool_call_signals(&[event], "memory-api");

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].kind, FeedbackSignalKind::FailedToolCall);
    }

    #[test]
    fn deduplicates_failed_call_when_complete_and_result_overlap() {
        let mut complete = failed_tool_call_event(
            Some("mcp_rmcp6_board_check_out"),
            serde_json::json!({ "ticket_id": "abc123" }),
        );
        complete.event_type = Some("tool.execution_complete".to_string());
        let mut result = complete.clone();
        result.event_id = Some("evt-fail-2".to_string());
        result.event_type = Some("tool.execution_result".to_string());

        let signals =
            mine_failed_tool_call_signals(&[complete, result], "memory-api");

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].kind, FeedbackSignalKind::FailedToolCall);
    }

    #[test]
    fn maps_id_keyed_method_to_ticket_entity() {
        // `get_ticket` (and friends) key their ticket reference as `id`.
        let mapping = map_failed_tool_call_to_entity(
            Some("mcp_rmcp6_get_ticket"),
            Some(&serde_json::json!({ "id": "abc123" })),
            "memory-api",
        );

        assert_eq!(
            mapping,
            FailedToolCallMapping::Entity {
                urn: EntityUrn::ticket("memory-api", "abc123").unwrap(),
            }
        );
    }

    #[test]
    fn create_ticket_failure_is_unmapped_no_entity_id_argument() {
        // create_ticket creates a *new* entity; there is no existing entity
        // to reference.
        let mapping = map_failed_tool_call_to_entity(
            Some("mcp_rmcp6_create_ticket"),
            Some(&serde_json::json!({ "title": "x" })),
            "memory-api",
        );

        assert_eq!(
            mapping,
            FailedToolCallMapping::Unmapped {
                reason: UnmappedReason::NoEntityIdArgument,
            }
        );
    }

    #[test]
    fn file_tool_failure_is_unmapped_no_supported_entity_store() {
        let mapping = map_failed_tool_call_to_entity(
            Some("read_file"),
            Some(&serde_json::json!({ "filePath": "src/lib.rs" })),
            "memory-api",
        );

        assert_eq!(
            mapping,
            FailedToolCallMapping::Unmapped {
                reason: UnmappedReason::NoSupportedEntityStore,
            }
        );
    }

    #[test]
    fn edge_tool_failure_is_unmapped_ambiguous() {
        let mapping = map_failed_tool_call_to_entity(
            Some("mcp_rmcp6_add_edge"),
            Some(&serde_json::json!({ "from": "a", "to": "b" })),
            "memory-api",
        );

        assert_eq!(
            mapping,
            FailedToolCallMapping::Unmapped {
                reason: UnmappedReason::AmbiguousMultipleCandidates,
            }
        );
    }

    #[test]
    fn unknown_tool_failure_is_unmapped_unknown_tool() {
        let mapping = map_failed_tool_call_to_entity(None, None, "memory-api");

        assert_eq!(
            mapping,
            FailedToolCallMapping::Unmapped {
                reason: UnmappedReason::UnknownTool,
            }
        );
    }

    #[test]
    fn known_ticket_method_missing_id_argument_is_unmapped() {
        // The tool is known, but this particular call's arguments happen not
        // to carry the id — must not guess a target.
        let mapping = map_failed_tool_call_to_entity(
            Some("mcp_rmcp6_get_ticket"),
            Some(&serde_json::json!({ "workspace": "x" })),
            "memory-api",
        );

        assert_eq!(
            mapping,
            FailedToolCallMapping::Unmapped {
                reason: UnmappedReason::NoEntityIdArgument,
            }
        );
    }

    #[test]
    fn mines_failed_tool_call_signal_with_resolved_mapping_from_events() {
        let event = failed_tool_call_event(
            Some("mcp_rmcp6_board_check_out"),
            serde_json::json!({ "ticket_id": "abc123" }),
        );

        let signals = mine_failed_tool_call_signals(&[event], "memory-api");

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].kind, FeedbackSignalKind::FailedToolCall);
        assert_eq!(signals[0].sequence, None);
        assert_eq!(
            signals[0].mapping,
            Some(FailedToolCallMapping::Entity {
                urn: EntityUrn::ticket("memory-api", "abc123").unwrap(),
            })
        );
    }

    #[test]
    fn ignores_successful_calls_when_mining_failed_tool_call_signals() {
        let mut event = failed_tool_call_event(
            Some("mcp_rmcp6_board_check_out"),
            serde_json::json!({ "ticket_id": "abc123" }),
        );
        event.tool_success = Some(true);

        let signals = mine_failed_tool_call_signals(&[event], "memory-api");

        assert!(signals.is_empty());
    }
}
