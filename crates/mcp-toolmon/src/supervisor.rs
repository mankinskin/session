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
    path::{
        Path,
        PathBuf,
    },
    process::Stdio,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            AtomicU64,
            Ordering,
        },
    },
    time::Duration,
};

use serde_json::{
    Value,
    json,
};
use tokio::{
    io::{
        AsyncBufReadExt,
        AsyncWriteExt,
        BufReader,
    },
    process::{
        Child,
        ChildStdin,
        ChildStdout,
        Command,
    },
    sync::{
        Mutex,
        Notify,
        RwLock,
    },
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
    async fn write_line(
        &self,
        line: &str,
    ) -> bool {
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
            },
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

fn resolve_shadow_exe(
    command: &[String],
    root: &Path,
) -> (PathBuf, Option<PathBuf>) {
    match shadow::resolve_canonical(&command[0]) {
        Ok(canonical) => match shadow::make_shadow_copy(&canonical, root) {
            Ok(shadow_exe) => (shadow_exe.clone(), Some(shadow_exe)),
            Err(e) => {
                eprintln!(
                    "[mcp-toolmon] shadow copy of {} failed ({e}); falling back to spawning the canonical path directly",
                    canonical.display()
                );
                (canonical, None)
            },
        },
        Err(e) => {
            eprintln!(
                "[mcp-toolmon] canonical path resolution failed for {:?} ({e}); spawning as given",
                command[0]
            );
            (PathBuf::from(&command[0]), None)
        },
    }
}

fn spawn_exe(
    exe: &Path,
    args: &[String],
    shadow_path: Option<PathBuf>,
) -> std::io::Result<ChildHandles> {
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    Ok(ChildHandles {
        child: Mutex::new(child),
        stdin: Mutex::new(Some(stdin)),
        stdout: Mutex::new(BufReader::new(stdout)),
        shadow_path,
    })
}

fn spawn_handles(
    command: &[String],
    root: &Path,
) -> std::io::Result<ChildHandles> {
    let (exe, shadow_path) = resolve_shadow_exe(command, root);
    spawn_exe(&exe, &command[1..], shadow_path)
}

/// Snapshot a healthy generation's shadow exe into a dedicated path that
/// regular re-copies of the (possibly now-corrupt) canonical binary can
/// never collide with or overwrite.
///
/// `shadow::make_shadow_copy` keys its destination directory only by a hash
/// of the CANONICAL path, not by content or call count, so calling it twice
/// in a row for the same canonical path copies into the SAME destination
/// file. Left unguarded, a failed respawn attempt would overwrite the last
/// known-good shadow copy with corrupt bytes before we could fall back to
/// it, defeating R7. This snapshot is what makes the fallback durable.
fn snapshot_last_known_good(
    shadow_exe: &Path,
    root: &Path,
) -> std::io::Result<PathBuf> {
    let pid = std::process::id();
    let dir = root.join(format!("lastgood-{pid}"));
    std::fs::create_dir_all(&dir)?;
    let file_name = shadow_exe.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "shadow exe has no file name",
        )
    })?;
    let dest = dir.join(file_name);
    std::fs::copy(shadow_exe, &dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms)?;
    }
    Ok(dest)
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
fn log_capability_divergence(
    original: Option<&Value>,
    new_response: &Value,
) {
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
    pub async fn record_pending(
        &self,
        id: &Value,
    ) {
        if id.is_null() {
            return;
        }
        self.pending.lock().await.insert(id_key(id), id.clone());
    }

    /// Mark a request id as resolved (its response arrived from the child)
    /// so it is no longer a candidate for synthesized-error failure.
    pub async fn resolve_pending(
        &self,
        id: &Value,
    ) {
        let mut pending = self.pending.lock().await;
        if pending.remove(&id_key(id)).is_some() {
            drop(pending);
            self.pending_notify.notify_waiters();
        }
    }

    /// If `id` is still pending, remove it and return a synthesized
    /// reload-interruption error for it regardless (used when a write to the
    /// child fails outright, e.g. no healthy child is currently running).
    pub async fn synthesize_and_clear(
        &self,
        id: &Value,
    ) -> Value {
        self.pending.lock().await.remove(&id_key(id));
        synthesize_error(id)
    }

    async fn drain(
        &self,
        drain_ms: u64,
    ) {
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(drain_ms);
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
    pub async fn write_line(
        &self,
        line: &str,
    ) -> bool {
        self.maybe_cache_client_handshake(line).await;
        let handles = { self.current.read().await.clone() };
        match handles {
            Some(h) => h.write_line(line).await,
            None => false,
        }
    }

    /// Cache `line` if it is the client's `initialize` request or
    /// `notifications/initialized` notification and one has not already
    /// been observed (first-observation-only, per R5: "cache verbatim").
    async fn maybe_cache_client_handshake(
        &self,
        line: &str,
    ) {
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
            },
            "notifications/initialized" => {
                let mut hs = self.handshake.lock().await;
                if hs.initialized_notif.is_none() {
                    hs.initialized_notif = Some(msg);
                }
            },
            _ => {},
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
                    },
                    None => {
                        if self.shutting_down.load(Ordering::SeqCst) {
                            return None;
                        }
                        eprintln!(
                            "[mcp-toolmon] child exited unexpectedly (not a triggered swap); attempting automatic recovery"
                        );
                        self.throttle_crash_recovery().await;
                        let synthesized =
                            self.swap_child_with_drain_ms(0).await;
                        if !synthesized.is_empty() {
                            let mut q = self.outgoing_queue.lock().await;
                            for err in synthesized {
                                if let Ok(s) = serde_json::to_string(&err) {
                                    q.push_back(s);
                                }
                            }
                        }
                        continue;
                    },
                },
                None => {
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                },
            }
        }
    }

    /// Bound automatic crash-triggered respawns (T6, item 6): more than
    /// `CRASH_THROTTLE_MAX` unexpected exits within `CRASH_THROTTLE_WINDOW_MS`
    /// makes each further automatic recovery in that window sleep
    /// `CRASH_THROTTLE_COOLDOWN_MS` before proceeding. Never gives up (R7).
    async fn throttle_crash_recovery(&self) {
        let should_cooldown = {
            let mut t = self.crash_throttle.lock().await;
            let now = tokio::time::Instant::now();
            if now.duration_since(t.window_start)
                > Duration::from_millis(CRASH_THROTTLE_WINDOW_MS)
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
            tokio::time::sleep(Duration::from_millis(
                CRASH_THROTTLE_COOLDOWN_MS,
            ))
            .await;
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

    /// If `line` is a response whose id matches the cached `initialize`
    /// request id, and no baseline has been captured yet, cache it as the
    /// original-child baseline for future divergence comparisons.
    async fn maybe_cache_original_response(
        &self,
        line: &str,
    ) {
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
    async fn replay_handshake(
        &self,
        handles: &ChildHandles,
    ) {
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
            eprintln!(
                "[mcp-toolmon] handshake replay: failed to write initialize to new child"
            );
            return;
        }
        match handles.read_line().await {
            Some(resp_line) =>
                match serde_json::from_str::<Value>(&resp_line) {
                    Ok(resp) => {
                        log_capability_divergence(
                            original_response.as_ref(),
                            &resp,
                        );
                        self.record_divergence(
                            original_response.as_ref(),
                            &resp,
                        )
                        .await;
                    },
                    Err(_) => {
                        eprintln!(
                            "[mcp-toolmon] handshake replay: new child's initialize response was not valid JSON"
                        );
                    },
                },
            None => {
                eprintln!(
                    "[mcp-toolmon] handshake replay: new child closed stdout before responding to replayed initialize"
                );
            },
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
    async fn record_divergence(
        &self,
        original: Option<&Value>,
        new_response: &Value,
    ) {
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
    pub async fn swap_child_with_drain_ms(
        &self,
        drain_ms: u64,
    ) -> Vec<Value> {
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
                    tokio::time::sleep(Duration::from_millis(
                        LIVENESS_CHECK_MS,
                    ))
                    .await;
                    if !handles.has_exited().await {
                        self.replay_handshake(&handles).await;
                        self.adopt_healthy(handles).await;
                        return synthesized;
                    }
                    eprintln!(
                        "[mcp-toolmon] respawned child exited immediately (attempt {attempt}); retrying"
                    );
                },
                Err(e) => {
                    eprintln!(
                        "[mcp-toolmon] respawn attempt {attempt} failed to spawn: {e}"
                    );
                },
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
                },
                Err(e) => {
                    eprintln!(
                        "[mcp-toolmon] last-known-good fallback spawn failed too ({e}); no healthy child until the next swap"
                    );
                },
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

    async fn adopt_healthy(
        &self,
        handles: ChildHandles,
    ) {
        if let Some(p) = &handles.shadow_path {
            if let Some(dir) = p.parent() {
                self.shadow_dirs.lock().await.push(dir.to_path_buf());
            }
            if let Ok(snapshot) = snapshot_last_known_good(p, &self.shadow_root)
            {
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

    async fn cleanup_shadow_dirs(&self) {
        let dirs = { std::mem::take(&mut *self.shadow_dirs.lock().await) };
        for dir in dirs {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
