pub mod audit;
pub mod delegation_cost;
pub mod error;
pub mod escalation;
pub mod follow_up;
pub mod grants;
pub mod hook;
pub mod model;
pub mod move_domain;
pub mod peek;
pub mod price_loader;
pub mod quality_gate;
pub mod store;
pub mod subagent_rollup;
pub mod tool_metrics;
pub mod transcript_feedback;

pub use audit::{
    SessionAuditFinding,
    SessionAuditMetrics,
    SessionAuditReport,
    SessionAuditSelector,
    SessionAuditSeverity,
    SessionAuditToolCount,
};
pub use delegation_cost::{
    CrossAgentDuplicate,
    DelegationCostReport,
    DelegationFailure,
    RepeatCount,
    SubAgentDelegationReport,
    compute_delegation_cost_report,
    compute_delegation_cost_report_from_events,
    normalize_path_for_dedup,
};
pub use error::SessionError;
pub use escalation::{
    EscalationAction,
    EscalationRecord,
    EscalationResolution,
    EscalationStatus,
    create_escalation,
    escalation_marker,
    get_escalation,
    list_escalations,
    parse_escalation_marker,
    resolve_escalation,
};
pub use follow_up::{
    FollowUpSynthesisOutcome,
    FollowUpTicketDraft,
    build_follow_up_ticket_draft,
    follow_up_ticket_id,
    synthesize_follow_up_ticket,
};
pub use grants::{
    BudgetGrant,
    BudgetGrantScope,
    create_grant,
    list_grants,
    revoke_grant,
};
pub use hook::{
    CopilotHookEvent,
    CopilotHookMessage,
    CopilotHookPayload,
    CopilotRuntimeMetadata,
    SessionCaptureRequest,
    ToolResponseOverride,
    copilot_payload_from_transcript_path,
    copilot_payload_from_transcript_path_with_tool_response_override,
    copilot_payload_from_transcript_reader,
};
pub use model::{
    HandoffBacklogFilter,
    SESSION_SCHEMA_VERSION,
    SessionFinishRecord,
    SessionFinishResult,
    SessionHandoffPackage,
    SessionHandoffRecord,
    SessionHandoffResult,
    SessionHandoffTargetTicket,
    SessionHandoffUpwardContextEntry,
    SessionHandoffUpwardContextRole,
    SessionLinks,
    SessionMetadata,
    SessionPinFeedbackSink,
    SessionPinnedEntity,
    SessionPinnedEntityHeader,
    SessionPinnedEntityKind,
    SessionProvisioningDiagnostic,
    SessionRecord,
    SessionRole,
    SessionRunLineage,
    SessionRuntimeContext,
    SessionRuntimeInitRequest,
    SessionRuntimeInitResult,
    SessionRuntimeView,
    SessionTerminalCreateRequest,
    SessionTerminalEvent,
    SessionTerminalManifest,
    SessionTerminalPeekResult,
    SessionTerminalRecord,
    SessionTerminalStatus,
    SessionTicketStateResolver,
    SessionTurn,
    SessionTurnEventMeta,
    SessionValidationGate,
    SessionWorkflowDiagnostic,
    SessionWorkflowEdge,
    SessionWorkflowEdgeKind,
    SessionWorkflowGraph,
    SessionWorkflowNode,
    SessionWorkflowNodeDraft,
    SessionWorkflowNodeKind,
    SessionWorkflowNodePatch,
    SessionWorkflowNodeRequirement,
    SessionWorkflowNodeResolution,
    SessionWorkflowNodeStatus,
    SessionWorkflowSnapshot,
    SessionWorkflowValidationIssue,
    SessionWorktreeAllocationMode,
    SessionWorktreeAssignment,
    SessionWorktreeStatus,
    default_session_schema_version,
    validate_workflow_graph,
};
pub use peek::{
    DEFAULT_PROMPT_SUMMARIZE_THRESHOLD_CHARS,
    DEFAULT_SKELETON_PREVIEW_CHARS,
    PromptInclusion,
    PromptPackOptions,
    SessionPromptPack,
    SessionPromptPackEntry,
    SessionSkeleton,
    SessionSkeletonEntry,
    SessionTurnRange,
    peek_prompt_pack,
    peek_skeleton,
    peek_turn_range,
};
pub use quality_gate::{
    QualityGate,
    QualityGateOutcome,
    QualityGatePhase,
    post_delegation_gate,
    pre_delegation_gate,
};
pub use store::{
    PersistedSessionEvents,
    PersistedSessionManifest,
    PersistedSessionTranscript,
    RelationStrength,
    SessionQuery,
    SessionRuntimePaths,
    SessionStoreConfig,
    SessionStorePaths,
    SessionStorePlan,
    SessionTicketBackfillReport,
    SessionWorktreeCheckInReceipt,
    SessionWorktreeCheckInRequest,
    TicketSessionMatch,
};
pub use subagent_rollup::{
    SubAgentRollup,
    compute_subagent_rollups,
    compute_subagent_rollups_with_events,
};
pub use tool_metrics::{
    CharsPerTokenEstimator,
    GradedCostCalibration,
    SessionToolMetricsSummary,
    TokenEstimator,
    ToolCallSummary,
    ToolMetricsReport,
    ToolMetricsRollup,
    ToolMetricsWindow,
    ToolMetricsWindowDescription,
    ToolTokenStats,
    aggregate,
    aggregate_multi_store,
    aggregate_with_cost,
    compute_session_summary,
    compute_session_summary_with_events,
    graded_cost,
    write_rollup,
};
pub use transcript_feedback::{
    EntityDiscoveryQueue,
    ExplicitIngestionArgs,
    FailedToolCallMapping,
    FeedbackSignalKind,
    StructuredFeedbackSignal,
    UnmappedReason,
    discover_entities_from_signals,
    map_failed_tool_call_to_entity,
    mine_explicit_ingestion_signals,
    mine_failed_tool_call_signals,
    mine_structured_feedback_signals,
    recover_feedback_entry_from_signal,
};
