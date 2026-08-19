use std::{
    collections::{
        BTreeMap,
        BTreeSet,
    },
    fs,
    io::ErrorKind,
    path::{
        Path,
        PathBuf,
    },
};
use uuid::Uuid;

use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
    de::DeserializeOwned,
};

use crate::{
    CopilotHookPayload,
    HandoffBacklogFilter,
    SESSION_SCHEMA_VERSION,
    SessionAuditReport,
    SessionAuditSelector,
    SessionCaptureRequest,
    SessionError,
    SessionFinishRecord,
    SessionFinishResult,
    SessionHandoffPackage,
    SessionHandoffRecord,
    SessionHandoffResult,
    SessionLinks,
    SessionMetadata,
    SessionPinFeedbackSink,
    SessionPinnedEntity,
    SessionPinnedEntityHeader,
    SessionPinnedEntityKind,
    SessionRecord,
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
    SessionValidationGate,
    SessionWorkflowDiagnostic,
    SessionWorkflowEdge,
    SessionWorkflowEdgeKind,
    SessionWorkflowNode,
    SessionWorkflowNodeDraft,
    SessionWorkflowNodeResolution,
    SessionWorkflowNodeStatus,
    SessionWorkflowSnapshot,
    SessionWorktreeAllocationMode,
    SessionWorktreeAssignment,
    SessionWorktreeStatus,
    audit::build_session_audit_report,
    hook::{
        CopilotHookEvent,
        ToolResponseOverride,
        copilot_payload_from_transcript_path,
        copilot_payload_from_transcript_path_with_tool_response_override,
    },
    peek::{
        PromptPackOptions,
        SessionPromptPack,
        SessionSkeleton,
        SessionTurnRange,
        peek_prompt_pack,
        peek_skeleton,
        peek_turn_range,
    },
    validate_workflow_graph,
};
use rule_api::RuleStore;
use spec_api::SpecStore;
use test_api::{
    ExecutionQuery,
    TestStoreConfig,
    ValidationOutcome,
};
use ticket_api::{
    model::parts::ViewProfile,
    query_helpers::resolve_uuid_with_prefix,
    storage::{
        ReadProjection,
        TicketStore,
    },
};

#[path = "store_persistence_types.rs"]
mod store_persistence_types;
pub use store_persistence_types::*;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Widening relation tiers for [`SessionStoreConfig::sessions_for_ticket`].
/// Each tier includes every match from the tiers before it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum RelationStrength {
    Strict,
    Linked,
    Mentioned,
}

/// One session row matched by [`SessionStoreConfig::sessions_for_ticket`].
/// Does not inline the handoff package body — callers fetch handoff content
/// separately via the existing handoff read paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketSessionMatch {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<PathBuf>,
    pub matched_strength: RelationStrength,
}

/// Result of [`SessionStoreConfig::backfill_ticket_links`]. Counts describe
/// individual signal instances, not distinct sessions: one session can
/// contribute to more than one counter (e.g. a strict-tier `ticket_id` write
/// plus several linked-tier handoff targets).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTicketBackfillReport {
    pub total_sessions: usize,
    pub linked_via_branch: usize,
    pub linked_via_worktree_path: usize,
    pub linked_via_handoff: usize,
    pub skipped_unresolvable_shortid: usize,
    pub skipped_corrupt: usize,
    pub already_linked_untouched: usize,
    pub total_would_link: usize,
    /// True when at least one scanned session already surfaces a handoff
    /// `target_tickets` entry at the mentioned tier without any backfill
    /// write — writing to `links.ticket_ids` still promotes that session to
    /// the strictly stronger linked tier.
    pub handoff_already_at_mentioned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStoreConfig {
    pub root: PathBuf,
    pub workspace_slug: String,
}

