//! Async child-process transport core plus the child lifecycle supervisor
//! (T4): swap, pending-request failure synthesis, and never-exit fallback.
//!
//! `Supervisor` owns the current child generation (process handle + stdio
//! pipes) behind one `RwLock<Option<Arc<ChildHandles>>>`. Every `read_line`/
//! `write_line` call snapshots the `Arc<ChildHandles>` under a single lock
//! acquisition, so a caller can never observe stdin from one generation
//! paired with stdout from another (see [`ChildHandles`] doc for why this
//! rules out the mismatched-generation race flagged during T2).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, Notify, RwLock},
};

use crate::shadow;

/// JSON-RPC error code used for synthesized reload-interruption responses
/// (R6). In the `-32000`..`-32099` server-error range reserved by the spec.
pub const RELOAD_ERROR_CODE: i64 = -32001;

/// Default drain grace period (R10), overridable via `TOOLMON_DRAIN_MS`.
const DEFAULT_DRAIN_MS: u64 = 2000;

/// How long a freshly spawned child is given to prove it hasn't already
/// exited (e.g. a corrupt/non-executable binary) before it is trusted.
const LIVENESS_CHECK_MS: u64 = 20;

/// Max respawn attempts before falling back to the last-known-good shadow (R7).
const MAX_RESPAWN_ATTEMPTS: u32 = 4;
/// Initial backoff delay between respawn attempts; doubles each attempt.
const INITIAL_BACKOFF_MS: u64 = 25;
/// Backoff never grows past this bound (R7: "bounded max interval").
const MAX_BACKOFF_MS: u64 = 500;

/// Crash-loop bound (T6, item 6): more than this many *unexpected* child
/// exits within `CRASH_THROTTLE_WINDOW_MS` triggers a cooldown sleep before
/// each further automatic recovery. This never refuses to recover (R7
/// still holds — the proxy always eventually retries), it only slows a hot
/// crash-loop (e.g. a binary that spawns and immediately dies every time)
/// down so it cannot spin the CPU with back-to-back respawn attempts.
const CRASH_THROTTLE_MAX: u32 = 5;
const CRASH_THROTTLE_WINDOW_MS: u64 = 10_000;
const CRASH_THROTTLE_COOLDOWN_MS: u64 = MAX_BACKOFF_MS;

/// Sliding-window counter backing [`Supervisor::throttle_crash_recovery`].
struct CrashThrottle {
    window_start: tokio::time::Instant,
    count: u32,
}

impl Default for CrashThrottle {
    fn default() -> Self {
        Self {
            window_start: tokio::time::Instant::now(),
            count: 0,
        }
    }
}

/// One spawned child generation: its process handle, stdio pipes, and the
/// shadow path it was launched from, bundled into a single struct behind one
/// `Arc`.
///
/// This is the atomicity fix for the race flagged during T2: with `child`,
/// `stdin`, and `stdout` as three independent `Supervisor`-level mutexes, a
/// concurrent reader could observe a `stdout` from generation N+1 paired
/// with a `stdin`/`child` still from generation N if a swap interleaved
/// between two separate lock acquisitions. Bundling all three into one
/// `ChildHandles`, reached only via `Arc<ChildHandles>` cloned out of
/// `Supervisor::current` under ONE lock acquisition, makes that
/// interleaving structurally impossible: every `read_line`/`write_line` call
/// operates on fields that all belong to the same immutable generation
/// snapshot, never a mix of two. The per-field inner mutexes below exist
/// only so a slow blocking stdout read doesn't stall a concurrent stdin
/// write to that SAME generation; they do not reintroduce cross-generation
/// mismatch because both are reached through the same `Arc`.
struct ChildHandles {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    stdout: Mutex<BufReader<ChildStdout>>,
    shadow_path: Option<PathBuf>,
}

impl ChildHandles {
    async fn write_line(&self, line: &str) -> bool {
        let mut guard = self.stdin.lock().await;
        let Some(stdin) = guard.as_mut() else {
            return false;
        };
        stdin.write_all(line.as_bytes()).await.is_ok()
            && stdin.write_all(b"\n").await.is_ok()
            && stdin.flush().await.is_ok()
    }

