#[allow(unused_imports)]
use super::*;

impl Supervisor {
    /// Cache `line` if it is the client's `initialize` request or
    /// `notifications/initialized` notification and one has not already
    /// been observed (first-observation-only, per R5: "cache verbatim").
    pub(super) async fn maybe_cache_client_handshake(&self, line: &str) {
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        match method {
            "initialize" => {
                let mut hs = self.handshake.lock().await;
                if hs.init_request.is_none() {
                    hs.init_request_id = msg.get("id").cloned();
                    hs.init_request = Some(msg);
                }
            }
            "notifications/initialized" => {
                let mut hs = self.handshake.lock().await;
                if hs.initialized_notif.is_none() {
                    hs.initialized_notif = Some(msg);
                }
            }
            _ => {}
        }
    }

    /// If `line` is a response whose id matches the cached `initialize`
    /// request id, and no baseline has been captured yet, cache it as the
    /// original-child baseline for future divergence comparisons.
    pub(super) async fn maybe_cache_original_response(&self, line: &str) {
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let Some(id) = msg.get("id") else {
            return;
        };
        if id.is_null() {
            return;
        }
        let mut hs = self.handshake.lock().await;
        if hs.original_response.is_some() {
            return;
        }
        if hs.init_request_id.as_ref() == Some(id) {
            hs.original_response = Some(msg);
        }
    }

    /// Replay the cached handshake into a freshly spawned (not-yet-published)
    /// child: send the cached `initialize` request, await and consume its
    /// response internally (comparing capabilities against the original
    /// baseline and logging divergence), then send `notifications/initialized`.
    ///
    /// Called on `handles` BEFORE it is installed into `self.current`, which
    /// is what rules out both hazards this exists to prevent: no other
    /// caller can reach `handles` via `Supervisor::write_line`/`read_line`
    /// until this returns and publishes it, so (a) no client traffic can
    /// reach the new child ahead of the handshake, and (b) the reader pump
    /// (which only ever reads through `self.current`) cannot observe or
    /// forward the replayed `initialize` response — it is consumed here, off
    /// the client-visible stream, before the client-visible stream exists
    /// for this generation.
    ///
    /// If no handshake has been observed yet (a swap raced ahead of the
    /// client's `initialize`), this is a clean no-op (R5 "swap before
    /// handshake" case) rather than sending a malformed replay.
    pub(super) async fn replay_handshake(&self, handles: &ChildHandles) {
        let (init_request, initialized_notif, original_response) = {
            let hs = self.handshake.lock().await;
            (
                hs.init_request.clone(),
                hs.initialized_notif.clone(),
                hs.original_response.clone(),
            )
        };
        let Some(init_request) = init_request else {
            return;
        };

        let Ok(req_line) = serde_json::to_string(&init_request) else {
            eprintln!(
                "[mcp-toolmon] handshake replay: cached initialize request failed to serialize"
            );
            return;
        };
        if !handles.write_line(&req_line).await {
            eprintln!("[mcp-toolmon] handshake replay: failed to write initialize to new child");
            return;
        }
        match handles.read_line().await {
            Some(resp_line) => match serde_json::from_str::<Value>(&resp_line) {
                Ok(resp) => {
                    log_capability_divergence(original_response.as_ref(), &resp);
                    self.record_divergence(original_response.as_ref(), &resp)
                        .await;
                }
                Err(_) => {
                    eprintln!(
                        "[mcp-toolmon] handshake replay: new child's initialize response was not valid JSON"
                    );
                }
            },
            None => {
                eprintln!(
                    "[mcp-toolmon] handshake replay: new child closed stdout before responding to replayed initialize"
                );
            }
        }

        if let Some(notif) = initialized_notif {
            if let Ok(notif_line) = serde_json::to_string(&notif) {
                let _ = handles.write_line(&notif_line).await;
            }
        }
    }

    /// Record divergence-log entries (test/observability hook; see
    /// `divergence_log` field doc) mirroring [`log_capability_divergence`]'s
    /// stderr output.
    pub(super) async fn record_divergence(&self, original: Option<&Value>, new_response: &Value) {
        let Some(original) = original else {
            return;
        };
        let orig_result = original.get("result");
        let new_result = new_response.get("result");
        let mut entries = Vec::new();
        for field in ["protocolVersion", "capabilities", "serverInfo"] {
            let a = orig_result.and_then(|r| r.get(field));
            let b = new_result.and_then(|r| r.get(field));
            if a != b {
                entries.push(format!("initialize.{field} diverged"));
            }
        }
        if !entries.is_empty() {
            self.divergence_log.lock().await.extend(entries);
        }
    }

    /// Divergence warnings recorded across all swaps so far (test hook).
    pub async fn divergence_log(&self) -> Vec<String> {
        self.divergence_log.lock().await.clone()
    }
}
