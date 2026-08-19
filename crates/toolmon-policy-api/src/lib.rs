//! Transport-agnostic policy boundary for `mcp-toolmon`.
//!
//! `proxy.rs` calls into a `Policy` trait object at each interception point
//! instead of calling gate-engine functions directly, so the transport
//! (reload/lifecycle, message pumping) stays policy-agnostic and the cost
//! gate (`toolmon-costgate`) is the only crate that knows how a verdict is
//! computed. This crate has zero knowledge of the cost gate and must never
//! depend on `mcp-toolmon` or `toolmon-costgate` — that is what breaks the
//! `policy.rs` <-> `proxy.rs` dependency cycle that existed before the split.
//!
//! Future direction (deferred, not in scope for the crate split that created
//! this boundary): this trait is intended to eventually become a dynamically
//! loaded plugin boundary, so a policy/gate change needs no host reload.
//! Known FFI blockers an implementer will need to solve first, found during
//! the audit that motivated the split:
//! * [`Policy::on_tools_list`] takes `&mut serde_json::Value` — not FFI-safe.
//! * [`Decision`] is a Rust enum carrying owned `String` payloads.
//! * `Arc<dyn Policy>` is Rust-ABI only; it is not a stable plugin ABI.
//! * Borrowed `&str` / `Option<&str>` parameters (e.g. `evaluate`'s
//!   `grant_id`) are not FFI-safe either.

use serde_json::Value;

/// The argument name injected into every tool schema and required on each call.
pub const CALLER_MODEL_ARG: &str = "caller_model";

/// The session anchor injected into every tool schema and required on each call.
pub const SESSION_ID_ARG: &str = "session_id";

/// Outcome of a policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Delegate {
        guidance: String,
    },
    /// The `caller_model` could not be resolved by the policy. The call is
    /// refused so price-awareness enforcement is never silently bypassed by
    /// an unrecognized model id.
    Reject {
        guidance: String,
    },
}

/// Hook points the proxy asks policy at its interception points.
pub trait Policy: Send + Sync {
    /// Mutate a single tool schema advertised by a `tools/list` response
    /// (e.g. inject a required argument). Called once per advertised tool.
    fn on_tools_list(
        &self,
        tool: &mut Value,
    );

    /// Whether `caller_model` resolves to a known entry this policy tracks.
    fn resolves(
        &self,
        caller_model: &str,
    ) -> bool;

    /// Evaluate an outbound `tools/call` and return an allow/delegate/reject verdict.
    fn evaluate(
        &self,
        caller_model: &str,
        tool: &str,
        grant_id: Option<&str>,
    ) -> Decision;
}

/// Ensure a single tool object requires a `caller_model` string argument.
pub fn inject_caller_model_schema(tool: &mut Value) {
    let Some(obj) = tool.as_object_mut() else {
        return;
    };
    let schema = obj
        .entry("inputSchema")
        .or_insert_with(|| serde_json::json!({ "type": "object" }));
    let Some(schema_obj) = schema.as_object_mut() else {
        return;
    };
    schema_obj
        .entry("type")
        .or_insert_with(|| serde_json::json!("object"));

    let props = schema_obj
        .entry("properties")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(props_obj) = props.as_object_mut() {
        props_obj.insert(
            CALLER_MODEL_ARG.to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Id of the model issuing this call (e.g. claude-opus-4-8). Required for price-awareness enforcement. Client-appended qualifiers such as 'Claude Sonnet 5 (copilot)', and space/underscore separators, are tolerated as a fallback and normalized to hyphens; prefer the exact price-table model_id."
            }),
        );
        props_obj.insert(
            SESSION_ID_ARG.to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Id of the session this call belongs to. Required so the call is anchored to that session's worktree rather than silently resolving to the server process working directory."
            }),
        );
    }

    let required = schema_obj
        .entry("required")
        .or_insert_with(|| serde_json::json!([]));
    if let Some(arr) = required.as_array_mut() {
        if !arr.iter().any(|v| v.as_str() == Some(CALLER_MODEL_ARG)) {
            arr.push(serde_json::json!(CALLER_MODEL_ARG));
        }
        if !arr.iter().any(|v| v.as_str() == Some(SESSION_ID_ARG)) {
            arr.push(serde_json::json!(SESSION_ID_ARG));
        }
    }
}
