//! Pure MCP message interception logic for the cost-gate middleware.
//!
//! These functions operate on parsed JSON-RPC messages so they can be unit
//! tested without spawning processes. The wiring in `main.rs` reads/writes
//! newline-delimited JSON on stdio and calls into here.

#[path = "proxy/gating.rs"]
mod gating;
#[path = "proxy/path_arguments.rs"]
mod path_arguments;
#[path = "proxy/telemetry.rs"]
mod telemetry;
#[path = "proxy/workspace_resolution.rs"]
mod workspace_resolution;

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use serde_json::{Value, json};

use toolmon_policy_api::{
    CALLER_MODEL_ARG, Decision, Policy, SESSION_ID_ARG, inject_caller_model_schema,
};

use gating::{ToolAccess, tool_access};
use path_arguments::{PathArgument, PathArgumentKind, registered_path_argument};
pub use telemetry::{CallTelemetry, PendingCall, PendingCalls};
use telemetry::{compute_payload_telemetry, id_key, normalize_caller_model, now_rfc3339};
use workspace_resolution::{canonicalize_tool_path, resolve_workspace_for_tool};

/// Optional grant id argument for budget offset.
pub const GRANT_ID_ARG: &str = "grant_id";

/// What the proxy should do with a client→server message.
#[derive(Debug)]
pub enum ClientAction {
    /// Forward the message with proxy-only arguments removed.
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
    pub fn record(&mut self, id: &Value) {
        self.ids.insert(id_key(id));
    }

    pub fn take(&mut self, id: &Value) -> bool {
        self.ids.remove(&id_key(id))
    }

    fn record_path_arguments(&mut self, tool: &Value) {
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

    fn path_arguments(&self, tool: &str) -> Vec<PathArgument> {
        self.path_arguments.get(tool).cloned().unwrap_or_default()
    }
}

/// Build a `tools/call` result carrying an error message (isError=true).
fn error_result(id: &Value, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "isError": true
        }
    })
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
        }
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
            let (request_bytes, request_chars, _) = compute_payload_telemetry(&msg);

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
                let telemetry = immediate_telemetry("reject-missing-model", None);
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
                let telemetry = immediate_telemetry("reject-missing-session", Some(caller_model));
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

            match policy.evaluate(&effective_model, &tool, grant_id.as_deref()) {
                Decision::Reject { guidance } => {
                    let telemetry = immediate_telemetry("reject", Some(caller_model));
                    (
                        ClientAction::Respond(error_result(&id, &guidance)),
                        Some(telemetry),
                    )
                }
                Decision::Delegate { guidance } => {
                    let telemetry = immediate_telemetry("delegate", Some(caller_model));
                    (
                        ClientAction::Respond(error_result(&id, &guidance)),
                        Some(telemetry),
                    )
                }
                Decision::Allow => {
                    let path_arguments = pending.path_arguments(&tool);
                    let workspace = path_arguments
                        .iter()
                        .find(|argument| argument.kind == PathArgumentKind::Workspace)
                        .and_then(|argument| msg["params"]["arguments"][argument.name].as_str())
                        .map(|value| {
                            if tool != "session_unassign"
                                && (value.is_empty() || value == "default")
                            {
                                "."
                            } else {
                                value
                            }
                        });
                    let (_, store_root) =
                        match resolve_workspace_for_tool(&tool, &session_id, workspace) {
                            Ok(resolved) => resolved,
                            Err(error) => {
                                let telemetry =
                                    immediate_telemetry("reject-workspace", Some(caller_model));
                                return (
                                    ClientAction::Respond(error_result(&id, &error)),
                                    Some(telemetry),
                                );
                            }
                        };
                    for argument in &path_arguments {
                        let value = msg
                            .get("params")
                            .and_then(|params| params.get("arguments"))
                            .and_then(|arguments| arguments.get(argument.name))
                            .and_then(Value::as_str)
                            .or_else(|| {
                                (argument.kind == PathArgumentKind::Workspace
                                    && tool_access(&tool) == ToolAccess::Mutation)
                                    .then_some(".")
                            });
                        let Some(value) = value else {
                            continue;
                        };
                        let value = if argument.kind == PathArgumentKind::Workspace
                            && tool != "session_unassign"
                            && (value.is_empty() || value == "default")
                        {
                            "."
                        } else {
                            value
                        };
                        if argument.kind == PathArgumentKind::ReadSource {
                            let source = canonicalize_tool_path(Path::new(value))
                                .ok()
                                .filter(|source| source.is_file());
                            if source.is_none() {
                                let telemetry =
                                    immediate_telemetry("reject-workspace", Some(caller_model));
                                let error = format!(
                                    "copy source '{value}' does not exist or is not a file"
                                );
                                return (
                                    ClientAction::Respond(error_result(&id, &error)),
                                    Some(telemetry),
                                );
                            }
                            continue;
                        }
                        let validation = if argument.kind == PathArgumentKind::RenameDestination
                            && !Path::new(value).is_absolute()
                        {
                            let from = msg["params"]["arguments"]["from"].as_str();
                            match from {
                                Some(from) => {
                                    canonicalize_tool_path(Path::new(from)).and_then(|source| {
                                        let parent = source.parent().ok_or_else(|| {
                                            format!("cannot determine rename parent for '{from}'")
                                        })?;
                                        let destination = parent.join(value);
                                        resolve_workspace_for_tool(
                                            &tool,
                                            &session_id,
                                            Some(&destination.to_string_lossy()),
                                        )
                                    })
                                }
                                None => continue,
                            }
                        } else {
                            resolve_workspace_for_tool(&tool, &session_id, Some(value))
                        };
                        match validation {
                            Ok(_) => {}
                            Err(error) => {
                                let telemetry =
                                    immediate_telemetry("reject-workspace", Some(caller_model));
                                return (
                                    ClientAction::Respond(error_result(&id, &error)),
                                    Some(telemetry),
                                );
                            }
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
                }
            }
        }
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
    let telemetry = msg
        .get("id")
        .and_then(|id| pending_calls.take(id))
        .map(|call| {
            let (response_bytes, response_chars, _) = compute_payload_telemetry(&msg);
            let duration_ms = call.started_at.elapsed().as_millis() as u64;
            let tokens_estimated = (call.request_chars + response_chars) / 4;
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
        if let Some(result) = msg.get_mut("result").and_then(Value::as_object_mut) {
            result.insert("costGateWarning".to_string(), json!(warning));
        }
    }

    let is_list_response = msg.get("id").map(|id| pending.take(id)).unwrap_or(false)
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
#[path = "proxy/tests.rs"]
mod tests;