    async fn read_line(&self) -> Option<String> {
        let mut stdout = self.stdout.lock().await;
        let mut buf = String::new();
        match stdout.read_line(&mut buf).await {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                if buf.ends_with('\n') {
                    buf.pop();
                    if buf.ends_with('\r') {
                        buf.pop();
                    }
                }
                Some(buf)
            }
        }
    }

    /// Close stdin (so the child observes EOF), kill it, and wait. Best-effort.
    async fn kill_and_wait(&self) {
        {
            let mut stdin = self.stdin.lock().await;
            *stdin = None;
        }
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
    }

    /// `true` if the child has already exited — used to validate a fresh
    /// spawn before trusting it (catches corrupt/non-executable binaries
    /// that spawn "successfully" as an OS process but exit immediately).
    async fn has_exited(&self) -> bool {
        let mut child = self.child.lock().await;
        matches!(child.try_wait(), Ok(Some(_)))
    }
}

fn id_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_default()
}

/// Build the JSON-RPC error response synthesized for a request that was
/// in flight when its child was torn down (R6).
fn synthesize_error(id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": RELOAD_ERROR_CODE,
            "message": "mcp-toolmon: server reload interrupted this in-flight request",
        }
    })
}

/// Handshake replay cache (T5, R5): the client's original `initialize`
/// request and `notifications/initialized` notification, captured verbatim
/// on first observation, plus the first child's `initialize` response used
/// as the divergence-comparison baseline.
#[derive(Default)]
struct HandshakeCache {
    init_request: Option<Value>,
    init_request_id: Option<Value>,
    initialized_notif: Option<Value>,
    original_response: Option<Value>,
}

/// Compare `field` (`protocolVersion` / `capabilities` / `serverInfo`) of a
/// new child's `initialize` result against the cached original baseline;
/// log a warning per differing field. Divergence is never fatal (R5).
fn log_capability_divergence(original: Option<&Value>, new_response: &Value) {
    let Some(original) = original else {
        return;
    };
    let orig_result = original.get("result");
    let new_result = new_response.get("result");
    for field in ["protocolVersion", "capabilities", "serverInfo"] {
        let a = orig_result.and_then(|r| r.get(field));
        let b = new_result.and_then(|r| r.get(field));
        if a != b {
            eprintln!(
                "[mcp-toolmon] handshake divergence in initialize.{field}: original={} new={}",
                a.map(Value::to_string)
                    .unwrap_or_else(|| "null".to_string()),
                b.map(Value::to_string)
                    .unwrap_or_else(|| "null".to_string()),
            );
        }
    }
}

/// Owns the current child generation, the in-flight request table, and the
/// last-known-good shadow path used as the R7 fallback.
pub struct Supervisor {
    command: Vec<String>,
    shadow_root: PathBuf,
    current: RwLock<Option<Arc<ChildHandles>>>,
    last_known_good: Mutex<Option<PathBuf>>,
    shadow_dirs: Mutex<Vec<PathBuf>>,
    pending: Mutex<HashMap<String, Value>>,
    pending_notify: Notify,
    retry_count: AtomicU64,
    shutting_down: AtomicBool,
    handshake: Mutex<HandshakeCache>,
    /// Human-readable capability-divergence warnings observed across swaps
    /// (test/observability hook mirroring the stderr `eprintln!` in
    /// [`log_capability_divergence`], since stderr itself is not easily
    /// assertable from an in-process async test).
    divergence_log: Mutex<Vec<String>>,
    /// Lines queued by the crash-auto-recovery path in [`Supervisor::read_line`]
    /// (synthesized reload-interruption errors for requests that were
    /// in-flight when the child crashed) — drained ahead of real child
    /// output by the next `read_line` call(s), so a caller looping on
    /// `read_line` (the production reader pump) delivers them to the
    /// client exactly like any other server message (T6, item 5/6).
    outgoing_queue: Mutex<std::collections::VecDeque<String>>,
    /// T6 item 6: bounds automatic recovery from unexpected (non-swap)
    /// child exits so a hot crash-loop cannot spin.
    crash_throttle: Mutex<CrashThrottle>,
}

