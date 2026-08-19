//! Binary watcher (T6): debounced mtime/size + content-hash polling of the
//! canonical child binary path, triggering `Supervisor::swap_child()` on a
//! confirmed change and notifying the client via
//! `notifications/tools/list_changed` (R9) afterward.
//!
//! Debounce logic (R11): a change is acted on only once the canonical
//! path's `(mtime, size)` has been observed stable across two consecutive
//! polls, AND a scratch copy of it can be read successfully, AND that
//! scratch copy's content hash differs from the currently-running shadow's
//! hash. A copy failure (binary mid-write) is treated as "not yet changed"
//! and retried on the next poll, never as an error.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{
        Hash,
        Hasher,
    },
    path::{
        Path,
        PathBuf,
    },
    sync::Arc,
    time::{
        Duration,
        SystemTime,
    },
};

use tokio::sync::mpsc::UnboundedSender;

use crate::supervisor::Supervisor;

/// Default poll interval (ms), used when `TOOLMON_POLL_MS` is unset/unparseable.
pub const DEFAULT_POLL_MS: u64 = 1000;

/// Watcher on/off + interval, resolved from env (`TOOLMON_RELOAD`,
/// `TOOLMON_POLL_MS`).
#[derive(Debug, Clone, Copy)]
pub struct WatcherConfig {
    pub enabled: bool,
    pub poll_ms: u64,
}

impl WatcherConfig {
    /// `TOOLMON_RELOAD` defaults to enabled; only an explicit `0` or
    /// `false` (case-insensitive, surrounding whitespace ignored) disables
    /// it — any other value, including unset or garbage, is treated
    /// leniently as "on" rather than rejected. `TOOLMON_POLL_MS` defaults
    /// to [`DEFAULT_POLL_MS`]; unset, unparseable, or non-positive values
    /// fall back to the default rather than erroring.
    pub fn from_env() -> Self {
        let enabled = std::env::var("TOOLMON_RELOAD")
            .ok()
            .map(|v| !is_falsy(&v))
            .unwrap_or(true);
        let poll_ms = std::env::var("TOOLMON_POLL_MS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_POLL_MS);
        Self { enabled, poll_ms }
    }
}

fn is_falsy(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false")
}

fn stat(path: &Path) -> std::io::Result<(SystemTime, u64)> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.modified()?, meta.len()))
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Copy `canonical` into a scratch file under `scratch_dir` distinct from
/// the live shadow-copy path, read it back, and delete the scratch copy —
/// used only to obtain hashable bytes for a stability-confirmed candidate
/// without ever touching the shadow file the currently-running child has
/// open. `shadow::make_shadow_copy` reuses the SAME destination path across
/// calls for a given canonical path + this process's pid (see its doc), so
/// calling it here — before the old child is killed — would corrupt the
/// file the live child is still executing from. The scratch copy used for
/// hashing is deliberately a completely separate path.
fn copy_for_hash(
    canonical: &Path,
    scratch_dir: &Path,
) -> std::io::Result<Vec<u8>> {
    std::fs::create_dir_all(scratch_dir)?;
    let dest = scratch_dir.join(format!("candidate-{}", std::process::id()));
    std::fs::copy(canonical, &dest)?;
    let bytes = std::fs::read(&dest);
    let _ = std::fs::remove_file(&dest);
    bytes
}

/// Pure debounce state machine over `(mtime, size)` observations (R11:
/// "stable across two consecutive polls"). No I/O, no timing — fully unit
/// testable with injected metadata sequences.
#[derive(Debug, Default)]
struct Debouncer {
    baseline: Option<(SystemTime, u64)>,
    pending: Option<(SystemTime, u64)>,
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// Matches the already-accounted-for baseline; nothing to do.
    Steady,
    /// Differs from baseline but not yet confirmed stable (first
    /// observation, or it changed again before stabilizing).
    Candidate,
    /// Stable across two consecutive polls; caller should attempt the
    /// copy+hash step next.
    Stable,
}

impl Debouncer {
    fn observe(
        &mut self,
        current: (SystemTime, u64),
    ) -> Step {
        if Some(current) == self.baseline {
            self.pending = None;
            return Step::Steady;
        }
        if self.pending == Some(current) {
            return Step::Stable;
        }
        self.pending = Some(current);
        Step::Candidate
    }

