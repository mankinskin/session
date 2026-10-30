#[allow(unused_imports)]
use super::*;

impl Supervisor {
    /// Bound automatic crash-triggered respawns (T6, item 6): more than
    /// `CRASH_THROTTLE_MAX` unexpected exits within `CRASH_THROTTLE_WINDOW_MS`
    /// makes each further automatic recovery in that window sleep
    /// `CRASH_THROTTLE_COOLDOWN_MS` before proceeding. Never gives up (R7).
    pub(super) async fn throttle_crash_recovery(&self) {
        let should_cooldown = {
            let mut t = self.crash_throttle.lock().await;
            let now = tokio::time::Instant::now();
            if now.duration_since(t.window_start) > Duration::from_millis(CRASH_THROTTLE_WINDOW_MS)
            {
                t.window_start = now;
                t.count = 0;
            }
            t.count += 1;
            t.count > CRASH_THROTTLE_MAX
        };
        if should_cooldown {
            eprintln!(
                "[mcp-toolmon] child has crashed repeatedly within {CRASH_THROTTLE_WINDOW_MS}ms; throttling automatic recovery"
            );
            tokio::time::sleep(Duration::from_millis(CRASH_THROTTLE_COOLDOWN_MS)).await;
        }
    }

    /// Kill the current child and respawn a replacement from a freshly
    /// re-copied shadow binary, using `TOOLMON_DRAIN_MS` (default 2000) as
    /// the drain grace period. Returns synthesized JSON-RPC error responses
    /// (R6) for any request ids still pending after the drain window — the
    /// caller (currently tests; T6's watcher in production) is responsible
    /// for delivering these to the client transport.
    ///
    /// This is the injectable seam (spec C1): callable directly, with no
    /// polling or wall-clock waiting of its own.
    pub async fn swap_child(&self) -> Vec<Value> {
        let drain_ms = std::env::var("TOOLMON_DRAIN_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_DRAIN_MS);
        self.swap_child_with_drain_ms(drain_ms).await
    }

    /// Like [`Supervisor::swap_child`] but with an explicit drain window,
    /// bypassing `TOOLMON_DRAIN_MS` — used by tests to keep the drain bound
    /// tight without racing the env var across parallel test threads.
    pub async fn swap_child_with_drain_ms(&self, drain_ms: u64) -> Vec<Value> {
        self.drain(drain_ms).await;
        let synthesized = self.fail_all_pending().await;

        let outgoing = { self.current.write().await.take() };
        if let Some(outgoing) = &outgoing {
            outgoing.kill_and_wait().await;
        }

        let mut attempt = 0u32;
        let mut backoff = Duration::from_millis(INITIAL_BACKOFF_MS);
        loop {
            match spawn_handles(&self.command, &self.shadow_root) {
                Ok(handles) => {
                    tokio::time::sleep(Duration::from_millis(LIVENESS_CHECK_MS)).await;
                    if !handles.has_exited().await {
                        self.replay_handshake(&handles).await;
                        self.adopt_healthy(handles).await;
                        return synthesized;
                    }
                    eprintln!(
                        "[mcp-toolmon] respawned child exited immediately (attempt {attempt}); retrying"
                    );
                }
                Err(e) => {
                    eprintln!("[mcp-toolmon] respawn attempt {attempt} failed to spawn: {e}");
                }
            }
            self.retry_count.fetch_add(1, Ordering::SeqCst);
            attempt += 1;
            if attempt >= MAX_RESPAWN_ATTEMPTS {
                break;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_millis(MAX_BACKOFF_MS));
        }

        // Retries exhausted: restore service from the last-known-good
        // snapshot (R7) rather than leaving the client without a child.
        let fallback_path = self.last_known_good.lock().await.clone();
        if let Some(path) = fallback_path {
            match spawn_exe(&path, &self.command[1..], Some(path.clone())) {
                Ok(handles) => {
                    self.replay_handshake(&handles).await;
                    *self.current.write().await = Some(Arc::new(handles));
                    eprintln!(
                        "[mcp-toolmon] restored service from last-known-good shadow copy after {attempt} failed respawn attempt(s)"
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[mcp-toolmon] last-known-good fallback spawn failed too ({e}); no healthy child until the next swap"
                    );
                }
            }
        } else {
            eprintln!(
                "[mcp-toolmon] no last-known-good shadow copy available; no healthy child until the next swap"
            );
        }
        // Either way, the proxy process itself is never torn down here (R7):
        // `current` may legitimately be `None`, in which case `write_line`
        // reports unavailability and `read_line` waits instead of exiting.
        synthesized
    }

    pub(super) async fn adopt_healthy(&self, handles: ChildHandles) {
        if let Some(p) = &handles.shadow_path {
            if let Some(dir) = p.parent() {
                self.shadow_dirs.lock().await.push(dir.to_path_buf());
            }
            if let Ok(snapshot) = snapshot_last_known_good(p, &self.shadow_root) {
                if let Some(dir) = snapshot.parent() {
                    self.shadow_dirs.lock().await.push(dir.to_path_buf());
                }
                *self.last_known_good.lock().await = Some(snapshot);
            }
        }
        *self.current.write().await = Some(Arc::new(handles));
    }

    /// Close the current child's stdin so it observes EOF, wait for exit,
    /// return its exit code (0 if unavailable), and best-effort delete this
    /// process's own shadow directories. Shadow cleanup failures are never
    /// fatal and never block shutdown.
    pub async fn shutdown(&self) -> i32 {
        self.shutting_down.store(true, Ordering::SeqCst);
        let handles = { self.current.write().await.take() };
        let code = if let Some(h) = &handles {
            {
                let mut stdin = h.stdin.lock().await;
                *stdin = None;
            }
            let mut child = h.child.lock().await;
            child.wait().await.ok().and_then(|s| s.code()).unwrap_or(0)
        } else {
            0
        };
        self.cleanup_shadow_dirs().await;
        code
    }

    pub(super) async fn cleanup_shadow_dirs(&self) {
        let dirs = { std::mem::take(&mut *self.shadow_dirs.lock().await) };
        for dir in dirs {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