impl Supervisor {
    /// Spawn `command[0]` with `command[1..]` as args, piping stdin/stdout and
    /// inheriting stderr, from a private shadow copy (see [`shadow`]) using
    /// the default shadow root (`TOOLMON_SHADOW_DIR` or the system temp dir).
    pub fn spawn(command: &[String]) -> std::io::Result<Self> {
        Self::spawn_with_shadow_dir(command, None)
    }

    /// Like [`Supervisor::spawn`], but `shadow_dir_override` takes precedence
    /// over `TOOLMON_SHADOW_DIR` and the default temp dir.
    pub fn spawn_with_shadow_dir(
        command: &[String],
        shadow_dir_override: Option<&Path>,
    ) -> std::io::Result<Self> {
        let root = shadow::shadow_root(shadow_dir_override);
        shadow::sweep_startup(&root);

        let handles = spawn_handles(command, &root)?;
        let mut shadow_dirs = Vec::new();
        let mut last_known_good = None;
        if let Some(p) = &handles.shadow_path {
            if let Some(dir) = p.parent() {
                shadow_dirs.push(dir.to_path_buf());
            }
            if let Ok(snapshot) = snapshot_last_known_good(p, &root) {
                if let Some(dir) = snapshot.parent() {
                    shadow_dirs.push(dir.to_path_buf());
                }
                last_known_good = Some(snapshot);
            }
        }

        Ok(Self {
            command: command.to_vec(),
            shadow_root: root,
            current: RwLock::new(Some(Arc::new(handles))),
            last_known_good: Mutex::new(last_known_good),
            shadow_dirs: Mutex::new(shadow_dirs),
            pending: Mutex::new(HashMap::new()),
            pending_notify: Notify::new(),
            retry_count: AtomicU64::new(0),
            shutting_down: AtomicBool::new(false),
            handshake: Mutex::new(HandshakeCache::default()),
            divergence_log: Mutex::new(Vec::new()),
            outgoing_queue: Mutex::new(std::collections::VecDeque::new()),
            crash_throttle: Mutex::new(CrashThrottle::default()),
        })
    }

    /// The shadow-copy root directory this supervisor spawns children under
    /// (`TOOLMON_SHADOW_DIR` or the system temp dir) — exposed so the
    /// binary watcher (T6) can derive scratch-copy paths under the same
    /// root without duplicating env/default resolution logic.
    pub fn shadow_root_path(&self) -> &Path {
        &self.shadow_root
    }

    /// The shadow path the currently active child was spawned from, or
    /// `None` if no healthy child is running or the shadow copy fell back to
    /// the canonical path. Non-blocking: safe to call right after spawn
    /// before any concurrent swap could be contending for the lock.
    pub fn shadow_path(&self) -> Option<PathBuf> {
        self.current.try_read().ok()?.as_ref()?.shadow_path.clone()
    }

    /// `true` if a healthy child is currently running.
    pub async fn has_healthy_child(&self) -> bool {
        self.current.read().await.is_some()
    }

    /// Number of respawn attempts that have failed validation across all
    /// swaps so far (test/observability hook for R7 backoff behavior).
    pub fn retry_count(&self) -> u64 {
        self.retry_count.load(Ordering::SeqCst)
    }

    /// Record a client→server request id as in-flight, so that a swap
    /// occurring before its response arrives can synthesize a failure for it
    /// (R6). Notifications (no id, or a JSON `null` id) are not tracked.
    pub async fn record_pending(&self, id: &Value) {
        if id.is_null() {
            return;
        }
        self.pending.lock().await.insert(id_key(id), id.clone());
    }

    /// Mark a request id as resolved (its response arrived from the child)
    /// so it is no longer a candidate for synthesized-error failure.
    pub async fn resolve_pending(&self, id: &Value) {
        let mut pending = self.pending.lock().await;
        if pending.remove(&id_key(id)).is_some() {
            drop(pending);
            self.pending_notify.notify_waiters();
        }
    }

    /// If `id` is still pending, remove it and return a synthesized
    /// reload-interruption error for it regardless (used when a write to the
    /// child fails outright, e.g. no healthy child is currently running).
    pub async fn synthesize_and_clear(&self, id: &Value) -> Value {
        self.pending.lock().await.remove(&id_key(id));
        synthesize_error(id)
    }

