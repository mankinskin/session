use feedback_api::EntityUrn;
use serde_json::Value;

use super::{
    FeedbackSignalKind,
    StructuredFeedbackSignal,
    event_outcomes::{
        canonicalize_outcome_events,
        is_tool_execution_outcome,
    },
};
use crate::CopilotHookEvent;

/// Outcome of mapping a failed tool call to a feedback target entity.
///
/// The policy maps only known `ticket-mcp` methods with a single captured
/// ticket identifier. All other outcomes remain explicitly typed instead of
/// guessing an entity target from file paths or multiple candidates.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum FailedToolCallMapping {
    Entity { urn: EntityUrn },
    Unmapped { reason: UnmappedReason },
}

/// Why a failed tool call could not be mapped to one entity.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum UnmappedReason {
    UnknownTool,
    NoEntityIdArgument,
    AmbiguousMultipleCandidates,
    NoSupportedEntityStore,
}

const TICKET_ID_KEYED_METHODS: &[&str] = &[
    "board_check_in",
    "board_check_out",
    "board_update_files",
    "board_rename_file",
];
const TICKET_ID_ALIAS_KEYED_METHODS: &[&str] = &[
    "get_ticket",
    "get_ticket_description",
    "update_ticket",
    "close_ticket",
    "cancel_ticket",
    "delete_ticket",
];
const NO_ENTITY_ID_METHODS: &[&str] = &["create_ticket"];
const AMBIGUOUS_METHODS: &[&str] = &["add_edge", "remove_edge"];
const NO_SUPPORTED_STORE_METHODS: &[&str] = &[
    "read_file",
    "apply_patch",
    "create_file",
    "grep_search",
    "list_dir",
    "run_in_terminal",
    "test_record_execution",
];

/// Map a failed tool call to a feedback target entity based on its captured
/// method suffix and documented single-ticket argument.
pub fn map_failed_tool_call_to_entity(
    tool_name: Option<&str>,
    tool_arguments_json: Option<&Value>,
    workspace_slug: &str,
) -> FailedToolCallMapping {
    let Some(tool_name) = tool_name else {
        return unmapped(UnmappedReason::UnknownTool);
    };

    if AMBIGUOUS_METHODS
        .iter()
        .any(|method| tool_name.ends_with(method))
    {
        return unmapped(UnmappedReason::AmbiguousMultipleCandidates);
    }
    if NO_SUPPORTED_STORE_METHODS
        .iter()
        .any(|method| tool_name.ends_with(method))
    {
        return unmapped(UnmappedReason::NoSupportedEntityStore);
    }
    if NO_ENTITY_ID_METHODS
        .iter()
        .any(|method| tool_name.ends_with(method))
    {
        return unmapped(UnmappedReason::NoEntityIdArgument);
    }

    let id_key = ticket_id_key(tool_name);
    let Some(id_key) = id_key else {
        return unmapped(UnmappedReason::UnknownTool);
    };

    match json_str(tool_arguments_json, id_key)
        .and_then(|id| EntityUrn::ticket(workspace_slug, id).ok())
    {
        Some(urn) => FailedToolCallMapping::Entity { urn },
        None => unmapped(UnmappedReason::NoEntityIdArgument),
    }
}

/// Extract failed-tool-call signals from canonical captured outcome events.
pub fn mine_failed_tool_call_signals(
    events: &[CopilotHookEvent],
    workspace_slug: &str,
) -> Vec<StructuredFeedbackSignal> {
    canonicalize_outcome_events(events)
        .iter()
        .filter_map(|event| detect_failed_tool_call(event, workspace_slug))
        .collect()
}

fn detect_failed_tool_call(
    event: &CopilotHookEvent,
    workspace_slug: &str,
) -> Option<StructuredFeedbackSignal> {
    if !is_tool_execution_outcome(event.event_type.as_deref())
        || event.tool_success != Some(false)
    {
        return None;
    }

    Some(StructuredFeedbackSignal {
        kind: FeedbackSignalKind::FailedToolCall,
        sequence: None,
        tool_name: event.tool_name.clone(),
        tool_call_id: event.tool_call_id.clone(),
        event_id: event.event_id.clone(),
        tool_success: event.tool_success,
        ingestion: None,
        mapping: Some(map_failed_tool_call_to_entity(
            event.tool_name.as_deref(),
            event.tool_arguments_json.as_ref(),
            workspace_slug,
        )),
    })
}

fn ticket_id_key(tool_name: &str) -> Option<&'static str> {
    if TICKET_ID_KEYED_METHODS
        .iter()
        .any(|method| tool_name.ends_with(method))
    {
        Some("ticket_id")
    } else if TICKET_ID_ALIAS_KEYED_METHODS
        .iter()
        .any(|method| tool_name.ends_with(method))
    {
        Some("id")
    } else {
        None
    }
}

fn json_str(
    value: Option<&Value>,
    key: &str,
) -> Option<String> {
    value?.get(key)?.as_str().map(str::to_string)
}

fn unmapped(reason: UnmappedReason) -> FailedToolCallMapping {
    FailedToolCallMapping::Unmapped { reason }
}
