use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};

use super::{
    SessionPinnedEntityHeader,
    SessionValidationGate,
    SessionWorkflowSnapshot,
};

/// Role of an entry in the ordered program context above a handoff's leaf work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionHandoffUpwardContextRole {
    #[serde(alias = "initiative")]
    Epic,
    #[serde(alias = "stage")]
    Phase,
    #[serde(alias = "parent-work")]
    Parent,
}

/// A higher-level program entity that gives the handoff's implementation work context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHandoffUpwardContextEntry {
    pub entity_urn: String,
    pub title: String,
    pub role: SessionHandoffUpwardContextRole,
}

/// A ticket included in a handoff together with its local implementation context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionHandoffTargetTicket {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SessionHandoffTargetTicketRepr {
    Legacy(String),
    Structured {
        id: String,
        #[serde(default)]
        why: String,
        #[serde(default)]
        state: String,
        #[serde(default)]
        acceptance_criteria: Vec<String>,
    },
}

impl<'de> Deserialize<'de> for SessionHandoffTargetTicket {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match SessionHandoffTargetTicketRepr::deserialize(deserializer)? {
            SessionHandoffTargetTicketRepr::Legacy(id) => Ok(Self {
                id,
                why: String::new(),
                state: String::new(),
                acceptance_criteria: Vec::new(),
            }),
            SessionHandoffTargetTicketRepr::Structured {
                id,
                why,
                state,
                acceptance_criteria,
            } => Ok(Self {
                id,
                why,
                state,
                acceptance_criteria,
            }),
        }
    }
}

/// Handoff-package schema fields supplied by the caller to describe the next
/// implementation unit.  All fields are optional at the type level but the
/// store enforces required-field completeness when a package is provided.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHandoffPackage {
    /// The single goal of the next implementation unit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub objective: String,
    /// Tickets expected to be worked in the next session and their local context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_tickets: Vec<SessionHandoffTargetTicket>,
    /// Why the current implementation unit matters to the broader program.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub higher_level_objective: String,
    /// Ordered ancestor chain from program context to the current leaf work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upward_context: Vec<SessionHandoffUpwardContextEntry>,
    /// Workspace-relative file paths expected to be touched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_files: Vec<String>,
    /// Resolved design choices so the next session does not re-decide.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    /// Explicit out-of-scope boundaries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_goals: Vec<String>,
    /// Prior findings, links, and ids needed so no search is required.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_anchors: Vec<String>,
    /// Must be empty for the package to be implementation-ready.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_escalations: Vec<String>,
    /// Known risks or fragile areas (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_notes: Option<String>,
    /// Id of the handoff this one supersedes (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_handoff: Option<String>,
}

impl SessionHandoffPackage {
    /// Returns `true` when all required fields are present and
    /// `open_escalations` is empty — i.e. the package is implementation-ready.
    pub fn is_implementation_ready(&self) -> bool {
        !self.objective.trim().is_empty()
            && self.open_escalations.is_empty()
            && !self.target_tickets.is_empty()
            && !self.target_files.is_empty()
            && !self.decisions.is_empty()
            && !self.non_goals.is_empty()
            && !self.context_anchors.is_empty()
    }

    /// Returns the names of required fields that are absent or empty.
    pub fn missing_fields(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.objective.trim().is_empty() {
            missing.push("objective");
        }
        if self.target_tickets.is_empty() {
            missing.push("target_tickets");
        }
        if self.target_files.is_empty() {
            missing.push("target_files");
        }
        if self.decisions.is_empty() {
            missing.push("decisions");
        }
        if self.non_goals.is_empty() {
            missing.push("non_goals");
        }
        if self.context_anchors.is_empty() {
            missing.push("context_anchors");
        }
        missing
    }

    /// Returns required upward-context fields that are absent or empty.
    pub fn missing_upward_context_fields(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.higher_level_objective.trim().is_empty() {
            missing.push("higher_level_objective");
        }
        if self.upward_context.is_empty() {
            missing.push("upward_context");
        }
        missing
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHandoffRecord {
    pub handoff_id: String,
    #[serde(alias = "workspace_session_id")]
    pub session_id: String,
    pub outgoing_run_id: String,
    pub created_at: DateTime<Utc>,
    pub resume_command: String,
    /// The session that picked this handoff up. `None` means the handoff is
    /// unclaimed (target-less) and appears in the backlog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_entities: Vec<SessionPinnedEntityHeader>,
    pub workflow: SessionWorkflowSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validation: Vec<SessionValidationGate>,
    // ── Handoff-package schema fields ────────────────────────────────────────
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub objective: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_tickets: Vec<SessionHandoffTargetTicket>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub higher_level_objective: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upward_context: Vec<SessionHandoffUpwardContextEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_goals: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_anchors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_escalations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_handoff: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHandoffResult {
    pub record: SessionHandoffRecord,
    pub record_path: String,
    pub render: String,
}

/// Filter for querying the unclaimed-handoff backlog. Both fields are
/// optional narrowing predicates; leave a field `None` to skip it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffBacklogFilter {
    /// Only include handoffs whose source session belongs to this track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    /// Only include handoffs emitted by this source session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFinishRecord {
    #[serde(alias = "workspace_session_id")]
    pub session_id: String,
    pub run_id: String,
    pub finished_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_optional_node_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validation: Vec<SessionValidationGate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFinishResult {
    pub record: SessionFinishRecord,
    pub already_finished: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn handoff_record_deserializes_legacy_workspace_session_id_alias() {
        let record: SessionHandoffRecord = serde_json::from_value(json!({
            "handoff_id": "handoff-1",
            "workspace_session_id": "11111111-1111-4111-8111-111111111111",
            "outgoing_run_id": "run-1",
            "created_at": "2026-08-12T00:00:00Z",
            "resume_command": "session resume",
            "pinned_entities": [],
            "workflow": {
                "workflow": { "nodes": [], "edges": [] }
            }
        }))
        .unwrap();

        assert_eq!(record.session_id, "11111111-1111-4111-8111-111111111111");
    }
}