#[derive(Debug, Clone)]
struct FederatedSessionEntry {
    store: SessionStoreConfig,
    session_id: String,
    session_dir: PathBuf,
    source_path: PathBuf,
    priority: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorktreeCheckInRequest {
    pub session_id: String,
    pub owner_id: String,
    pub ticket_id: String,
    pub worktree_path: PathBuf,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorktreeCheckInReceipt {
    pub session_id: String,
    pub owner_id: String,
    pub ticket_id: String,
    pub worktree_path: PathBuf,
    pub branch: String,
    pub allocation_mode: SessionWorktreeAllocationMode,
    pub status: SessionWorktreeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_path: Option<PathBuf>,
}

mod config {
    use super::*;

    include!("store/config/federated_sessions.rs");
    include!("store/config/capture_query.rs");
    include!("store/config/worktree_runtime.rs");
    include!("store/config/runtime_workflow.rs");
    include!("store/config/workflow.rs");
    include!("store/config/handoff_finish.rs");
    include!("store/config/handoff_pickup.rs");
    include!("store/config/persistence.rs");
    include!("store/config/worktree_conflicts.rs");
    include!("store/config/tool_metrics.rs");
    include!("store/config/subagent_rollup_query.rs");
    include!("store/config/ticket_relation.rs");
    include!("store/config/ticket_backfill.rs");
    include!("store/config/worktree_capture_inference.rs");
    include!("store/config/terminals.rs");
}

#[cfg(test)]
pub(crate) use config::WorktreeCheckInFailurePoint;

#[path = "store_routing_types.rs"]
mod store_routing_types;
use crate::SessionWorkflowGraph;
pub use store_routing_types::{
    SessionRuntimePaths,
    SessionStorePaths,
};
use store_routing_types::{
    parse_entity_urn,
    parse_entity_urn_kind,
    validate_session_id,
};

fn sibling_store_base(session_store_root: &Path) -> &Path {
    if session_store_root
        .file_name()
        .and_then(|name| name.to_str())
        == Some(".session")
    {
        if let Some(parent) = session_store_root.parent() {
            return parent;
        }
    }
    session_store_root
}

fn sibling_store_root(
    session_store_root: &Path,
    sibling_store_dir: &str,
) -> PathBuf {
    sibling_store_base(session_store_root).join(sibling_store_dir)
}

/// Resolves a URN workspace slug to a sibling store root. The literal slug
/// `default` and the session's own workspace slug both resolve to the
/// existing sibling store (byte-identical to pre-cross-workspace behavior);
/// any other slug resolves to `<base>/<slug>/<sibling_store_dir>` and is
/// validated to reject empty, `.`, `..`, and path-separator segments before
/// any path is built.
fn resolve_slug_store_root(
    session_store_root: &Path,
    session_workspace_slug: &str,
    slug: &str,
    sibling_store_dir: &str,
) -> Result<PathBuf, String> {
    if slug == "default" || slug == session_workspace_slug {
        return Ok(sibling_store_root(session_store_root, sibling_store_dir));
    }
    validate_segment(slug, true).map_err(|error| error.to_string())?;
    Ok(sibling_store_base(session_store_root)
        .join(slug)
        .join(sibling_store_dir))
}

/// RAII guard that releases the runtime mutation lock on drop.
struct RuntimeMutationLock {
    file: fs::File,
}

impl Drop for RuntimeMutationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

struct DefaultTicketStateResolver {
    session_store_root: PathBuf,
    workspace_slug: String,
    // Keyed by resolved store root path (not by raw URN slug) so that the
    // literal `default` alias and the session's own workspace slug share one
    // cache entry and open the store at most once.
    ticket_stores: std::sync::Mutex<BTreeMap<PathBuf, TicketStore>>,
    spec_stores: std::sync::Mutex<BTreeMap<PathBuf, SpecStore>>,
}

impl DefaultTicketStateResolver {
    /// Runs `f` against the cached (or freshly opened) ticket store for
    /// `slug`, opening and caching it at most once per resolved store root.
    /// Never creates a store as a side effect: every resolved store must
    /// already exist.
    fn with_ticket_store<T>(
        &self,
        slug: &str,
        f: impl FnOnce(&TicketStore) -> Result<T, String>,
    ) -> Result<T, String> {
        let root = resolve_slug_store_root(
            &self.session_store_root,
            &self.workspace_slug,
            slug,
            ".ticket",
        )?;
        let mut stores = self.ticket_stores.lock().unwrap();
        if !stores.contains_key(&root) {
            if !root.exists() {
                return Err(format!(
                    "ticket store for workspace `{slug}` is unavailable at {}: \
                     not initialized",
                    root.display()
                ));
            }
            let store = TicketStore::open(&root).map_err(|error| {
                format!(
                    "ticket store for workspace `{slug}` is unavailable at {}: {error}",
                    root.display()
                )
            })?;
            stores.insert(root.clone(), store);
        }
        f(stores.get(&root).expect("just inserted"))
    }

    /// Symmetric to [`Self::with_ticket_store`] for spec stores.
    fn with_spec_store<T>(
        &self,
        slug: &str,
        f: impl FnOnce(&SpecStore) -> Result<T, String>,
    ) -> Result<T, String> {
        let root = resolve_slug_store_root(
            &self.session_store_root,
            &self.workspace_slug,
            slug,
            ".spec",
        )?;
        let mut stores = self.spec_stores.lock().unwrap();
        if !stores.contains_key(&root) {
            let is_own_workspace =
                slug == "default" || slug == self.workspace_slug;
            if !is_own_workspace && !root.exists() {
                return Err(format!(
                    "spec store for workspace `{slug}` is unavailable at {}: \
                     not initialized",
                    root.display()
                ));
            }
            // `SpecStore::open` never creates the directory as a side effect;
            // the session's own store is expected to already exist (or be
            // absent, in which case sessions with no spec nodes never pay
            // the open cost until a spec URN actually needs resolving).
            let store =
                SpecStore::open(&root).map_err(|error| error.to_string())?;
            stores.insert(root.clone(), store);
        }
        f(stores.get(&root).expect("just inserted"))
    }
}

impl SessionTicketStateResolver for DefaultTicketStateResolver {
    fn resolve_ticket_state(
        &self,
        ticket_urn: &str,
    ) -> Result<Option<String>, String> {
        let parsed =
            parse_entity_urn(ticket_urn).map_err(|error| error.to_string())?;
        if parsed.kind != SessionPinnedEntityKind::Ticket {
            return Err(format!("not a ticket URN: {ticket_urn}"));
        }
        let ticket_id =
            Uuid::parse_str(&parsed.entity_id).map_err(|error| {
                format!("invalid ticket id in URN {ticket_urn}: {error}")
            })?;
        self.with_ticket_store(&parsed.workspace_slug, |store| {
            match store
                .get_indexed(&ticket_id)
                .map_err(|error| error.to_string())?
            {
                // A resolved ticket may legitimately have no recorded state; keep
                // that distinct from an absent ticket, which is an
                // unavailable-state error.
                Some(indexed) => Ok(indexed.state),
                None => Err(format!("required ticket not found: {ticket_urn}")),
            }
        })
    }

    fn resolve_spec_state(
        &self,
        spec_urn: &str,
    ) -> Result<Option<String>, String> {
        let parsed =
            parse_entity_urn(spec_urn).map_err(|error| error.to_string())?;
        if parsed.kind != SessionPinnedEntityKind::Spec {
            return Err(format!("not a spec URN: {spec_urn}"));
        }
        self.with_spec_store(&parsed.workspace_slug, |store| {
            let manifest = store
                .get(&parsed.entity_id)
                .map_err(|error| format!("required spec not found: {error}"))?;
            Ok(manifest.state().map(str::to_string))
        })
    }
}

fn validation_outcome_label(outcome: ValidationOutcome) -> String {
    match outcome {
        ValidationOutcome::Passed => "passed".to_string(),
        ValidationOutcome::Failed => "failed".to_string(),
        ValidationOutcome::Blocked => "blocked".to_string(),
    }
}

fn node_is_effectively_done(
    node: &SessionWorkflowNode,
    live_state: Option<&Option<String>>,
) -> bool {
    if node.kind == crate::SessionWorkflowNodeKind::Ticket {
        // Ticket-backed nodes derive completion exclusively from authoritative
        // live terminal state. Local `Done` status is display/cache only and can
        // never certify completion. When live state is missing or unavailable
        // (no resolution recorded), the node fails closed and is not done.
        return matches!(
            live_state.and_then(|value| value.as_deref()),
            Some("done") | Some("cancelled")
        );
    }

    if node.kind == crate::SessionWorkflowNodeKind::Spec {
        // Spec-backed nodes are symmetric to tickets: completion is certified
        // only by the authoritative live spec terminal state. `verified` is the
        // spec success terminal; `deprecated` and `cancelled` are terminal exit
        // paths. Any other or unavailable state fails closed.
        return matches!(
            live_state.and_then(|value| value.as_deref()),
            Some("verified") | Some("deprecated") | Some("cancelled")
        );
    }

    // Session-only nodes (task/validation) use local status.
    node.status == SessionWorkflowNodeStatus::Done
}

fn render_handoff_record_terminal(record: &SessionHandoffRecord) -> String {
    let mut lines = Vec::new();
    lines.push(format!("handoff {}", record.handoff_id));
    lines.push(format!("session_id: {}", record.session_id));
    lines.push(format!("outgoing_run_id: {}", record.outgoing_run_id));
    lines.push(format!("resume: {}", record.resume_command));
    if !record.objective.is_empty() {
        lines.push(format!("objective: {}", record.objective));
    }
    if !record.target_tickets.is_empty() {
        lines.push(format!(
            "target_tickets: {}",
            record
                .target_tickets
                .iter()
                .map(|ticket| ticket.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !record.target_files.is_empty() {
        lines.push(format!("target_files: {}", record.target_files.join(", ")));
    }
    if !record.open_escalations.is_empty() {
        lines.push(format!(
            "open_escalations: {}",
            record.open_escalations.join(", ")
        ));
        lines.push("implementation_ready: false".to_string());
    } else if !record.objective.is_empty() {
        lines.push("implementation_ready: true".to_string());
    }
    lines.push("workflow:".to_string());
    lines.push(format!("  nodes: {}", record.workflow.workflow.nodes.len()));
    lines.push(format!("  edges: {}", record.workflow.workflow.edges.len()));
    let blocked = record
        .workflow
        .workflow
        .nodes
        .iter()
        .filter(|node| node.status != SessionWorkflowNodeStatus::Done)
        .count();
    lines.push(format!("  not_done_nodes: {}", blocked));
    for pin in &record.pinned_entities {
        lines.push(format!(
            "pin {} {}",
            pin.urn,
            format!("{:?}", pin.kind).to_lowercase()
        ));
    }
    for gate in &record.validation {
        lines.push(format!(
            "validation {} required={} outcome={}",
            gate.validation_spec_id,
            gate.required,
            gate.outcome.as_deref().unwrap_or("-")
        ));
    }
    for diag in &record.workflow.diagnostics {
        lines.push(format!(
            "diag {} {} {}",
            diag.node_id, diag.code, diag.message
        ));
    }
    lines.join("\n")
}

fn render_handoff_record_markdown(
    record: &SessionHandoffRecord,
    ticket_store: Option<&TicketStore>,
) -> String {
    let mut sections = Vec::new();

    // Header
    sections.push(format!("# Handoff: {}", record.handoff_id));
    sections.push(String::new());

    if !record.higher_level_objective.is_empty() {
        sections.push(linkify_handoff_prose(
            &record.higher_level_objective,
            ticket_store,
        ));
        sections.push(String::new());
    }

    if !record.upward_context.is_empty() {
        sections.push("## Upward Context".to_string());
        let mut breadcrumb = record
            .upward_context
            .iter()
            .map(render_handoff_upward_context_entry)
            .collect::<Vec<_>>();
        if !record.target_tickets.is_empty() {
            breadcrumb.push(
                record
                    .target_tickets
                    .iter()
                    .map(|ticket| {
                        resolve_handoff_ticket_display(
                            record,
                            ticket,
                            ticket_store,
                        )
                        .reference
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        sections.push(breadcrumb.join(" -> "));
        sections.push(String::new());
    }

    // Summary section
    sections.push("## Summary".to_string());
    sections.push(format!("- **Workspace Session**: `{}`", record.session_id));
    sections.push(format!("- **Outgoing Run**: `{}`", record.outgoing_run_id));
    sections.push(format!("- **Created**: {}", record.created_at.to_rfc3339()));
    if !record.objective.is_empty() {
        sections.push(format!("- **Objective**: {}", record.objective));
    }
    let implementation_ready =
        !record.objective.is_empty() && record.open_escalations.is_empty();
    sections.push(format!(
        "- **Implementation Ready**: {}",
        implementation_ready
    ));
    sections.push(String::new());

    // Resume command
    sections.push("## Resume Command".to_string());
    sections.push("```bash".to_string());
    sections.push(record.resume_command.clone());
    sections.push("```".to_string());
    sections.push(String::new());

    // Target Tickets
    if !record.target_tickets.is_empty() {
        sections.push("## Target Tickets".to_string());
        sections.push("| Ticket | What it does | Why |".to_string());
        sections.push("| --- | --- | --- |".to_string());
        for ticket in &record.target_tickets {
            let display =
                resolve_handoff_ticket_display(record, ticket, ticket_store);
            sections.push(format!(
                "| {} | {} | {} |",
                display.reference,
                markdown_table_cell(&display.what),
                markdown_table_cell(&linkify_handoff_prose(
                    &ticket.why,
                    ticket_store
                )),
            ));
        }
        sections.push(String::new());
    }

    // Target Files
    if !record.target_files.is_empty() {
        sections.push("## Target Files".to_string());
        for file in &record.target_files {
            sections.push(format!("- `{}`", file));
        }
        sections.push(String::new());
    }

    // Decisions
    if !record.decisions.is_empty() {
        sections.push("## Decisions".to_string());
        for decision in &record.decisions {
            sections.push(format!(
                "- {}",
                linkify_handoff_prose(decision, ticket_store)
            ));
        }
        sections.push(String::new());
    }

    // Non-Goals
    if !record.non_goals.is_empty() {
        sections.push("## Non-Goals".to_string());
        for non_goal in &record.non_goals {
            sections.push(format!(
                "- {}",
                linkify_handoff_prose(non_goal, ticket_store)
            ));
        }
        sections.push(String::new());
    }

    // Context Anchors
    if !record.context_anchors.is_empty() {
        sections.push("## Context Anchors".to_string());
        for anchor in &record.context_anchors {
            sections.push(format!(
                "- {}",
                linkify_handoff_prose(anchor, ticket_store)
            ));
        }
        sections.push(String::new());
    }

    // Open Escalations
    if !record.open_escalations.is_empty() {
        sections.push("## ⚠️ Open Escalations".to_string());
        for escalation in &record.open_escalations {
            sections.push(format!("- {}", escalation));
        }
        sections.push(String::new());
    }

    // Risk Notes
    if let Some(ref risk_notes) = record.risk_notes {
        sections.push("## Risk Notes".to_string());
        sections.push(linkify_handoff_prose(risk_notes, ticket_store));
        sections.push(String::new());
    }

    // Workflow
    sections.push("## Workflow".to_string());
    sections.push(format!(
        "- **Nodes**: {}",
        record.workflow.workflow.nodes.len()
    ));
    sections.push(format!(
        "- **Edges**: {}",
        record.workflow.workflow.edges.len()
    ));
    let not_done = record
        .workflow
        .workflow
        .nodes
        .iter()
        .filter(|node| node.status != SessionWorkflowNodeStatus::Done)
        .count();
    sections.push(format!("- **Not Done**: {}", not_done));
    if !record.workflow.workflow.nodes.is_empty() {
        // Blank line required so Markdown renderers don't treat the fence as list continuation.
        sections.push(String::new());
        sections.push("```mermaid".to_string());
        sections.push(render_workflow_mermaid(
            &record.workflow.workflow,
            &record.workflow.resolutions,
        ));
        sections.push("```".to_string());
    }
    sections.push(String::new());

    // Pinned Entities
    if !record.pinned_entities.is_empty() {
        sections.push("## Pinned Entities".to_string());
        for pin in &record.pinned_entities {
            sections.push(format!(
                "- `{}` ({})",
                pin.urn,
                format!("{:?}", pin.kind).to_lowercase()
            ));
        }
        sections.push(String::new());
    }

    // Validation
    if !record.validation.is_empty() {
        sections.push("## Validation".to_string());
        for gate in &record.validation {
            let outcome = gate.outcome.as_deref().unwrap_or("-");
            let required = if gate.required {
                "required"
            } else {
                "optional"
            };
            sections.push(format!(
                "- `{}`: {} ({})",
                gate.validation_spec_id, outcome, required
            ));
        }
        sections.push(String::new());
    }

    // Diagnostics
    if !record.workflow.diagnostics.is_empty() {
        sections.push("## Diagnostics".to_string());
        for diag in &record.workflow.diagnostics {
            sections.push(format!(
                "- **{}** [{}]: {}",
                diag.node_id, diag.code, diag.message
            ));
        }
        sections.push(String::new());
    }

    sections.join("\n")
}

struct HandoffTicketDisplay {
    reference: String,
    what: String,
}

struct ResolvedHandoffTicket {
    id: Uuid,
    title: Option<String>,
    what: String,
}

fn resolve_handoff_ticket_display(
    record: &SessionHandoffRecord,
    ticket: &crate::SessionHandoffTargetTicket,
    ticket_store: Option<&TicketStore>,
) -> HandoffTicketDisplay {
    let cached_title = record
        .workflow
        .workflow
        .nodes
        .iter()
        .find(|node| {
            node.ticket_urn
                .as_deref()
                .is_some_and(|urn| urn.ends_with(&ticket.id))
        })
        .and_then(|node| node.cached_ticket_title.as_deref())
        .unwrap_or(&ticket.id);
    let resolved = ticket_store.and_then(|store| {
        resolve_handoff_ticket(store, &ticket.id).map(|ticket| {
            (
                ticket.id,
                ticket.title.unwrap_or_else(|| cached_title.to_string()),
                ticket.what,
            )
        })
    });
    match resolved {
        Some((id, title, what)) => HandoffTicketDisplay {
            reference: handoff_ticket_reference(&id.to_string(), &title),
            what,
        },
        None => HandoffTicketDisplay {
            reference: format!("{} {}", ticket.id, cached_title),
            what: String::new(),
        },
    }
}

fn resolve_handoff_ticket(
    ticket_store: &TicketStore,
    ticket_id: &str,
) -> Option<ResolvedHandoffTicket> {
    let id = resolve_uuid_with_prefix(ticket_store, ticket_id).ok()?;
    let projection = ticket_store
        .project(&id, &ReadProjection::Profile(ViewProfile::Summary))
        .ok()?;
    let title = projection
        .fields
        .get("title")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let what = projection
        .parts
        .iter()
        .find(|part| part.kind == "objective")
        .map(|part| part.content.clone())
        .unwrap_or_default();
    Some(ResolvedHandoffTicket { id, title, what })
}

fn handoff_ticket_reference(
    ticket_id: &str,
    title: &str,
) -> String {
    format!(
        "[{} {}](.ticket/tickets/{ticket_id}/ticket.toml)",
        ticket_id.chars().take(8).collect::<String>(),
        title,
    )
}

fn render_handoff_upward_context_entry(
    entry: &crate::SessionHandoffUpwardContextEntry
) -> String {
    let title = parse_entity_urn(&entry.entity_urn)
        .ok()
        .filter(|parsed| parsed.kind == SessionPinnedEntityKind::Ticket)
        .map(|parsed| handoff_ticket_reference(&parsed.entity_id, &entry.title))
        .unwrap_or_else(|| entry.title.clone());
    format!("{} ({})", title, format!("{:?}", entry.role).to_lowercase())
}

const MAX_HANDOFF_PROSE_TICKET_REFERENCES: usize = 128;

fn linkify_handoff_prose(
    value: &str,
    ticket_store: Option<&TicketStore>,
) -> String {
    let Some(ticket_store) = ticket_store else {
        return value.to_string();
    };

    let mut output = String::with_capacity(value.len());
    let mut in_fence = false;
    let mut replacements = 0;
    for line in value.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            output.push_str(line);
        } else if in_fence {
            output.push_str(line);
        } else {
            output.push_str(&linkify_handoff_prose_line(
                line,
                ticket_store,
                &mut replacements,
            ));
        }
    }
    output
}

fn linkify_handoff_prose_line(
    line: &str,
    ticket_store: &TicketStore,
    replacements: &mut usize,
) -> String {
    let bytes = line.as_bytes();
    let mut output = String::with_capacity(line.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            let end = line[index + 1..]
                .find('`')
                .map(|offset| index + offset + 2)
                .unwrap_or(bytes.len());
            output.push_str(&line[index..end]);
            index = end;
            continue;
        }
        if bytes[index] == b'['
            && let Some(label_end) = line[index..].find("](")
            && let Some(target_end) = line[index + label_end + 2..].find(')')
        {
            let end = index + label_end + target_end + 3;
            output.push_str(&line[index..end]);
            index = end;
            continue;
        }
        if *replacements < MAX_HANDOFF_PROSE_TICKET_REFERENCES
            && let Some(ticket_id) = bare_ticket_id_at(line, index)
            && let Some(resolved) =
                resolve_handoff_ticket(ticket_store, ticket_id)
            && let Some(title) = resolved.title
        {
            output.push_str(&handoff_ticket_reference(
                &resolved.id.to_string(),
                &title,
            ));
            index += ticket_id.len();
            *replacements += 1;
            continue;
        }
        let character = line[index..].chars().next().expect("valid UTF-8");
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn bare_ticket_id_at(
    value: &str,
    index: usize,
) -> Option<&str> {
    let bytes = value.as_bytes();
    if index > 0 && is_ticket_id_word_byte(bytes[index - 1]) {
        return None;
    }
    let remaining = &value[index..];
    let length = if remaining.len() >= 36 && is_uuid_token(&remaining[..36]) {
        36
    } else if remaining.len() >= 8
        && remaining[..8].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        8
    } else {
        return None;
    };
    if bytes
        .get(index + length)
        .is_some_and(|byte| is_ticket_id_word_byte(*byte))
    {
        return None;
    }
    Some(&value[index..index + length])
}

fn is_uuid_token(value: &str) -> bool {
    value.bytes().enumerate().all(|(index, byte)| {
        matches!(index, 8 | 13 | 18 | 23) && byte == b'-'
            || !matches!(index, 8 | 13 | 18 | 23) && byte.is_ascii_hexdigit()
    })
}

fn is_ticket_id_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn markdown_table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', "<br>")
}

fn sort_workflow_graph(graph: &mut crate::SessionWorkflowGraph) {
    graph
        .nodes
        .sort_by(|left, right| left.node_id.cmp(&right.node_id));
    graph.edges.sort_by(|left, right| {
        left.from
            .cmp(&right.from)
            .then_with(|| left.to.cmp(&right.to))
            .then_with(|| {
                format!("{:?}", left.kind).cmp(&format!("{:?}", right.kind))
            })
    });
}

fn workflow_status_label(status: SessionWorkflowNodeStatus) -> &'static str {
    match status {
        SessionWorkflowNodeStatus::Pending => "pending",
        SessionWorkflowNodeStatus::InProgress => "in-progress",
        SessionWorkflowNodeStatus::Blocked => "blocked",
        SessionWorkflowNodeStatus::Done => "done",
        SessionWorkflowNodeStatus::Deferred => "deferred",
    }
}

fn mermaid_node_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('n');
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn escape_mermaid_label(label: &str) -> String {
    label
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

fn render_workflow_mermaid(
    workflow: &crate::SessionWorkflowGraph,
    resolutions: &[crate::SessionWorkflowNodeResolution],
) -> String {
    let live_states = resolutions
        .iter()
        .map(|item| (item.node_id.clone(), item.live_ticket_state.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut lines = vec!["flowchart TD".to_string()];
    for node in &workflow.nodes {
        let req = match node.requirement {
            crate::SessionWorkflowNodeRequirement::Required => "req",
            crate::SessionWorkflowNodeRequirement::Optional => "opt",
        };
        let live = live_states
            .get(&node.node_id)
            .and_then(|state| state.as_deref())
            .unwrap_or("-");
        let label = format!(
            "{} |{}| |{}| |ticket:{}|",
            node.title,
            req,
            workflow_status_label(node.status),
            live
        );
        lines.push(format!(
            "  {}[\"{}\"]",
            mermaid_node_id(&node.node_id),
            escape_mermaid_label(&label)
        ));
    }

    for edge in &workflow.edges {
        let arrow = match edge.kind {
            SessionWorkflowEdgeKind::DependsOn => "-->|depends_on|",
            SessionWorkflowEdgeKind::Order => "-->|order|",
        };
        lines.push(format!(
            "  {} {} {}",
            mermaid_node_id(&edge.from),
            arrow,
            mermaid_node_id(&edge.to)
        ));
    }

    lines.join("\n")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionStorePlan {
    pub record: SessionRecord,
    pub paths: SessionStorePaths,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<PersistedSessionEvents>,
}

impl SessionStorePlan {
    pub fn manifest(&self) -> PersistedSessionManifest {
        PersistedSessionManifest::from(&self.record)
    }

    pub fn transcript(&self) -> PersistedSessionTranscript {
        PersistedSessionTranscript::from(&self.record)
    }

    pub fn persist(&self) -> Result<SessionStorePaths, SessionError> {
        let store_root = self
            .paths
            .session_dir
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| {
                SessionError::InvalidStorePath(self.paths.session_dir.clone())
            })?;
        ensure_local_gitignore(store_root)?;
        fs::create_dir_all(&self.paths.session_dir).map_err(|source| {
            SessionError::Io {
                path: self.paths.session_dir.clone(),
                source,
            }
        })?;

        let manifest = merge_manifest(
            read_json_if_exists(&self.paths.manifest_path)?,
            self.manifest(),
        );
        let transcript = merge_transcript(
            read_json_if_exists(&self.paths.transcript_path)?,
            self.transcript(),
        )?;

        let merged_events = merge_events(
            read_json_if_exists(&self.paths.events_path)?,
            self.events.clone(),
            self.record.session_id.clone(),
            self.record.captured_at,
        )?;

        write_json(&self.paths.manifest_path, &manifest)?;
        write_json(&self.paths.transcript_path, &transcript)?;
        if let Some(events) = &merged_events {
            write_json(&self.paths.events_path, events)?;
        }

        // Populate tool-metrics.json at capture time (ticket b7c61f0e AC4) so
        // newly captured sessions always have an up-to-date summary reflecting
        // the full merged transcript AND event stream. Tool telemetry lives in
        // the captured events, not in `role: tool` turns, which the Copilot
        // transcript never produces.
        let merged_record = SessionRecord {
            schema_version: manifest.schema_version,
            session_id: manifest.session_id.clone(),
            source: manifest.source.clone(),
            started_at: manifest.started_at,
            captured_at: manifest.captured_at,
            metadata: manifest.metadata.clone(),
            turns: transcript.turns.clone(),
            links: manifest.links.clone(),
            track_id: manifest.track_id.clone(),
            anchor_ticket_id: manifest.anchor_ticket_id.clone(),
            parent_session_id: manifest.parent_session_id.clone(),
            spawned_session_id: manifest.spawned_session_id.clone(),
            emitted_handoff_ids: manifest.emitted_handoff_ids.clone(),
            picked_up_handoff_ids: manifest.picked_up_handoff_ids.clone(),
        };
        let estimator = crate::tool_metrics::CharsPerTokenEstimator::default();
        let summary = crate::tool_metrics::compute_session_summary_with_events(
            &merged_record,
            merged_events
                .as_ref()
                .map(|events| events.events.as_slice())
                .unwrap_or_default(),
            &estimator,
        );
        // Create the sidecar lazily: a session with no observed tool call must
        // not leave an empty `tool-metrics.json` behind.
        let tool_metrics_path =
            self.paths.session_dir.join("tool-metrics.json");
        if summary.is_empty() {
            remove_file_if_exists(&tool_metrics_path)?;
        } else {
            write_json(&tool_metrics_path, &summary)?;
        }

        Ok(self.paths.clone())
    }
}

#[path = "store_helpers.rs"]
mod store_helpers;
use store_helpers::*;

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
