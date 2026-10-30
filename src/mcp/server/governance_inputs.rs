#[allow(unused_imports)]
use super::*;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrantCreateInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    /// Grant scope: session or subagent.
    pub scope: String,
    /// Budget offset to add.
    pub offset: u32,
    /// Optional model constraint (case-insensitive).
    #[serde(default)]
    pub model: Option<String>,
    /// Optional TTL in seconds from now.
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrantListInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrantRevokeInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    /// Grant ID to revoke.
    pub grant_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EscalationCreateInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    /// The blocking decision or problem statement.
    pub blocking_decision: String,
    /// Context explaining the situation.
    pub context: String,
    /// Optional requested capability or resource.
    #[serde(default)]
    pub requested_capability: Option<String>,
    /// Options considered before escalating.
    #[serde(default)]
    pub options_considered: Vec<String>,
    /// Optional session ID that created the escalation.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Optional model that created the escalation.
    #[serde(default)]
    pub from_model: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EscalationListInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    /// Optional status filter: open or resolved.
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EscalationGetInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    /// Escalation ID to retrieve.
    pub escalation_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EscalationResolveInput {
    /// Concrete workspace path, repo root, .session store path, or path inside that store. Do not use omitted, empty, 'default', or '..' for entity creation; use '.' explicitly to target the MCP server process's current working directory.
    pub workspace: String,
    /// Escalation ID to resolve.
    pub escalation_id: String,
    /// Resolution action: handled, granted-offset, escalated-to-user, spawned-session.
    pub action: String,
    /// Optional note about the resolution.
    #[serde(default)]
    pub note: Option<String>,
    /// Grant ID (required when action is granted-offset).
    #[serde(default)]
    pub grant_id: Option<String>,
    /// Spawned session ID (required when action is spawned-session).
    #[serde(default)]
    pub spawned_session_id: Option<String>,
}