    async fn drain(&self, drain_ms: u64) {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(drain_ms);
        loop {
            let notified = self.pending_notify.notified();
            if self.pending.lock().await.is_empty() {
                return;
            }
            tokio::pin!(notified);
            tokio::select! {
                _ = &mut notified => continue,
                _ = tokio::time::sleep_until(deadline) => return,
            }
        }
    }

    /// Drain the pending table into synthesized error responses (R6),
    /// leaving it empty.
    async fn fail_all_pending(&self) -> Vec<Value> {
        let mut pending = self.pending.lock().await;
        let ids: Vec<Value> = pending.values().cloned().collect();
        pending.clear();
        drop(pending);
        ids.iter().map(synthesize_error).collect()
    }

    /// Write one line (newline appended) to the current child's stdin.
    /// Returns `false` if there is no healthy child right now, or the write
    /// failed.
    ///
    /// As a side effect (R5), this observes the client's `initialize`
    /// request and `notifications/initialized` notification the first time
    /// each passes through, caching them verbatim for replay into future
    /// child generations.
    pub async fn write_line(&self, line: &str) -> bool {
        self.maybe_cache_client_handshake(line).await;
        let handles = { self.current.read().await.clone() };
        match handles {
            Some(h) => h.write_line(line).await,
            None => false,
        }
    }

    /// Read one line from the current child's stdout. While no healthy
    /// child is running (mid-outage, R7) this waits rather than returning
    /// `None`, so the reader pump is not torn down by a transient outage;
    /// it returns `None` only once the supervisor is shutting down.
    ///
    /// As a side effect (R5), the first response correlating to the cached
    /// `initialize` request id is captured as the divergence-comparison
    /// baseline.
    ///
    /// T6 item 6: if the current child's stdout hits EOF (process exited)
    /// while the supervisor is NOT shutting down, that is an *unexpected*
    /// crash (as opposed to the deliberate teardown inside
    /// [`Supervisor::swap_child_with_drain_ms`], which always replaces
    /// `current` before the old handle can be read from again). This
    /// triggers the same respawn path a watcher-triggered swap uses,
    /// throttled by [`Supervisor::throttle_crash_recovery`] so a hot
    /// crash-loop cannot spin, then continues the loop to serve from the
    /// recovered child. Any synthesized errors from that recovery are
    /// queued (see `outgoing_queue`) and drained ahead of real output.
    pub async fn read_line(&self) -> Option<String> {
        loop {
            if let Some(line) = self.outgoing_queue.lock().await.pop_front() {
                return Some(line);
            }
            let handles = { self.current.read().await.clone() };
            match handles {
                Some(h) => match h.read_line().await {
                    Some(l) => {
                        self.maybe_cache_original_response(&l).await;
                        return Some(l);
                    }
                    None => {
                        if self.shutting_down.load(Ordering::SeqCst) {
                            return None;
                        }
                        eprintln!(
                            "[mcp-toolmon] child exited unexpectedly (not a triggered swap); attempting automatic recovery"
                        );
                        self.throttle_crash_recovery().await;
                        let synthesized = self.swap_child_with_drain_ms(0).await;
                        if !synthesized.is_empty() {
                            let mut q = self.outgoing_queue.lock().await;
                            for err in synthesized {
                                if let Ok(s) = serde_json::to_string(&err) {
                                    q.push_back(s);
                                }
                            }
                        }
                        continue;
                    }
                },
                None => {
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    /// Test-only hook: kill the OS process backing the current child
    /// WITHOUT going through the normal swap/respawn path, so integration
    /// tests can simulate a spontaneous crash (as opposed to a
    /// binary-change-triggered swap) and exercise [`Supervisor::read_line`]'s
    /// automatic recovery. `current` is left pointing at the (now-dead)
    /// handle, exactly like a real crash would leave it.
    #[doc(hidden)]
    pub async fn force_kill_current_for_test(&self) {
        if let Some(h) = { self.current.read().await.clone() } {
            let mut child = h.child.lock().await;
            let _ = child.start_kill();
        }
    }
}

#[path = "supervisor/handshake.rs"]
mod handshake;

#[path = "supervisor/recovery.rs"]
mod recovery;

#[path = "supervisor/child_process.rs"]
mod child_process;
#[allow(unused_imports)]
pub use child_process::*;
