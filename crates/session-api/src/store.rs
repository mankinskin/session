use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};
use uuid::Uuid;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    CopilotHookPayload, HandoffBacklogFilter, SESSION_SCHEMA_VERSION, SessionAuditReport,
    SessionAuditSelector, SessionCaptureRequest, SessionError, SessionFinishRecord,
    SessionFinishResult, SessionHandoffPackage, SessionHandoffRecord, SessionHandoffResult,
    SessionLinks, SessionMetadata, SessionPinFeedbackSink, SessionPinnedEntity,
    SessionPinnedEntityHeader, SessionPinnedEntityKind, SessionRecord, SessionRunLineage,
    SessionRuntimeContext, SessionRuntimeInitRequest, SessionRuntimeInitResult, SessionRuntimeView,
    SessionTerminalCreateRequest, SessionTerminalEvent, SessionTerminalManifest,
    SessionTerminalPeekResult, SessionTerminalRecord, SessionTerminalStatus,
    SessionTicketStateResolver, SessionTurn, SessionValidationGate, SessionWorkflowDiagnostic,
    SessionWorkflowEdge, SessionWorkflowEdgeKind, SessionWorkflowNode, SessionWorkflowNodeDraft,
    SessionWorkflowNodeResolution, SessionWorkflowNodeStatus, SessionWorkflowSnapshot,
    SessionWorktreeAllocationMode, SessionWorktreeAssignment, SessionWorktreeStatus,
    audit::build_session_audit_report,
    hook::{
        CopilotHookEvent, ToolResponseOverride, copilot_payload_from_transcript_path,
        copilot_payload_from_transcript_path_with_tool_response_override,
    },
    peek::{
        PromptPackOptions, SessionPromptPack, SessionSkeleton, SessionTurnRange, peek_prompt_pack,
        peek_skeleton, peek_turn_range,
    },
    validate_workflow_graph,
};
use rule_api::RuleStore;
use spec_api::SpecStore;
use test_api::{ExecutionQuery, TestStoreConfig, ValidationOutcome};
use ticket_api::{
    model::parts::ViewProfile,
    query_helpers::resolve_uuid_with_prefix,
    storage::{ReadProjection, TicketStore},
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionQueryResult {
    pub sessions: Vec<SessionRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<SessionQueryDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionQueryDiagnostic {
    pub session_id: String,
    pub code: String,
}

/// Widening relation tiers for [`SessionStoreConfig::sessions_for_ticket`].
/// Each tier includes every match from the tiers before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    pub workspace_path: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorktreeUnassignRequest {
    pub session_id: String,
    pub owner_id: String,
    pub ticket_id: String,
    pub expected_worktree_path: PathBuf,
    pub expected_branch: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorktreeUnassignReceipt {
    pub session_id: String,
    pub main_checkout: PathBuf,
    pub previous_assignment: SessionWorktreeCheckInReceipt,
    pub reason: String,
    pub unassigned_at: DateTime<Utc>,
    pub audit_path: PathBuf,
}

#[path = "store/config.rs"]
mod config;

pub use config::SessionIntakeOperation;

#[cfg(test)]
pub(crate) use config::WorktreeCheckInFailurePoint;

#[path = "store_routing_types.rs"]
mod store_routing_types;
use crate::SessionWorkflowGraph;
pub use store_routing_types::{SessionRuntimePaths, SessionStorePaths};
use store_routing_types::{parse_entity_urn, parse_entity_urn_kind, validate_session_id};

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
    workspace_path: String,
    // Keyed by resolved store root path so path-equivalent URNs open each
    // store at most once.
    ticket_stores: std::sync::Mutex<BTreeMap<PathBuf, TicketStore>>,
    spec_stores: std::sync::Mutex<BTreeMap<PathBuf, SpecStore>>,
}

impl DefaultTicketStateResolver {
    /// Runs `f` against the cached (or freshly opened) ticket store for
    /// `workspace_path`, opening and caching it at most once per resolved store root.
    /// Never creates a store as a side effect: every resolved store must
    /// already exist.
    fn with_ticket_store<T>(
        &self,
        workspace_path: &str,
        f: impl FnOnce(&TicketStore) -> Result<T, String>,
    ) -> Result<T, String> {
        let root = resolve_workspace_store_root(
            &self.session_store_root,
            &self.workspace_path,
            workspace_path,
            ".ticket",
        )?;
        let mut stores = self.ticket_stores.lock().unwrap();
        if !stores.contains_key(&root) {
            if !root.exists() {
                return Err(format!(
                    "ticket store for workspace `{workspace_path}` is unavailable at {}: \
                     not initialized",
                    root.display()
                ));
            }
            let store = TicketStore::open(&root).map_err(|error| {
                format!(
                    "ticket store for workspace `{workspace_path}` is unavailable at {}: {error}",
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
        workspace_path: &str,
        f: impl FnOnce(&SpecStore) -> Result<T, String>,
    ) -> Result<T, String> {
        let root = resolve_workspace_store_root(
            &self.session_store_root,
            &self.workspace_path,
            workspace_path,
            ".spec",
        )?;
        let mut stores = self.spec_stores.lock().unwrap();
        if !stores.contains_key(&root) {
            let is_own_workspace = workspace_path == self.workspace_path;
            if !is_own_workspace && !root.exists() {
                return Err(format!(
                    "spec store for workspace `{workspace_path}` is unavailable at {}: \
                     not initialized",
                    root.display()
                ));
            }
            // `SpecStore::open` never creates the directory as a side effect;
            // the session's own store is expected to already exist (or be
            // absent, in which case sessions with no spec nodes never pay
            // the open cost until a spec URN actually needs resolving).
            let store = SpecStore::open(&root).map_err(|error| error.to_string())?;
            stores.insert(root.clone(), store);
        }
        f(stores.get(&root).expect("just inserted"))
    }
}

impl SessionTicketStateResolver for DefaultTicketStateResolver {
    fn resolve_ticket_state(&self, ticket_urn: &str) -> Result<Option<String>, String> {
        let parsed = parse_entity_urn(ticket_urn).map_err(|error| error.to_string())?;
        if parsed.kind != SessionPinnedEntityKind::Ticket {
            return Err(format!("not a ticket URN: {ticket_urn}"));
        }
        let ticket_id = Uuid::parse_str(&parsed.entity_id)
            .map_err(|error| format!("invalid ticket id in URN {ticket_urn}: {error}"))?;
        self.with_ticket_store(&parsed.workspace_path, |store| {
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

    fn resolve_spec_state(&self, spec_urn: &str) -> Result<Option<String>, String> {
        let parsed = parse_entity_urn(spec_urn).map_err(|error| error.to_string())?;
        if parsed.kind != SessionPinnedEntityKind::Spec {
            return Err(format!("not a spec URN: {spec_urn}"));
        }
        self.with_spec_store(&parsed.workspace_path, |store| {
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
            .ok_or_else(|| SessionError::InvalidStorePath(self.paths.session_dir.clone()))?;
        ensure_local_gitignore(store_root)?;
        fs::create_dir_all(&self.paths.session_dir).map_err(|source| SessionError::Io {
            path: self.paths.session_dir.clone(),
            source,
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
        let tool_metrics_path = self.paths.session_dir.join("tool-metrics.json");
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

#[path = "store/handoff_rendering.rs"]
mod handoff_rendering;
#[allow(unused_imports)]
pub use handoff_rendering::*;

#[path = "store/handoff_ticket_links.rs"]
mod handoff_ticket_links;
#[allow(unused_imports)]
pub use handoff_ticket_links::*;

#[path = "store/workflow_rendering.rs"]
mod workflow_rendering;
#[allow(unused_imports)]
pub use workflow_rendering::*;

#[path = "store/routing_roots.rs"]
mod routing_roots;
#[allow(unused_imports)]
pub use routing_roots::*;
