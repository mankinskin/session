//! Per-call telemetry: payload sizing, `caller_model` fallback normalization,
//! and in-flight `tools/call` tracking for duration/decision correlation.

use serde::{
    Deserialize,
    Serialize,
};
use serde_json::Value;

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

pub(crate) fn id_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_default()
}

pub(crate) fn now_rfc3339() -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_computation_is_monotonic() {
        // AC3: larger payloads yield larger estimates
        let small = serde_json::json!({"a": 1});
        let medium = serde_json::json!({"a": 1, "b": "hello", "c": [1,2,3]});
        let large = serde_json::json!({"a": 1, "b": "hello", "c": [1,2,3], "d": {"nested": "structure with more data"}});

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
        let payload = serde_json::json!({"method": "tools/call", "params": {"name": "read_file", "arguments": {}}});
        let (bytes, chars, tokens) = compute_payload_telemetry(&payload);

        assert!(bytes > 0, "bytes should be non-zero for non-empty payload");
        assert!(chars > 0, "chars should be non-zero for non-empty payload");
        assert!(
            tokens > 0,
            "tokens_estimated should be non-zero for non-empty payload"
        );
    }
}