    fn set_baseline(
        &mut self,
        meta: (SystemTime, u64),
    ) {
        self.baseline = Some(meta);
        self.pending = None;
    }

    /// Re-affirm `meta` as still-pending without treating it as a fresh
    /// candidate — used when the copy attempt fails after stability was
    /// already confirmed (R11: retry, don't restart the stability count).
    fn reaffirm_pending(
        &mut self,
        meta: (SystemTime, u64),
    ) {
        self.pending = Some(meta);
    }
}

/// Outcome of one poll iteration.
#[derive(Debug, PartialEq, Eq)]
pub enum PollResult {
    /// Nothing to do (steady, or not yet stable).
    NoAction,
    /// Stable, but the copy attempt failed (binary mid-write) — R11:
    /// retried next poll, never surfaced as an error.
    CopyFailed,
    /// Stable and copy succeeded, but the content hash matches the
    /// currently-running shadow — a false alarm (e.g. a touch without a
    /// content change). Baseline advanced so this isn't re-checked.
    SameContent,
    /// Stable, copy succeeded, hash differs from the currently-running
    /// shadow — confirmed change. Caller must trigger the swap.
    Changed,
}

/// Polls one canonical binary path, applying the R11 debounce/hash
/// pipeline described in the module doc.
pub struct Watcher {
    canonical: PathBuf,
    debouncer: Debouncer,
}

impl Watcher {
    pub fn new(canonical: PathBuf) -> Self {
        Self {
            canonical,
            debouncer: Debouncer::default(),
        }
    }

    /// Seed the baseline from the canonical path's CURRENT metadata so the
    /// binary that's already running at startup is never treated as "new".
    /// Best-effort: if the stat fails, the first real poll's Candidate step
    /// simply establishes the baseline organically.
    pub fn seed_baseline(&mut self) {
        if let Ok(meta) = stat(&self.canonical) {
            self.debouncer.set_baseline(meta);
        }
    }

    /// One poll iteration against injected stat/copy/current-hash sources —
    /// the fully generic seam unit tests drive directly with fake
    /// sequences, with no real filesystem timing involved.
    fn poll_once_with(
        &mut self,
        stat_fn: impl FnOnce() -> std::io::Result<(SystemTime, u64)>,
        copy_fn: impl FnOnce() -> std::io::Result<Vec<u8>>,
        current_hash_fn: impl FnOnce() -> Option<u64>,
    ) -> PollResult {
        let current = match stat_fn() {
            Ok(m) => m,
            Err(_) => return PollResult::NoAction,
        };
        match self.debouncer.observe(current) {
            Step::Steady => PollResult::NoAction,
            Step::Candidate => PollResult::NoAction,
            Step::Stable => match copy_fn() {
                Err(_) => {
                    self.debouncer.reaffirm_pending(current);
                    PollResult::CopyFailed
                },
                Ok(bytes) => {
                    let new_hash = hash_bytes(&bytes);
                    self.debouncer.set_baseline(current);
                    if current_hash_fn() == Some(new_hash) {
                        PollResult::SameContent
                    } else {
                        PollResult::Changed
                    }
                },
            },
        }
    }

    /// Real production poll: stat the canonical path for real, copy it into
    /// a throwaway scratch file under `shadow_root` for hashing (never the
    /// live shadow path — see [`copy_for_hash`]), and compare against the
    /// hash of the supervisor's currently-running shadow file.
    pub fn poll_real(
        &mut self,
        supervisor: &Supervisor,
        shadow_root: &Path,
    ) -> PollResult {
        let canonical = self.canonical.clone();
        let scratch_dir = shadow_root.join("watcher-candidates");
        let current_shadow = supervisor.shadow_path();
        self.poll_once_with(
            || stat(&canonical),
            || copy_for_hash(&canonical, &scratch_dir),
            || {
                current_shadow
                    .as_deref()
                    .and_then(|p| std::fs::read(p).ok())
                    .map(|b| hash_bytes(&b))
            },
        )
    }
}

