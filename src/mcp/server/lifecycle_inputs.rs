#[allow(unused_imports)]
use super::*;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeInitInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    /// Copilot session UUID. Required; the MCP tool does not resolve one implicitly.
    pub session_id: String,
    #[serde(default)]
    pub predecessor_run_id: Option<String>,
    #[serde(default)]
    pub force_new_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeResumeInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    pub session_id: String,
    pub predecessor_run_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimePinInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    pub session_id: String,
    /// Canonical entity URN in `ce://<workspace>/<store>/<entity>` format (e.g. `ce://default/ticket/<ticket-id>`, `ce://default/spec/<spec-id>`, `ce://default/rule/<rule-id>`, or `ce://default/dossier/<folder-or-slug>`).
    pub entity_urn: String,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeUnpinInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    pub session_id: String,
    /// Canonical entity URN in `ce://<workspace>/<store>/<entity>` format to unpin.
    pub entity_urn: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeViewInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeHandoffInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    pub session_id: String,
    #[serde(default)]
    pub validation: Vec<ValidationGateInput>,
    /// The single goal of the next implementation unit (required for an
    /// implementation-ready package).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub objective: String,
    /// Tickets expected to be worked in the next session. Accepts legacy ticket
    /// id strings or objects with `id`, author-supplied `why`, and optional
    /// cached `state` and `acceptance_criteria`.
    #[serde(default)]
    pub target_tickets: Vec<HandoffTargetTicketInput>,
    /// Why the next implementation unit matters to the broader program.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub higher_level_objective: String,
    /// Ordered ancestor chain for the next implementation unit. Every entry
    /// supplies `entity_urn`, `title`, and `role`; legal roles are `epic`,
    /// `phase`, and `parent`.
    #[serde(default)]
    pub upward_context: Vec<HandoffUpwardContextInput>,
    /// Workspace-relative file paths expected to be touched.
    #[serde(default)]
    pub target_files: Vec<String>,
    /// Resolved design choices.
    #[serde(default)]
    pub decisions: Vec<String>,
    /// Explicit out-of-scope boundaries.
    #[serde(default)]
    pub non_goals: Vec<String>,
    /// Prior findings and ids needed so no search is required.
    #[serde(default)]
    pub context_anchors: Vec<String>,
    /// Must be empty for the package to be implementation-ready.
    #[serde(default)]
    pub open_escalations: Vec<String>,
    /// Known risks or fragile areas (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_notes: Option<String>,
    /// Id of the handoff this one supersedes (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_handoff: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum HandoffTargetTicketInput {
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

impl From<HandoffTargetTicketInput> for SessionHandoffTargetTicket {
    fn from(value: HandoffTargetTicketInput) -> Self {
        match value {
            HandoffTargetTicketInput::Legacy(id) => Self {
                id,
                why: String::new(),
                state: String::new(),
                acceptance_criteria: Vec::new(),
            },
            HandoffTargetTicketInput::Structured {
                id,
                why,
                state,
                acceptance_criteria,
            } => Self {
                id,
                why,
                state,
                acceptance_criteria,
            },
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HandoffUpwardContextInput {
    pub entity_urn: String,
    pub title: String,
    /// Legal values are `epic`, `phase`, and `parent`.
    pub role: HandoffUpwardContextRoleInput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HandoffUpwardContextRoleInput {
    Epic,
    Phase,
    Parent,
}

impl From<HandoffUpwardContextInput> for SessionHandoffUpwardContextEntry {
    fn from(value: HandoffUpwardContextInput) -> Self {
        Self {
            entity_urn: value.entity_urn,
            title: value.title,
            role: match value.role {
                HandoffUpwardContextRoleInput::Epic => SessionHandoffUpwardContextRole::Epic,
                HandoffUpwardContextRoleInput::Phase => SessionHandoffUpwardContextRole::Phase,
                HandoffUpwardContextRoleInput::Parent => SessionHandoffUpwardContextRole::Parent,
            },
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuntimeFinishInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    pub session_id: String,
    #[serde(default)]
    pub validation: Vec<ValidationGateInput>,
    #[serde(default)]
    pub deferred_optional_node_ids: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidationGateInput {
    pub validation_spec_id: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub outcome: Option<String>,
    /// The command that performs the validation check. Optional; when absent,
    /// `validation_spec_id` should reference a test-api ValidationSpec entry.
    #[serde(default)]
    pub command: Option<String>,
}

impl From<ValidationGateInput> for SessionValidationGate {
    fn from(value: ValidationGateInput) -> Self {
        Self {
            validation_spec_id: value.validation_spec_id,
            required: value.required,
            outcome: value.outcome,
            command: value.command,
        }
    }
}
