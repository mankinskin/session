//! Pure MCP message interception logic for the cost-gate middleware.
//!
//! These functions operate on parsed JSON-RPC messages so they can be unit
//! tested without spawning processes. The wiring in `main.rs` reads/writes
//! newline-delimited JSON on stdio and calls into here.

use std::{
    collections::{
        HashMap,
        HashSet,
    },
    path::{
        Path,
        PathBuf,
    },
};

use serde::{
    Deserialize,
    Serialize,
};
use serde_json::{
    Value,
    json,
};
use session_api::{
    SessionError,
    store::SessionStoreConfig,
};
use session_workspace_resolver::{
    ResolutionError,
    ResolveRequest,
    ResolverConfig,
    SessionWorkspaceResolver,
};

use toolmon_policy_api::{
    CALLER_MODEL_ARG,
    Decision,
    Policy,
    SESSION_ID_ARG,
    inject_caller_model_schema,
};

/// Optional grant id argument for budget offset.
pub const GRANT_ID_ARG: &str = "grant_id";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathArgumentKind {
    Workspace,
    Path,
}

#[derive(Clone, Copy, Debug)]
struct PathArgument {
    name: &'static str,
    kind: PathArgumentKind,
}

const PATH_ARGUMENT_REGISTRY: &[(&str, PathArgument)] = &[
    (
        "fs_list_dir",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_stat",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_move_file",
        PathArgument {
            name: "from",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_move_file",
        PathArgument {
            name: "to",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_move_file",
        PathArgument {
            name: "root",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_rename_file",
        PathArgument {
            name: "from",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_rename_file",
        PathArgument {
            name: "root",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_copy_file",
        PathArgument {
            name: "from",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_copy_file",
        PathArgument {
            name: "to",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_copy_file",
        PathArgument {
            name: "root",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_delete_file",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_delete_file",
        PathArgument {
            name: "root",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_delete_dir",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_delete_dir",
        PathArgument {
            name: "root",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "peek_read",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "peek_grep",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "peek_count",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "peek_skeleton",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
];

fn registered_path_argument(
    tool: &str,
    name: &str,
) -> Option<PathArgument> {
    if name == "workspace" {
        return Some(PathArgument {
            name: "workspace",
            kind: PathArgumentKind::Workspace,
        });
    }
    PATH_ARGUMENT_REGISTRY
        .iter()
        .find(|(registered_tool, argument)| {
            *registered_tool == tool && argument.name == name
        })
        .map(|(_, argument)| *argument)
}

/// What the proxy should do with a client→server message.
#[derive(Debug)]
pub enum ClientAction {
    /// Forward this (possibly rewritten) message to the real server.
    Forward(Value),
    /// Do not forward; send this response straight back to the client.
    Respond(Value),
}

/// Track which JSON-RPC ids were `tools/list` requests, so their responses can
/// be schema-augmented on the way back.
#[derive(Default)]
pub struct PendingList {
    ids: HashSet<String>,
    path_arguments: HashMap<String, Vec<PathArgument>>,
}

impl PendingList {
    pub fn record(
        &mut self,
        id: &Value,
    ) {
        self.ids.insert(id_key(id));
    }

    pub fn take(
        &mut self,
        id: &Value,
    ) -> bool {
        self.ids.remove(&id_key(id))
    }

    fn record_path_arguments(
        &mut self,
        tool: &Value,
    ) {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            return;
        };
        let Some(properties) = tool
            .get("inputSchema")
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object)
        else {
            self.path_arguments.remove(name);
            return;
        };
        let arguments = properties
            .keys()
            .filter_map(|argument| registered_path_argument(name, argument))
            .collect::<Vec<_>>();
        if arguments.is_empty() {
            self.path_arguments.remove(name);
        } else {
            self.path_arguments.insert(name.to_string(), arguments);
        }
    }

    fn path_arguments(
        &self,
        tool: &str,
    ) -> Vec<PathArgument> {
        self.path_arguments.get(tool).cloned().unwrap_or_default()
    }
}

fn id_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_default()
}

/// Payload telemetry for an MCP tool call (ticket 9d527ad1).
///
/// `tokens_estimated` is a rough chars/4 estimate over the combined
/// request+response payloads — never an observed token count, and never a
/// dollar cost (tools have no dollar cost; see spec 7be68a48 R4).
///
/// Coverage is intentionally partial: this proxy only measures MCP
/// `tools/call` traffic that traverses this middleware.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallTelemetry {
    pub timestamp: String,
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_chars: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_chars: Option<u64>,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_estimated: Option<u64>,
}

/// A `tools/call` forwarded to the real server, awaiting its response.
///
/// Captured at the moment of forwarding so `handle_server_message` can
/// compute `duration_ms` and emit a `CallTelemetry` once the matching
/// response arrives (correlated by JSON-RPC id).
#[derive(Debug, Clone)]
pub struct PendingCall {
    pub tool_name: String,
    pub caller_model: Option<String>,
    pub grant_id: Option<String>,
    pub decision: String,
    pub request_bytes: u64,
    pub request_chars: u64,
    pub started_at: std::time::Instant,
    /// Soft warning to surface on the eventual server response when the
    /// `caller_model` only resolved after fallback normalization.
    pub warning: Option<String>,
}

/// Tracks in-flight forwarded `tools/call` requests by JSON-RPC id.
#[derive(Default)]
pub struct PendingCalls {
    calls: std::collections::HashMap<String, PendingCall>,
}

impl PendingCalls {
    pub fn record(
        &mut self,
        id: &Value,
        call: PendingCall,
    ) {
        self.calls.insert(id_key(id), call);
    }

    pub fn take(
        &mut self,
        id: &Value,
    ) -> Option<PendingCall> {
        self.calls.remove(&id_key(id))
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Fallback normalization for `caller_model` strings, applied only when the
/// raw value fails the gate's exact/substring resolution. Strips a trailing
/// parenthetical client qualifier (e.g. `"Claude Sonnet 5 (copilot)"` ->
/// `"Claude Sonnet 5"`), then folds spaces and underscores to hyphens, then
/// lowercases. No fuzzy or edit-distance matching.
pub fn normalize_caller_model(model: &str) -> String {
    let trimmed = model.trim();
    let stripped = if trimmed.ends_with(')') {
        trimmed
            .rfind('(')
            .map(|idx| trimmed[..idx].trim_end())
            .unwrap_or(trimmed)
    } else {
        trimmed
    };
    stripped
        .chars()
        .map(|c| if c == ' ' || c == '_' { '-' } else { c })
        .collect::<String>()
        .to_lowercase()
}

/// Compute payload size and estimated tokens from a JSON value.
pub fn compute_payload_telemetry(value: &Value) -> (u64, u64, u64) {
    let json_str = serde_json::to_string(value).unwrap_or_default();
    let bytes = json_str.as_bytes().len() as u64;
    let chars = json_str.chars().count() as u64;
    let tokens_estimated = chars / 4; // chars/4 divisor per ticket spec
    (bytes, chars, tokens_estimated)
}

/// Build a `tools/call` result carrying an error message (isError=true).
fn error_result(
    id: &Value,
    text: &str,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "isError": true
        }
    })
}

const MAIN_CHECKOUT_ENV: &str = "MCP_MAIN_CHECKOUT";
const DEFAULT_STORE_DIR: &str = ".session";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolAccess {
    Read,
    Mutation,
}

const KNOWN_READ_TOOLS: &[&str] = &[
    "fs_list_dir",
    "fs_stat",
    "get_part",
    "get_ticket",
    "get_ticket_description",
    "health",
    "health_check",
    "list_edges",
    "list_parts",
    "list_tickets",
    "list_workspaces",
    "next_tickets",
    "peek_count",
    "peek_grep",
    "peek_read",
    "peek_skeleton",
    "session_capabilities",
    "session_escalation_get",
    "session_escalation_list",
    "session_grant_list",
    "session_lookup",
    "session_peek_range",
    "session_peek_skeleton",
    "session_query",
    "session_runtime_render_instructions",
    "session_runtime_view",
    "session_sessions_for_ticket",
    "session_subagent_rollups",
    "session_terminal_peek",
    "session_terminal_status",
    "session_tool_metrics",
    "session_workflow_render_mermaid",
    "session_workflow_render_terminal",
    "spec_get",
    "spec_health",
    "spec_list",
    "spec_refs_validate",
    "spec_search",
    "spec_section_get",
    "spec_section_list",
    "spec_tree",
    "subgraph",
    "test_get_execution",
    "test_get_spec",
    "test_list_executions",
    "test_list_specs",
    "ticket_capabilities",
    "topgraph",
    "workflow",
];

/// Classifies MCP operations at the routing boundary. Unrecognized names are
/// mutations so newly added tools remain protected until explicitly reviewed.
fn tool_access(tool: &str) -> ToolAccess {
    if KNOWN_READ_TOOLS.contains(&tool) {
        ToolAccess::Read
    } else {
        ToolAccess::Mutation
    }
}

/// Builds the resolver anchored on the checkout the servers were launched in.
///
/// The anchor is inferred from the process working directory, which is the
/// checkout the MCP servers were started in. `MCP_MAIN_CHECKOUT` remains an
/// override for callers that cannot control that working directory; it is not
/// required for normal operation.
fn anchored_resolver() -> Result<SessionWorkspaceResolver, String> {
    let config = match std::env::var(MAIN_CHECKOUT_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(override_path) => ResolverConfig {
            main_checkout: PathBuf::from(override_path),
            workspace_slug: "default".to_string(),
        },
        None => ResolverConfig::from_working_dir("default")
            .map_err(|error| error.to_string())?,
    };
    SessionWorkspaceResolver::new(config).map_err(|error| error.to_string())
}

fn resolve_workspace(
    session_id: &str,
    workspace: Option<&str>,
    access: ToolAccess,
) -> Result<(String, PathBuf), String> {
    let store_dir = DEFAULT_STORE_DIR.to_string();
    let resolver = anchored_resolver()?;
    let absolute_workspace = workspace
        .filter(|value| Path::new(value).is_absolute())
        .map(PathBuf::from);
    let relative_workspace = workspace
        .filter(|value| !value.is_empty() && *value != "default")
        .filter(|value| !Path::new(value).is_absolute())
        .map(Path::new);
    let resolved = match resolver.resolve(ResolveRequest {
        session_id,
        relative_workspace,
        store_dir: &store_dir,
    }) {
        Ok(resolved) => resolved,
        Err(ResolutionError::MissingSessionWorktree { .. })
            if access == ToolAccess::Read
                && workspace.is_none_or(|value| {
                    value.is_empty() || value == "default"
                }) =>
        {
            let store_root = resolver
                .refused_candidates(&store_dir)
                .map_err(|error| error.to_string())?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    "read workspace resolution could not derive repository anchor"
                        .to_string()
                })?;
            let target_root = store_root.parent().ok_or_else(|| {
                format!(
                    "read workspace store root '{}' has no repository parent",
                    normalized_path(&store_root)
                )
            })?;
            return Ok((normalized_path(target_root), store_root));
        },
        Err(error) =>
            return Err(match error {
                ResolutionError::MissingSessionWorktree { .. }
                    if workspace.is_none_or(|value| {
                        value.is_empty() || value == "default"
                    }) =>
                {
                    let candidates = resolver
                        .refused_candidates(&store_dir)
                        .unwrap_or_default();
                    let looks_like_repository_root = candidates.len() == 1
                        && candidates[0].parent().is_some_and(|root| {
                            root.join(".worktrees").is_dir()
                        });
                    if looks_like_repository_root {
                        ResolutionError::MainCheckoutMutationBlocked.to_string()
                    } else {
                        ResolutionError::UnanchoredDefault {
                            session_id: session_id.to_string(),
                            candidates,
                        }
                        .to_string()
                    }
                },
                other => other.to_string(),
            }),
    };
    if access == ToolAccess::Mutation {
        resolved
            .require_mutation_target()
            .map_err(|error| error.to_string())?;
    }
    let store_root = resolved
        .store_root(&store_dir)
        .map_err(|error| error.to_string())?;
    let canonical_target_root = std::fs::canonicalize(resolved.target_root()).map_err(|error| {
        format!(
            "resolved session worktree '{}' could not be canonicalized: {error}",
            resolved.target_root().display()
        )
    })?;
    let target_root = absolute_workspace
        .map(|workspace| {
            let canonical_workspace = std::fs::canonicalize(&workspace).map_err(|error| {
                format!("workspace '{}' could not be canonicalized: {error}", workspace.display())
            })?;
            if !canonical_workspace.starts_with(&canonical_target_root) {
                return Err(format!(
                    "workspace '{}' (canonical '{}') is outside resolved session worktree '{}'",
                    workspace.display(),
                    canonical_workspace.display(),
                    canonical_target_root.display()
                ));
            }
            Ok(canonical_workspace)
        })
        .transpose()?
        .unwrap_or(canonical_target_root)
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .trim_end_matches('/')
        .to_string();
    Ok((target_root, store_root))
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .to_string()
}

fn session_is_unassigned(
    repository_root: &Path,
    session_id: &str,
) -> Result<bool, String> {
    let config = SessionStoreConfig::new(
        repository_root.join(DEFAULT_STORE_DIR),
        "default",
    );
    match config.read_session(session_id) {
        Ok(record) => Ok(record.metadata.worktree.is_none()),
        Err(SessionError::NotFound { .. }) => Ok(true),
        Err(error) => Err(error.to_string()),
    }
}

fn try_resolve_session_check_in_bootstrap_workspace(
    tool: &str,
    session_id: &str,
    workspace: Option<&str>,
) -> Result<Option<(String, PathBuf)>, String> {
    if tool != "session_check_in" {
        return Ok(None);
    }
    let Some(selector) = workspace else {
        return Ok(None);
    };
    if selector.is_empty() || selector == "default" {
        return Ok(None);
    }
    let workspace_path = PathBuf::from(selector);
    if !workspace_path.is_absolute() {
        return Ok(None);
    }

    let resolver = anchored_resolver()?;
    let canonical_workspace =
        std::fs::canonicalize(&workspace_path).map_err(|error| {
            format!(
                "workspace '{}' could not be canonicalized: {error}",
                workspace_path.display()
            )
        })?;
    let anchor_candidate = resolver
        .refused_candidates(DEFAULT_STORE_DIR)
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
        .ok_or_else(|| {
            "session_check_in bootstrap could not derive repository anchor"
                .to_string()
        })?;
    let repository = anchor_candidate
        .parent()
        .ok_or_else(|| {
            format!(
                "session_check_in bootstrap anchor '{}' has no repository parent",
                normalized_path(&anchor_candidate)
            )
        })?
        .to_path_buf();
    let canonical_repository = std::fs::canonicalize(&repository).map_err(|error| {
        format!(
            "session_check_in bootstrap repository '{}' could not be canonicalized: {error}",
            normalized_path(&repository)
        )
    })?;
    if !session_is_unassigned(&canonical_repository, session_id)? {
        return Ok(None);
    }
    let canonical_worktrees = canonical_repository.join(".worktrees");
    if canonical_workspace.parent() != Some(canonical_worktrees.as_path()) {
        return Err(format!(
            "session_check_in bootstrap workspace '{}' must be a direct child of '{}'; received '{}'.",
            workspace_path.display(),
            normalized_path(&canonical_worktrees),
            normalized_path(&canonical_workspace)
        ));
    }
    let git_entry = canonical_workspace.join(".git");
    if !git_entry.exists() {
        return Err(format!(
            "session_check_in bootstrap workspace '{}' is missing required '.git' entry",
            normalized_path(&canonical_workspace)
        ));
    }
    Ok(Some((
        normalized_path(&canonical_workspace),
        canonical_workspace.join(DEFAULT_STORE_DIR),
    )))
}

fn resolve_workspace_for_tool(
    tool: &str,
    session_id: &str,
    workspace: Option<&str>,
) -> Result<(String, PathBuf), String> {
    let access = tool_access(tool);
    match resolve_workspace(session_id, workspace, access) {
        Ok(resolved) => Ok(resolved),
        Err(error) => {
            match try_resolve_session_check_in_bootstrap_workspace(
                tool, session_id, workspace,
            ) {
                Ok(Some(resolved)) => Ok(resolved),
                Ok(None) => Err(error),
                Err(bootstrap_error) => Err(bootstrap_error),
            }
        },
    }
}

/// Handle a client→server message.
///
/// * `tools/list` requests are recorded and forwarded.
/// * `tools/call` requests are gated: a missing `caller_model` is rejected; a
///   delegate decision is refused with guidance; an allow strips `caller_model`
///   and forwards the cleaned call.
/// * `caller_model` is resolved as-is first (exact match, then substring,
///   unchanged precedence). Only if that fails is a normalized candidate
///   tried as a fallback (see [`normalize_caller_model`]); a match there
///   still allows the call but attaches a `costGateWarning` to the eventual
///   response instead of rejecting.
/// * Everything else is forwarded unchanged.
///
/// When `gate` is `None` (fail-open, e.g. price table missing) the message is
/// forwarded unchanged.
pub fn handle_client_message(
    mut msg: Value,
    policy: Option<&dyn Policy>,
    pending: &mut PendingList,
    pending_calls: &mut PendingCalls,
) -> (ClientAction, Option<CallTelemetry>) {
    let Some(policy) = policy else {
        return (ClientAction::Forward(msg), None);
    };

    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "tools/list" => {
            if let Some(id) = msg.get("id") {
                pending.record(id);
            }
            (ClientAction::Forward(msg), None)
        },
        "tools/call" => {
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let tool = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let caller_model = params
                .get("arguments")
                .and_then(|a| a.get(CALLER_MODEL_ARG))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let session_id = params
                .get("arguments")
                .and_then(|a| a.get(SESSION_ID_ARG))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let grant_id = params
                .get("arguments")
                .and_then(|a| a.get(GRANT_ID_ARG))
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string());
            let (request_bytes, request_chars, _) =
                compute_payload_telemetry(&msg);

            // Build an immediate (non-forwarded) telemetry record: nothing was
            // sent to the server, so response counts are zero and duration_ms
            // is zero (no wall-clock span to measure).
            let immediate_telemetry =
                |decision: &str, caller_model: Option<String>| CallTelemetry {
                    timestamp: now_rfc3339(),
                    tool_name: tool.clone(),
                    caller_model,
                    grant_id: grant_id.clone(),
                    decision: decision.to_string(),
                    request_bytes: Some(request_bytes),
                    request_chars: Some(request_chars),
                    response_bytes: Some(0),
                    response_chars: Some(0),
                    duration_ms: 0,
                    tokens_estimated: Some(request_chars / 4),
                };

            if caller_model.is_empty() {
                let telemetry =
                    immediate_telemetry("reject-missing-model", None);
                return (
                    ClientAction::Respond(error_result(
                        &id,
                        &format!(
                            "Missing required '{CALLER_MODEL_ARG}' argument. Every tool \
                             call must declare the id of the model issuing it (e.g. \
                             claude-opus-4-8) so price-awareness enforcement can run."
                        ),
                    )),
                    Some(telemetry),
                );
            }

            if session_id.is_empty() {
                let telemetry = immediate_telemetry(
                    "reject-missing-session",
                    Some(caller_model),
                );
                return (
                    ClientAction::Respond(error_result(
                        &id,
                        &format!(
                            "Missing required '{SESSION_ID_ARG}' argument. Every tool \
                             call must declare the session it belongs to so the session \
                             anchors the call to that session's worktree instead of silently \
                             resolving to the server process working directory."
                        ),
                    )),
                    Some(telemetry),
                );
            }

            // Resolve the raw caller_model first (exact -> substring, unchanged
            // precedence). Only when that fails do we retry with a normalized
            // candidate (trailing client qualifier stripped; separators
            // folded to hyphens) as a fallback, never in place of it.
            let mut effective_model = caller_model.clone();
            let mut soft_warning: Option<String> = None;
            if !policy.resolves(&caller_model) {
                let normalized = normalize_caller_model(&caller_model);
                if normalized != caller_model && policy.resolves(&normalized) {
                    soft_warning = Some(format!(
                        "caller_model '{caller_model}' did not match the price table \
                         exactly; normalized to '{normalized}' (stripped trailing client \
                         qualifier and/or folded separators to hyphens) and resolved from \
                         there. Pass the exact price-table model_id to avoid this warning."
                    ));
                    effective_model = normalized;
                }
            }

            match policy.evaluate(&effective_model, &tool, grant_id.as_deref())
            {
                Decision::Reject { guidance } => {
                    let telemetry =
                        immediate_telemetry("reject", Some(caller_model));
                    (
                        ClientAction::Respond(error_result(&id, &guidance)),
                        Some(telemetry),
                    )
                },
                Decision::Delegate { guidance } => {
                    let telemetry =
                        immediate_telemetry("delegate", Some(caller_model));
                    (
                        ClientAction::Respond(error_result(&id, &guidance)),
                        Some(telemetry),
                    )
                },
                Decision::Allow => {
                    let path_arguments = pending.path_arguments(&tool);
                    let workspace = path_arguments
                        .iter()
                        .find(|argument| {
                            argument.kind == PathArgumentKind::Workspace
                        })
                        .and_then(|argument| {
                            msg.get("params")
                                .and_then(|params| params.get("arguments"))
                                .and_then(|arguments| {
                                    arguments.get(argument.name)
                                })
                                .and_then(Value::as_str)
                        });
                    let (target_root, store_root) =
                        match resolve_workspace_for_tool(
                            &tool,
                            &session_id,
                            workspace,
                        ) {
                            Ok(resolved) => resolved,
                            Err(error) => {
                                let telemetry = immediate_telemetry(
                                    "reject-workspace",
                                    Some(caller_model),
                                );
                                return (
                                    ClientAction::Respond(error_result(
                                        &id, &error,
                                    )),
                                    Some(telemetry),
                                );
                            },
                        };
                    let mut rewrites = Vec::new();
                    for argument in &path_arguments {
                        let value = msg
                            .get("params")
                            .and_then(|params| params.get("arguments"))
                            .and_then(|arguments| arguments.get(argument.name))
                            .and_then(Value::as_str);
                        let Some(value) = value else {
                            if argument.kind == PathArgumentKind::Workspace {
                                rewrites
                                    .push((argument.name, target_root.clone()));
                            }
                            continue;
                        };
                        if Path::new(value).is_absolute() {
                            if let Err(error) = resolve_workspace_for_tool(
                                &tool,
                                &session_id,
                                Some(value),
                            ) {
                                let telemetry = immediate_telemetry(
                                    "reject-workspace",
                                    Some(caller_model),
                                );
                                return (
                                    ClientAction::Respond(error_result(
                                        &id, &error,
                                    )),
                                    Some(telemetry),
                                );
                            }
                            continue;
                        }
                        match resolve_workspace_for_tool(
                            &tool,
                            &session_id,
                            Some(value),
                        ) {
                            Ok((rewritten, _)) => {
                                rewrites.push((argument.name, rewritten));
                            },
                            Err(error) => {
                                let telemetry = immediate_telemetry(
                                    "reject-workspace",
                                    Some(caller_model),
                                );
                                return (
                                    ClientAction::Respond(error_result(
                                        &id, &error,
                                    )),
                                    Some(telemetry),
                                );
                            },
                        }
                    }
                    eprintln!(
                        "[mcp-toolmon] resolved store root: {}",
                        store_root.to_string_lossy().replace('\\', "/")
                    );
                    // Strip proxy-only arguments before forwarding to the real server.
                    if let Some(args) = msg
                        .get_mut("params")
                        .and_then(|p| p.get_mut("arguments"))
                        .and_then(Value::as_object_mut)
                    {
                        args.remove(CALLER_MODEL_ARG);
                        args.remove(GRANT_ID_ARG);
                        for (name, value) in rewrites {
                            args.insert(name.to_string(), Value::String(value));
                        }
                    }
                    let decision_label = if soft_warning.is_some() {
                        "allow-normalized"
                    } else {
                        "allow"
                    };
                    pending_calls.record(
                        &id,
                        PendingCall {
                            tool_name: tool,
                            caller_model: Some(caller_model),
                            grant_id,
                            decision: decision_label.to_string(),
                            request_bytes,
                            request_chars,
                            started_at: std::time::Instant::now(),
                            warning: soft_warning,
                        },
                    );
                    (ClientAction::Forward(msg), None)
                },
            }
        },
        _ => (ClientAction::Forward(msg), None),
    }
}

/// Handle a server→client message: if it is the response to a recorded
/// `tools/list` request, inject a required `caller_model` argument into every
/// advertised tool's `inputSchema`. Otherwise pass through unchanged.
pub fn handle_server_message(
    mut msg: Value,
    policy: Option<&dyn Policy>,
    pending: &mut PendingList,
    pending_calls: &mut PendingCalls,
) -> (Value, Option<CallTelemetry>) {
    let mut warning_to_inject: Option<String> = None;
    let telemetry =
        msg.get("id")
            .and_then(|id| pending_calls.take(id))
            .map(|call| {
                let (response_bytes, response_chars, _) =
                    compute_payload_telemetry(&msg);
                let duration_ms = call.started_at.elapsed().as_millis() as u64;
                let tokens_estimated =
                    (call.request_chars + response_chars) / 4;
                warning_to_inject = call.warning.clone();
                CallTelemetry {
                    timestamp: now_rfc3339(),
                    tool_name: call.tool_name,
                    caller_model: call.caller_model,
                    grant_id: call.grant_id,
                    decision: call.decision,
                    request_bytes: Some(call.request_bytes),
                    request_chars: Some(call.request_chars),
                    response_bytes: Some(response_bytes),
                    response_chars: Some(response_chars),
                    duration_ms,
                    tokens_estimated: Some(tokens_estimated),
                }
            });

    if let Some(warning) = warning_to_inject {
        if let Some(result) =
            msg.get_mut("result").and_then(Value::as_object_mut)
        {
            result.insert("costGateWarning".to_string(), json!(warning));
        }
    }

    let is_list_response =
        msg.get("id").map(|id| pending.take(id)).unwrap_or(false)
            && msg
                .get("result")
                .and_then(|r| r.get("tools"))
                .map(Value::is_array)
                .unwrap_or(false);

    if !is_list_response {
        return (msg, telemetry);
    }

    if let Some(tools) = msg
        .get_mut("result")
        .and_then(|r| r.get_mut("tools"))
        .and_then(Value::as_array_mut)
    {
        for tool in tools.iter_mut() {
            pending.record_path_arguments(tool);
            match policy {
                Some(p) => p.on_tools_list(tool),
                None => inject_caller_model_schema(tool),
            }
        }
    }
    (msg, telemetry)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SESSION_ID: &str = "66666666-6666-4666-8666-666666666666";
    use session_api::{
        SessionStoreConfig,
        SessionWorktreeCheckInRequest,
        SessionWorktreeStatus,
    };
    use session_workspace_resolver::{
        ResolverConfig,
        SessionWorkspaceResolver,
    };
    use std::{
        path::{
            Path,
            PathBuf,
        },
        process::Command,
        sync::Mutex,
    };
    use tempfile::TempDir;
    use toolmon_costgate::{
        CostGatePolicy,
        Gate,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn test_gate() -> CostGatePolicy {
        // Write a tiny fixture table to a unique temp file and load it. A
        // per-call counter avoids collisions between parallel tests (same pid).
        use std::sync::atomic::{
            AtomicU64,
            Ordering,
        };
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "mcpcg-fixture-{}-{}.json",
            std::process::id(),
            n
        ));
        std::fs::write(
            &path,
            r#"{"models":[
                {"provider_id":"anthropic","model_id":"claude-opus-4-1","output_mtok":75.0},
                {"provider_id":"openai","model_id":"gpt-5-mini","output_mtok":2.0}
            ]}"#,
        )
        .unwrap();
        let g = Gate::load(
            Path::new(&path),
            toolmon_costgate::ModelBudgetCalibration::default(),
            None,
            None,
        )
        .unwrap();
        let _ = std::fs::remove_file(&path);
        CostGatePolicy::new(g)
    }

    fn call(
        tool: &str,
        model: Option<&str>,
    ) -> Value {
        let mut args = serde_json::Map::new();
        if let Some(m) = model {
            args.insert(CALLER_MODEL_ARG.into(), json!(m));
        }
        args.insert(SESSION_ID_ARG.into(), json!(TEST_SESSION_ID));
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": args }
        })
    }

    fn routing_fixture(
        status: SessionWorktreeStatus,
        use_main_checkout: bool,
    ) -> (TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let main_checkout = temp.path().join("repository");
        let worktree = main_checkout
            .join(".worktrees")
            .join(TEST_SESSION_ID)
            .join("feature");
        assert!(
            Command::new("git")
                .current_dir(temp.path())
                .args(["init", "--quiet", &main_checkout.to_string_lossy()])
                .status()
                .unwrap()
                .success()
        );
        for args in [
            ["config", "user.email", "tests@example.invalid"],
            ["config", "user.name", "mcp-toolmon tests"],
        ] {
            assert!(
                Command::new("git")
                    .current_dir(&main_checkout)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(main_checkout.join("README.md"), "fixture\n").unwrap();
        assert!(
            Command::new("git")
                .current_dir(&main_checkout)
                .args(["add", "README.md"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(&main_checkout)
                .args(["commit", "--quiet", "-m", "fixture"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(&main_checkout)
                .args(["worktree", "add", "--quiet", "-b", "agent/test"])
                .arg(&worktree)
                .arg("HEAD")
                .status()
                .unwrap()
                .success()
        );
        let _resolver = SessionWorkspaceResolver::new(ResolverConfig {
            main_checkout: main_checkout.clone(),
            workspace_slug: "default".to_string(),
        })
        .unwrap();
        // The anchor store is the worktree registry: assignments always live
        // beneath the checkout the servers were launched in.
        let store =
            SessionStoreConfig::new(main_checkout.join(".session"), "default");
        store
            .check_in_worktree(SessionWorktreeCheckInRequest {
                session_id: TEST_SESSION_ID.to_string(),
                owner_id: "agent".to_string(),
                ticket_id: "ticket".to_string(),
                worktree_path: worktree.clone(),
                branch: "agent/test".to_string(),
                predecessor_session_id: None,
            })
            .unwrap();
        if use_main_checkout {
            let registry_path = main_checkout
                .join(".session/local/worktrees")
                .join(format!("{TEST_SESSION_ID}.json"));
            let mut registry: Value =
                serde_json::from_slice(&std::fs::read(&registry_path).unwrap())
                    .unwrap();
            registry["assignment"]["path"] =
                json!(main_checkout.to_string_lossy());
            std::fs::write(
                registry_path,
                serde_json::to_vec_pretty(&registry).unwrap(),
            )
            .unwrap();
        }
        let path = main_checkout
            .join(format!(".session/sessions/{TEST_SESSION_ID}/session.json"));
        let mut record = store.read_session(TEST_SESSION_ID).unwrap();
        record.metadata.worktree.as_mut().unwrap().status = status;
        std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap())
            .unwrap();
        (temp, main_checkout, worktree)
    }

    fn main_checkout_fixture() -> (TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let main_checkout = temp.path().join("repository");
        std::fs::create_dir_all(main_checkout.join(".git")).unwrap();
        std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
        std::fs::create_dir_all(main_checkout.join(".worktrees")).unwrap();
        (temp, main_checkout)
    }

    struct TestRouting {
        _guard: std::sync::MutexGuard<'static, ()>,
        _temp: TempDir,
    }

    impl Drop for TestRouting {
        fn drop(&mut self) {
            unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
        }
    }

    fn active_routing() -> TestRouting {
        let guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (temp, main_checkout, _worktree) =
            routing_fixture(SessionWorktreeStatus::Active, false);
        unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, main_checkout) };
        TestRouting {
            _guard: guard,
            _temp: temp,
        }
    }

    fn allowed_call() -> Value {
        call("read_file", Some("gpt-5-mini"))
    }

    fn route(
        request: Value,
        gate: &CostGatePolicy,
    ) -> (ClientAction, Option<CallTelemetry>) {
        let mut pending = PendingList::default();
        let mut pending_calls = PendingCalls::default();
        handle_client_message(
            request,
            Some(gate),
            &mut pending,
            &mut pending_calls,
        )
    }

    fn route_with_schema(
        request: Value,
        gate: &CostGatePolicy,
        tool: &str,
        properties: Value,
    ) -> (ClientAction, Option<CallTelemetry>) {
        let mut pending = PendingList::default();
        let mut pending_calls = PendingCalls::default();
        let _ = handle_client_message(
            json!({"jsonrpc":"2.0","id":99,"method":"tools/list","params":{}}),
            Some(gate),
            &mut pending,
            &mut pending_calls,
        );
        let _ = handle_server_message(
            json!({
                "jsonrpc": "2.0",
                "id": 99,
                "result": { "tools": [{
                    "name": tool,
                    "inputSchema": { "type": "object", "properties": properties }
                }] }
            }),
            Some(gate),
            &mut pending,
            &mut pending_calls,
        );
        handle_client_message(
            request,
            Some(gate),
            &mut pending,
            &mut pending_calls,
        )
    }

    fn normalized(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn canonicalized_normalized(path: &Path) -> String {
        normalized(&std::fs::canonicalize(path).unwrap())
            .trim_start_matches("//?/")
            .to_string()
    }

    fn response_text(action: ClientAction) -> String {
        let ClientAction::Respond(value) = action else {
            panic!("expected routing rejection");
        };
        assert_eq!(value["result"]["isError"], true);
        value["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn session_resolving_to_worktree_rewrites_workspace_argument() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (_temp, main_checkout, worktree) =
            routing_fixture(SessionWorktreeStatus::Active, false);
        unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
        let (ClientAction::Forward(forwarded), _) = route_with_schema(
            allowed_call(),
            &test_gate(),
            "read_file",
            json!({"workspace": {"type": "string"}}),
        ) else {
            panic!("expected forwarded request");
        };
        assert_eq!(
            forwarded["params"]["arguments"]["workspace"],
            json!(normalized(&worktree))
        );
        assert_ne!(
            forwarded["params"]["arguments"]["workspace"],
            json!(normalized(&main_checkout))
        );
        unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
    }

    #[test]
    fn reads_from_main_checkout_are_forwarded() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (_temp, main_checkout) = main_checkout_fixture();
        unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
        for tool in [
            "list_workspaces",
            "get_ticket",
            "session_lookup",
            "spec_get",
            "test_list_specs",
        ] {
            let (ClientAction::Forward(forwarded), _) = route_with_schema(
                call(tool, Some("gpt-5-mini")),
                &test_gate(),
                tool,
                json!({"workspace": {"type": "string"}}),
            ) else {
                panic!("{tool} should forward from the main checkout");
            };
            assert_eq!(
                forwarded["params"]["arguments"]["workspace"],
                json!(normalized(&main_checkout))
            );
        }
        unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
    }

    #[test]
    fn mutations_and_unknown_tools_remain_blocked_from_main_checkout() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (_temp, main_checkout) = main_checkout_fixture();
        unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
        for tool in ["update_ticket", "unknown_tool", "get_unknown"] {
            let text = response_text(
                route_with_schema(
                    call(tool, Some("gpt-5-mini")),
                    &test_gate(),
                    tool,
                    json!({"workspace": {"type": "string"}}),
                )
                .0,
            );
            assert!(text.contains("main checkout mutations are blocked"));
        }
        unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
    }

    #[test]
    fn unassigned_session_check_in_with_direct_worktree_workspace_is_forwarded()
    {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let main_checkout = temp.path().join("repository");
        let worktree = main_checkout.join(".worktrees").join("bootstrap");
        std::fs::create_dir_all(main_checkout.join(".git")).unwrap();
        std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            "gitdir: ../../.git/worktrees/bootstrap\n",
        )
        .unwrap();
        unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

        let mut request = call("session_check_in", Some("gpt-5-mini"));
        request["params"]["arguments"]["workspace"] =
            json!(normalized(&worktree));
        let (ClientAction::Forward(forwarded), _) = route_with_schema(
            request,
            &test_gate(),
            "session_check_in",
            json!({"workspace": {"type": "string"}}),
        ) else {
            panic!("expected forwarded request");
        };
        assert_eq!(
            forwarded["params"]["arguments"]["workspace"],
            json!(normalized(&worktree))
        );
        unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
    }

    #[test]
    fn tool_access_allows_registered_reads_and_guards_writes() {
        for tool in KNOWN_READ_TOOLS {
            assert_eq!(tool_access(tool), ToolAccess::Read, "{tool}");
        }

        for tool in [
            "fs_copy_file",
            "fs_delete_dir",
            "fs_delete_file",
            "fs_move_file",
            "fs_rename_file",
            "session_check_in",
            "update_ticket",
            "unknown_tool",
        ] {
            assert_eq!(tool_access(tool), ToolAccess::Mutation, "{tool}");
        }
    }

    #[test]
    fn unassigned_session_check_in_rejects_non_direct_worktree_target() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let main_checkout = temp.path().join("repository");
        let worktree = main_checkout.join(".worktrees").join("bootstrap");
        let nested = worktree.join("nested");
        std::fs::create_dir_all(main_checkout.join(".git")).unwrap();
        std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            worktree.join(".git"),
            "gitdir: ../../.git/worktrees/bootstrap\n",
        )
        .unwrap();
        std::fs::write(nested.join(".git"), "gitdir: ../../.git/fake\n")
            .unwrap();
        unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

        let mut request = call("session_check_in", Some("gpt-5-mini"));
        request["params"]["arguments"]["workspace"] =
            json!(nested.to_string_lossy());
        let text = response_text(
            route_with_schema(
                request,
                &test_gate(),
                "session_check_in",
                json!({"workspace": {"type": "string"}}),
            )
            .0,
        );
        assert!(text.contains("must be a direct child"));
        assert!(text.contains(&normalized(&main_checkout.join(".worktrees"))));
        unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
    }

    #[test]
    fn anchor_falls_back_to_the_process_working_directory() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
        let resolver = anchored_resolver()
            .expect("working directory should anchor the resolver");
        let candidates = resolver
            .refused_candidates(DEFAULT_STORE_DIR)
            .expect("candidates should enumerate");
        let working_dir =
            canonicalized_normalized(&std::env::current_dir().unwrap());
        assert!(
            candidates.iter().any(
                |candidate| normalized(candidate).starts_with(&working_dir)
            ),
            "expected a candidate anchored on {working_dir}, got {candidates:?}"
        );
    }

    #[test]
    fn positional_worktree_discovery_overrides_legacy_main_checkout_assignment()
    {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (_temp, main_checkout, _worktree) =
            routing_fixture(SessionWorktreeStatus::Active, true);
        unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", main_checkout) };
        let (ClientAction::Forward(_), _) = route(allowed_call(), &test_gate())
        else {
            panic!("positional UUID worktree should be forwarded");
        };
        unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
    }

    #[test]
    fn positional_worktree_discovery_ignores_legacy_assignment_status() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (_temp, main_checkout, _worktree) =
            routing_fixture(SessionWorktreeStatus::Superseded, false);
        unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", main_checkout) };
        let (ClientAction::Forward(_), _) = route(allowed_call(), &test_gate())
        else {
            panic!("positional UUID worktree should be forwarded");
        };
        unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
    }

    #[test]
    fn absolute_workspace_is_contained_by_session_worktree() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (_temp, main_checkout, worktree) =
            routing_fixture(SessionWorktreeStatus::Active, false);
        let inside = worktree.join("nested");
        std::fs::create_dir_all(&inside).unwrap();
        unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };

        let mut request = allowed_call();
        request["params"]["arguments"]["workspace"] =
            json!(inside.to_string_lossy());
        let (ClientAction::Forward(forwarded), _) = route_with_schema(
            request,
            &test_gate(),
            "read_file",
            json!({"workspace": {"type": "string"}}),
        ) else {
            panic!("expected forwarded request");
        };
        assert_eq!(
            forwarded["params"]["arguments"]["workspace"],
            json!(inside)
        );

        let outside = main_checkout.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let mut request = allowed_call();
        request["params"]["arguments"]["workspace"] =
            json!(outside.to_string_lossy());
        let text = response_text(
            route_with_schema(
                request,
                &test_gate(),
                "read_file",
                json!({"workspace": {"type": "string"}}),
            )
            .0,
        );
        assert!(text.contains(outside.to_string_lossy().as_ref()));
        assert!(text.contains("resolved session worktree"));

        let mut request = allowed_call();
        request["params"]["arguments"]["workspace"] =
            json!(main_checkout.to_string_lossy());
        let text = response_text(
            route_with_schema(
                request,
                &test_gate(),
                "read_file",
                json!({"workspace": {"type": "string"}}),
            )
            .0,
        );
        assert!(text.contains(main_checkout.to_string_lossy().as_ref()));
        assert!(text.contains("resolved session worktree"));

        let mut request = allowed_call();
        request["params"]["arguments"]["workspace"] = json!("nested");
        let (ClientAction::Forward(forwarded), _) = route_with_schema(
            request,
            &test_gate(),
            "read_file",
            json!({"workspace": {"type": "string"}}),
        ) else {
            panic!("expected forwarded request");
        };
        assert_eq!(
            forwarded["params"]["arguments"]["workspace"],
            json!(normalized(&inside))
        );
        unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
    }

    #[test]
    fn resolved_store_root_is_logged() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (_temp, main_checkout, worktree) =
            routing_fixture(SessionWorktreeStatus::Active, false);
        unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", main_checkout) };
        let (ClientAction::Forward(forwarded), _) = route_with_schema(
            allowed_call(),
            &test_gate(),
            "read_file",
            json!({"workspace": {"type": "string"}}),
        ) else {
            panic!("expected forwarded request");
        };
        assert_eq!(
            forwarded["params"]["arguments"]["workspace"],
            json!(normalized(&worktree))
        );
        unsafe { std::env::remove_var("MCP_MAIN_CHECKOUT") };
    }

    #[test]
    fn registered_paths_rewrite_only_declared_schema_arguments() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (_temp, main_checkout, worktree) =
            routing_fixture(SessionWorktreeStatus::Active, false);
        let nested = worktree.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        unsafe { std::env::set_var(MAIN_CHECKOUT_ENV, &main_checkout) };

        let mut request = call("peek_read", Some("gpt-5-mini"));
        request["params"]["arguments"]["path"] = json!("nested");
        request["params"]["arguments"]["untouched"] = json!("value");
        let (ClientAction::Forward(forwarded), _) = route_with_schema(
            request,
            &test_gate(),
            "peek_read",
            json!({
                "path": {"type": "string"},
                "untouched": {"type": "string"}
            }),
        ) else {
            panic!("expected forwarded request");
        };
        assert_eq!(
            forwarded["params"]["arguments"]["path"],
            json!(normalized(&nested))
        );
        assert_eq!(
            forwarded["params"]["arguments"]["untouched"],
            json!("value")
        );
        assert!(forwarded["params"]["arguments"].get("workspace").is_none());

        let mut request = call("peek_read", Some("gpt-5-mini"));
        request["params"]["arguments"]["path"] = json!(nested);
        let (ClientAction::Forward(forwarded), _) = route_with_schema(
            request,
            &test_gate(),
            "peek_read",
            json!({"path": {"type": "string"}}),
        ) else {
            panic!("expected forwarded request");
        };
        assert_eq!(forwarded["params"]["arguments"]["path"], json!(nested));

        let outside = main_checkout.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let mut request = call("peek_read", Some("gpt-5-mini"));
        request["params"]["arguments"]["path"] = json!(outside);
        let text = response_text(
            route_with_schema(
                request,
                &test_gate(),
                "peek_read",
                json!({"path": {"type": "string"}}),
            )
            .0,
        );
        assert!(text.contains("outside"));

        let mut request = call("unknown_tool", Some("gpt-5-mini"));
        request["params"]["arguments"]["workspace"] = json!("unchanged");
        let (ClientAction::Forward(forwarded), _) = route_with_schema(
            request,
            &test_gate(),
            "unknown_tool",
            json!({"untouched": {"type": "string"}}),
        ) else {
            panic!("expected forwarded request");
        };
        assert_eq!(
            forwarded["params"]["arguments"]["workspace"],
            json!("unchanged")
        );
        unsafe { std::env::remove_var(MAIN_CHECKOUT_ENV) };
    }

    #[test]
    fn missing_caller_model_is_rejected() {
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        match handle_client_message(
            call("read_file", None),
            Some(&g),
            &mut p,
            &mut pc,
        ) {
            (ClientAction::Respond(v), telemetry) => {
                assert_eq!(v["result"]["isError"], json!(true));
                let text = v["result"]["content"][0]["text"].as_str().unwrap();
                assert!(text.contains(CALLER_MODEL_ARG));
                let telemetry =
                    telemetry.expect("expected telemetry for refused call");
                assert_eq!(telemetry.decision, "reject-missing-model");
                assert_eq!(telemetry.duration_ms, 0);
                assert_eq!(telemetry.response_bytes, Some(0));
            },
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn missing_session_id_is_rejected() {
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        let mut request = call("read_file", Some("gpt-5-mini"));
        request["params"]["arguments"]
            .as_object_mut()
            .unwrap()
            .remove(SESSION_ID_ARG);

        match handle_client_message(request, Some(&g), &mut p, &mut pc) {
            (ClientAction::Respond(v), telemetry) => {
                assert_eq!(v["result"]["isError"], json!(true));
                let text = v["result"]["content"][0]["text"].as_str().unwrap();
                assert!(text.contains(SESSION_ID_ARG));
                assert_eq!(
                    telemetry.expect("expected telemetry").decision,
                    "reject-missing-session"
                );
            },
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn expensive_measured_tool_is_refused() {
        // Build a gate with a rollup that measures read_file with cost 75
        use std::sync::atomic::{
            AtomicU64,
            Ordering,
        };
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir();
        let tid = std::thread::current().id();
        let path = dir.join(format!(
            "mcpcg-fixture-{:?}-{}-{}.json",
            tid,
            std::process::id(),
            n
        ));
        let rollup_path = dir.join(format!(
            "mcpcg-rollup-{:?}-{}-{}.json",
            tid,
            std::process::id(),
            n
        ));
        std::fs::write(
            &path,
            r#"{"models":[
                {"provider_id":"anthropic","model_id":"claude-opus-4-1","output_mtok":75.0},
                {"provider_id":"openai","model_id":"gpt-5-mini","output_mtok":2.0}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            &rollup_path,
            r#"{"report":{"tools":[
                {"tool_name":"read_file","call_count":10,"cost":75}
            ]}}"#,
        )
        .unwrap();
        let g = CostGatePolicy::new(
            Gate::load(
                std::path::Path::new(&path),
                toolmon_costgate::ModelBudgetCalibration::default(),
                Some(std::path::Path::new(&rollup_path)),
                None,
            )
            .unwrap(),
        );
        // Clean up temp files after loading
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&rollup_path);

        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        match handle_client_message(
            call("read_file", Some("claude-opus-4-1")),
            Some(&g),
            &mut p,
            &mut pc,
        ) {
            (ClientAction::Respond(v), telemetry) => {
                assert_eq!(v["result"]["isError"], json!(true));
                assert!(
                    v["result"]["content"][0]["text"]
                        .as_str()
                        .unwrap()
                        .to_lowercase()
                        .contains("delegate")
                );
                assert_eq!(
                    telemetry.expect("expected telemetry").decision,
                    "delegate"
                );
            },
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn unmeasured_tool_fail_open() {
        let _routing = active_routing();
        // Without a rollup, even expensive models can call any tool (fail open)
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        match handle_client_message(
            call("read_file", Some("claude-opus-4-1")),
            Some(&g),
            &mut p,
            &mut pc,
        ) {
            (ClientAction::Forward(v), telemetry) => {
                let args = &v["params"]["arguments"];
                assert!(
                    args.get(CALLER_MODEL_ARG).is_none(),
                    "caller_model must be stripped"
                );
                assert!(
                    telemetry.is_none(),
                    "forwarded calls emit telemetry on response, not on forward"
                );
            },
            other => panic!("expected Forward (fail open), got {other:?}"),
        }
    }

    #[test]
    fn cheap_forwards_session_id_and_strips_proxy_arguments() {
        let _routing = active_routing();
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        let mut request = call("some_unknown_tool", Some("gpt-5-mini"));
        request["params"]["arguments"]
            .as_object_mut()
            .unwrap()
            .insert(GRANT_ID_ARG.to_string(), json!("grant-123"));
        // Use a light tool (cost 1) that gpt-5-mini (budget ~97) can afford
        match handle_client_message(request, Some(&g), &mut p, &mut pc) {
            (ClientAction::Forward(v), _) => {
                let args = &v["params"]["arguments"];
                assert!(
                    args.get(CALLER_MODEL_ARG).is_none(),
                    "caller_model must be stripped"
                );
                assert!(
                    args.get(SESSION_ID_ARG).is_some(),
                    "session_id must be forwarded"
                );
                assert!(
                    args.get(GRANT_ID_ARG).is_none(),
                    "grant_id must be stripped"
                );
            },
            other => panic!("expected Forward, got {other:?}"),
        }
    }

    #[test]
    fn forwarded_session_id_keeps_proxy_arguments_private() {
        let _routing = active_routing();
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        let mut request = call("some_unknown_tool", Some("gpt-5-mini"));
        request["params"]["arguments"]
            .as_object_mut()
            .unwrap()
            .insert(GRANT_ID_ARG.to_string(), json!("grant-123"));

        match handle_client_message(request, Some(&g), &mut p, &mut pc) {
            (ClientAction::Forward(v), _) => {
                let args = &v["params"]["arguments"];
                assert_eq!(args[SESSION_ID_ARG], json!(TEST_SESSION_ID));
                assert!(args.get(CALLER_MODEL_ARG).is_none());
                assert!(args.get(GRANT_ID_ARG).is_none());
            },
            other => panic!("expected Forward, got {other:?}"),
        }
    }

    #[test]
    fn expensive_light_tool_forwards() {
        let _routing = active_routing();
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        match handle_client_message(
            call("runSubagent", Some("claude-opus-4-1")),
            Some(&g),
            &mut p,
            &mut pc,
        ) {
            (ClientAction::Forward(_), _) => {},
            other => panic!("expected Forward, got {other:?}"),
        }
    }

    #[test]
    fn unknown_caller_model_is_rejected() {
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        match handle_client_message(
            call("read_file", Some("github-copilot")),
            Some(&g),
            &mut p,
            &mut pc,
        ) {
            (ClientAction::Respond(v), telemetry) => {
                assert_eq!(v["result"]["isError"], json!(true));
                let text = v["result"]["content"][0]["text"].as_str().unwrap();
                assert!(text.to_lowercase().contains("unknown caller_model"));
                assert_eq!(
                    telemetry.expect("expected telemetry").decision,
                    "reject"
                );
            },
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn parenthetical_client_qualifier_is_tolerated() {
        let _routing = active_routing();
        // "gpt-5-mini (copilot)" doesn't match exactly or by substring, but
        // stripping the trailing "(copilot)" qualifier resolves to "gpt-5-mini".
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        match handle_client_message(
            call("some_unknown_tool", Some("gpt-5-mini (copilot)")),
            Some(&g),
            &mut p,
            &mut pc,
        ) {
            (ClientAction::Forward(v), _) => {
                let args = &v["params"]["arguments"];
                assert!(
                    args.get(CALLER_MODEL_ARG).is_none(),
                    "caller_model must be stripped"
                );
            },
            other => panic!(
                "expected Forward (allow after normalization), got {other:?}"
            ),
        }

        // The soft warning surfaces on the eventual server response.
        let resp = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "content": [{ "type": "text", "text": "ok" }] }
        });
        let (out, telemetry) =
            handle_server_message(resp, Some(&g), &mut p, &mut pc);
        assert!(
            out["result"]["costGateWarning"]
                .as_str()
                .unwrap()
                .contains("normalized")
        );
        assert_eq!(telemetry.unwrap().decision, "allow-normalized");
    }

    #[test]
    fn space_and_underscore_separators_are_normalized() {
        let _routing = active_routing();
        // "Claude_Opus 4 1" doesn't match exactly, but normalizing separators
        // to hyphens and lowercasing resolves to "claude-opus-4-1".
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        match handle_client_message(
            call("runSubagent", Some("Claude_Opus 4 1")),
            Some(&g),
            &mut p,
            &mut pc,
        ) {
            (ClientAction::Forward(v), _) => {
                let args = &v["params"]["arguments"];
                assert!(
                    args.get(CALLER_MODEL_ARG).is_none(),
                    "caller_model must be stripped"
                );
            },
            other => panic!(
                "expected Forward (allow after normalization), got {other:?}"
            ),
        }
        let resp = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "content": [{ "type": "text", "text": "ok" }] }
        });
        let (out, _) = handle_server_message(resp, Some(&g), &mut p, &mut pc);
        assert!(out["result"]["costGateWarning"].is_string());
    }

    #[test]
    fn genuinely_unknown_model_still_rejected_after_normalization() {
        // Normalizing "Totally Unknown Model (copilot)" still doesn't match
        // anything in the price table, so the call is rejected as before.
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        match handle_client_message(
            call("read_file", Some("Totally Unknown Model (copilot)")),
            Some(&g),
            &mut p,
            &mut pc,
        ) {
            (ClientAction::Respond(v), telemetry) => {
                assert_eq!(v["result"]["isError"], json!(true));
                let text = v["result"]["content"][0]["text"].as_str().unwrap();
                assert!(text.to_lowercase().contains("unknown caller_model"));
                assert_eq!(
                    telemetry.expect("expected telemetry").decision,
                    "reject"
                );
            },
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn no_gate_is_passthrough() {
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        match handle_client_message(
            call("read_file", None),
            None,
            &mut p,
            &mut pc,
        ) {
            (ClientAction::Forward(_), None) => {},
            other =>
                panic!("expected Forward with no telemetry, got {other:?}"),
        }
    }

    #[test]
    fn tools_list_response_gets_schema_injected() {
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        // Record the list request id.
        let req =
            json!({"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}});
        let g = test_gate();
        let _ = handle_client_message(req, Some(&g), &mut p, &mut pc);

        let resp = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "result": { "tools": [
                { "name": "read_file", "inputSchema": { "type": "object", "properties": {}, "required": [] } },
                { "name": "write_file" }
            ] }
        });
        let (out, telemetry) =
            handle_server_message(resp, Some(&g), &mut p, &mut pc);
        for tool in out["result"]["tools"].as_array().unwrap() {
            assert_eq!(
                tool["inputSchema"]["properties"][CALLER_MODEL_ARG]["type"],
                json!("string")
            );
            assert_eq!(
                tool["inputSchema"]["properties"][SESSION_ID_ARG]["type"],
                json!("string")
            );
            let required = tool["inputSchema"]["required"].as_array().unwrap();
            assert!(required.iter().any(|v| v == CALLER_MODEL_ARG));
            assert!(required.iter().any(|v| v == SESSION_ID_ARG));
        }
        assert!(
            telemetry.is_none(),
            "tools/list response is not a tools/call, no telemetry expected"
        );
    }

    #[test]
    fn inject_creates_schema_when_absent() {
        let mut tool = json!({ "name": "x" });
        inject_caller_model_schema(&mut tool);
        assert_eq!(tool["inputSchema"]["type"], json!("object"));
        assert_eq!(
            tool["inputSchema"]["properties"][SESSION_ID_ARG]["type"],
            json!("string")
        );
        assert_eq!(tool["inputSchema"]["required"][0], json!(CALLER_MODEL_ARG));
        assert_eq!(tool["inputSchema"]["required"][1], json!(SESSION_ID_ARG));
    }

    #[test]
    fn telemetry_computation_is_monotonic() {
        // AC3: larger payloads yield larger estimates
        let small = json!({"a": 1});
        let medium = json!({"a": 1, "b": "hello", "c": [1,2,3]});
        let large = json!({"a": 1, "b": "hello", "c": [1,2,3], "d": {"nested": "structure with more data"}});

        let (bytes_s, chars_s, tokens_s) = compute_payload_telemetry(&small);
        let (bytes_m, chars_m, tokens_m) = compute_payload_telemetry(&medium);
        let (bytes_l, chars_l, tokens_l) = compute_payload_telemetry(&large);

        assert!(
            bytes_s < bytes_m && bytes_m < bytes_l,
            "bytes should be monotonic"
        );
        assert!(
            chars_s < chars_m && chars_m < chars_l,
            "chars should be monotonic"
        );
        assert!(
            tokens_s < tokens_m && tokens_m < tokens_l,
            "tokens_estimated should be monotonic"
        );

        // Verify the chars/4 relationship
        assert_eq!(tokens_s, chars_s / 4);
        assert_eq!(tokens_m, chars_m / 4);
        assert_eq!(tokens_l, chars_l / 4);
    }

    #[test]
    fn telemetry_computation_returns_nonzero() {
        // AC1/AC2: non-empty payloads yield non-zero counts
        let payload = json!({"method": "tools/call", "params": {"name": "read_file", "arguments": {}}});
        let (bytes, chars, tokens) = compute_payload_telemetry(&payload);

        assert!(bytes > 0, "bytes should be non-zero for non-empty payload");
        assert!(chars > 0, "chars should be non-zero for non-empty payload");
        assert!(
            tokens > 0,
            "tokens_estimated should be non-zero for non-empty payload"
        );
    }

    #[test]
    fn allowed_call_emits_nonzero_tokens_estimated_on_response() {
        let _routing = active_routing();
        // (a) A forwarded (allowed) tools/call correlates its response by
        // JSON-RPC id and records a non-zero tokens_estimated derived from
        // the combined request+response payload.
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        let req = call("some_unknown_tool", Some("gpt-5-mini"));
        let (action, telemetry) =
            handle_client_message(req, Some(&g), &mut p, &mut pc);
        assert!(
            telemetry.is_none(),
            "no telemetry until the response arrives"
        );
        let forwarded = match action {
            ClientAction::Forward(v) => v,
            other => panic!("expected Forward, got {other:?}"),
        };
        let id = forwarded["id"].clone();

        let resp = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [{ "type": "text", "text": "some tool output" }] }
        });
        let (_, telemetry) =
            handle_server_message(resp, Some(&g), &mut p, &mut pc);
        let telemetry = telemetry
            .expect("expected telemetry once the response is correlated");
        assert_eq!(telemetry.decision, "allow");
        assert_eq!(telemetry.tool_name, "some_unknown_tool");
        assert!(
            telemetry.response_bytes.unwrap_or(0) > 0,
            "response_bytes should be non-zero"
        );
        assert!(
            telemetry.response_chars.unwrap_or(0) > 0,
            "response_chars should be non-zero"
        );
        assert!(
            telemetry.tokens_estimated.unwrap_or(0) > 0,
            "tokens_estimated should be non-zero for a real intercepted tools/call"
        );
        assert_eq!(
            telemetry.tokens_estimated,
            Some(
                (telemetry.request_chars.unwrap_or(0)
                    + telemetry.response_chars.unwrap_or(0))
                    / 4
            )
        );
    }

    #[test]
    fn duration_ms_is_populated_for_forwarded_calls() {
        let _routing = active_routing();
        // (c) duration_ms measures wall-clock from forward to response receipt.
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        let req = call("some_unknown_tool", Some("gpt-5-mini"));
        let (action, _) = handle_client_message(req, Some(&g), &mut p, &mut pc);
        let forwarded = match action {
            ClientAction::Forward(v) => v,
            other => panic!("expected Forward, got {other:?}"),
        };
        let id = forwarded["id"].clone();

        // Sleep a measurable span so duration_ms is guaranteed nonzero.
        std::thread::sleep(std::time::Duration::from_millis(5));

        let resp =
            json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [] } });
        let (_, telemetry) =
            handle_server_message(resp, Some(&g), &mut p, &mut pc);
        let telemetry = telemetry.expect("expected telemetry");
        assert!(
            telemetry.duration_ms >= 5,
            "duration_ms should reflect the wall-clock span, got {}",
            telemetry.duration_ms
        );
    }

    #[test]
    fn refused_call_records_zero_duration_and_response_counts() {
        // (b/AC4 null-vs-zero): refused calls never reach the server, so
        // response counts and duration_ms are recorded as zero (measured),
        // not omitted — the call itself was still observed.
        let g = test_gate();
        let mut p = PendingList::default();
        let mut pc = PendingCalls::default();
        let (_, telemetry) = handle_client_message(
            call("read_file", None),
            Some(&g),
            &mut p,
            &mut pc,
        );
        let telemetry =
            telemetry.expect("expected telemetry for the refused call");
        assert_eq!(telemetry.response_bytes, Some(0));
        assert_eq!(telemetry.response_chars, Some(0));
        assert_eq!(telemetry.duration_ms, 0);
        assert!(telemetry.request_chars.unwrap_or(0) > 0);
    }
}