/// Spawn the polling task if `config.enabled`. On a confirmed change,
/// triggers `supervisor.swap_child()`, forwards any synthesized
/// reload-interruption error responses (R6) and the
/// `notifications/tools/list_changed` notification (R9) to `client_tx` —
/// the caller (main.rs) is responsible for actually writing lines received
/// on the paired receiver to the client's stdout.
pub fn spawn(
    supervisor: Arc<Supervisor>,
    canonical: PathBuf,
    shadow_root: PathBuf,
    client_tx: UnboundedSender<String>,
    config: WatcherConfig,
) -> Option<tokio::task::JoinHandle<()>> {
    if !config.enabled {
        return None;
    }
    // Seeded synchronously, before the task is scheduled, so the baseline
    // is guaranteed to reflect the binary as of THIS call — not whatever
    // happens to be on disk whenever the tokio scheduler gets around to
    // running the spawned task (which could otherwise race a caller that
    // overwrites the binary immediately after calling `spawn`).
    let mut watcher = Watcher::new(canonical);
    watcher.seed_baseline();
    Some(tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(config.poll_ms)).await;
            if watcher.poll_real(&supervisor, &shadow_root)
                == PollResult::Changed
            {
                let synthesized = supervisor.swap_child().await;
                for err in synthesized {
                    if let Ok(line) = serde_json::to_string(&err) {
                        let _ = client_tx.send(line);
                    }
                }
                let notif = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/tools/list_changed"
                });
                let _ = client_tx.send(notif.to_string());
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// R11: a slow/partial write — metadata changes on every poll for a
    /// while before finally settling — must produce exactly one confirmed
    /// change, not zero, not many. Drives `poll_once_with` directly with an
    /// injected metadata sequence; no real filesystem timing involved.
    #[test]
    fn watcher_debounces_partial_write() {
        let mut watcher = Watcher::new(PathBuf::from("/unused"));
        watcher.debouncer.set_baseline((t(0), 100));

        let sequence = [
            (t(1), 10),  // partial write, growing
            (t(2), 55),  // still growing (differs from previous candidate)
            (t(3), 120), // still growing
            (t(3), 120), // stable! same as previous poll -> attempt copy/hash
            (t(3), 120), // already accounted for (baseline updated) -> steady
        ];

        let mut changed_count = 0;
        let mut stability_attempts = 0;
        for meta in sequence {
            let result = watcher.poll_once_with(
                || Ok(meta),
                || Ok(b"new-content".to_vec()),
                || Some(hash_bytes(b"old-content")),
            );
            if result == PollResult::Changed {
                changed_count += 1;
            }
            if matches!(
                result,
                PollResult::Changed
                    | PollResult::SameContent
                    | PollResult::CopyFailed
            ) {
                stability_attempts += 1;
            }
        }

        assert_eq!(
            changed_count, 1,
            "a partial write settling must trigger exactly one confirmed change"
        );
        assert_eq!(
            stability_attempts, 1,
            "the copy/hash step must only be attempted once the metadata is stable"
        );
    }

    /// R11: a copy failure (binary mid-write even though metadata looked
    /// stable) must be retried on the next poll, not surfaced as an error,
    /// and must not require re-establishing stability from scratch.
    #[test]
    fn watcher_retries_after_copy_failure_without_resetting_stability() {
        let mut watcher = Watcher::new(PathBuf::from("/unused"));
        watcher.debouncer.set_baseline((t(0), 100));

        let stable_meta = (t(1), 55);
        // First poll observes the candidate.
        let r1 =
            watcher.poll_once_with(|| Ok(stable_meta), || Ok(vec![]), || None);
        assert_eq!(r1, PollResult::NoAction);
        // Second poll: stable, but copy fails (mid-write).
        let r2 = watcher.poll_once_with(
            || Ok(stable_meta),
            || Err(std::io::Error::other("mid-write")),
            || None,
        );
        assert_eq!(r2, PollResult::CopyFailed);
        // Third poll: same metadata still, copy now succeeds -> confirmed
        // change, without needing a fresh two-poll stability window.
        let r3 = watcher.poll_once_with(
            || Ok(stable_meta),
            || Ok(b"content".to_vec()),
            || Some(hash_bytes(b"different")),
        );
        assert_eq!(r3, PollResult::Changed);
    }

    #[test]
    fn watcher_config_env_parsing_leniency() {
        assert!(!is_falsy("1"));
        assert!(is_falsy("0"));
        assert!(is_falsy("FALSE"));
        assert!(is_falsy(" false "));
        assert!(!is_falsy("no"));
        assert!(!is_falsy(""));
    }
}
