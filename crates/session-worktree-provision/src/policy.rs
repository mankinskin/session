use std::{
    collections::BTreeSet,
    fs,
    path::{
        Path,
        PathBuf,
    },
    time::{
        Duration,
        SystemTime,
    },
};

use serde_json::Value;
use thiserror::Error;
use time::{
    OffsetDateTime,
    format_description::well_known::Rfc3339,
};

use crate::{
    WorktreeGit,
    WorktreeGitError,
    WorktreeRef,
};

const DEFAULT_MAX_WORKTREES: usize = 8;
const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(4 * 60 * 60);
const DEFAULT_IDLE_BEFORE_RECLAIM: Duration = Duration::from_secs(24 * 60 * 60);

/// Determines whether a live session currently owns a worktree.
pub trait SessionActivity {
    /// True when some live session currently owns this worktree.
    fn is_active(
        &self,
        worktree: &Path,
    ) -> bool;

    fn worktree_ownership(
        &self,
        _worktree: &Path,
    ) -> WorktreeOwnership {
        WorktreeOwnership::Unowned
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeOwnership {
    Unowned,
    Owned(String),
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeLayout {
    Nested { session_id: String, slug: String },
    LegacyFlat { short_id: String, slug: String },
}

/// Session activity backed by records in a `.session` store.
pub struct SessionStoreActivity {
    session_store: PathBuf,
    stale_after: Duration,
}

impl SessionStoreActivity {
    pub fn new(
        session_store: impl Into<PathBuf>,
        stale_after: Duration,
    ) -> Self {
        Self {
            session_store: session_store.into(),
            stale_after,
        }
    }

    pub fn with_default_staleness(session_store: impl Into<PathBuf>) -> Self {
        Self::new(session_store, DEFAULT_STALE_AFTER)
    }

    fn session_stores(&self) -> Result<Vec<PathBuf>, std::io::Error> {
        let mut stores = BTreeSet::from([self.session_store.clone()]);
        let Some(main_checkout) = self.session_store.parent() else {
            return Ok(stores.into_iter().collect());
        };
        let worktree_root = main_checkout.join(".worktrees");
        let entries = match fs::read_dir(&worktree_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(stores.into_iter().collect());
            },
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let store = path.join(".session");
            if store.is_dir() {
                stores.insert(store);
                continue;
            }
            for nested in fs::read_dir(path)? {
                let nested = nested?;
                let store = nested.path().join(".session");
                if store.is_dir() {
                    stores.insert(store);
                }
            }
        }
        Ok(stores.into_iter().collect())
    }
}

impl SessionActivity for SessionStoreActivity {
    fn is_active(
        &self,
        worktree: &Path,
    ) -> bool {
        let Some(worktree) = normalized_path(worktree) else {
            return false;
        };
        let Ok(stores) = self.session_stores() else {
            return true;
        };
        stores.into_iter().any(|store| {
            match fs::read_dir(store.join("sessions")) {
                Ok(entries) => entries.flatten().any(|entry| {
                    session_record_is_active(
                        &entry.path().join("session.json"),
                        &worktree,
                        self.stale_after,
                    )
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound =>
                    false,
                Err(_) => true,
            }
        })
    }

    fn worktree_ownership(
        &self,
        worktree: &Path,
    ) -> WorktreeOwnership {
        let Some(worktree) = normalized_path(worktree) else {
            return WorktreeOwnership::Ambiguous;
        };
        let Ok(stores) = self.session_stores() else {
            return WorktreeOwnership::Ambiguous;
        };
        let mut unreadable_store = false;
        let owners = stores
            .into_iter()
            .filter_map(|store| match fs::read_dir(store.join("sessions")) {
                Ok(entries) => Some(entries),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound =>
                    None,
                Err(_) => {
                    unreadable_store = true;
                    None
                },
            })
            .flat_map(|entries| entries.flatten())
            .filter_map(|entry| {
                session_record_owner(
                    &entry.path().join("session.json"),
                    &worktree,
                )
            })
            .collect::<BTreeSet<_>>();
        if unreadable_store {
            return WorktreeOwnership::Ambiguous;
        }
        match owners.len() {
            0 => WorktreeOwnership::Unowned,
            1 => WorktreeOwnership::Owned(owners.into_iter().next().unwrap()),
            _ => WorktreeOwnership::Ambiguous,
        }
    }
}

/// Test activity implementation that never marks a worktree active.
#[derive(Debug, Default)]
pub struct NeverActive;

impl SessionActivity for NeverActive {
    fn is_active(
        &self,
        _worktree: &Path,
    ) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionPolicy {
    pub max_worktrees: usize,
    pub stale_after: Duration,
    pub idle_before_reclaim: Duration,
    pub base_ref: String,
}

impl Default for ProvisionPolicy {
    fn default() -> Self {
        Self {
            max_worktrees: env_usize("WORKTREE_MAX")
                .unwrap_or(DEFAULT_MAX_WORKTREES),
            stale_after: Duration::from_secs(
                env_u64("WORKTREE_STALE_SECS")
                    .unwrap_or(DEFAULT_STALE_AFTER.as_secs()),
            ),
            idle_before_reclaim: Duration::from_secs(
                env_u64("WORKTREE_IDLE_SECS")
                    .unwrap_or(DEFAULT_IDLE_BEFORE_RECLAIM.as_secs()),
            ),
            base_ref: "main".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionOutcome {
    AlreadyProvisioned(WorktreeRef),
    Reclaimed {
        worktree: WorktreeRef,
        previous_name: String,
    },
    Created(WorktreeRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReclaimEligibility {
    Reclaimable,
    Rejected(ReclaimRejectionReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReclaimRejectionReason {
    OutsideWorktreeRoot,
    SessionActive,
    Detached,
    Dirty,
    ContainsCurrentDirectory,
    NotIdle,
    DirtySubmodule { path: PathBuf },
    AheadOfMain,
}

#[derive(Debug, Error)]
pub enum ProvisionError {
    #[error(transparent)]
    Git(#[from] WorktreeGitError),
    #[error(
        "worktree cap {max_worktrees} reached with {current_count} registered worktrees; no reclaimable candidate: {reason}"
    )]
    CapReached {
        max_worktrees: usize,
        current_count: usize,
        reason: String,
    },
    #[error(
        "worktree {} is owned by session {owner_session_id}, not requesting session {session_id}",
        worktree.display()
    )]
    SessionOwnershipConflict {
        worktree: PathBuf,
        session_id: String,
        owner_session_id: String,
    },
    #[error(
        "worktree {} has ambiguous recorded session ownership",
        worktree.display()
    )]
    AmbiguousSessionWorktreeOwnership { worktree: PathBuf },
    #[error(
        "session {session_id} has ambiguous worktree candidates: {candidates:?}"
    )]
    AmbiguousSessionWorktree {
        session_id: String,
        candidates: Vec<PathBuf>,
    },
}

pub fn evaluate_reclaim_candidate(
    git: &WorktreeGit,
    activity: &dyn SessionActivity,
    worktree: &WorktreeRef,
    policy: &ProvisionPolicy,
) -> Result<ReclaimEligibility, WorktreeGitError> {
    let root = git.main_checkout().join(".worktrees");
    if !is_discoverable_worktree_path(&root, &worktree.path) {
        return Ok(ReclaimEligibility::Rejected(
            ReclaimRejectionReason::OutsideWorktreeRoot,
        ));
    }
    if activity.is_active(&worktree.path) {
        return Ok(ReclaimEligibility::Rejected(
            ReclaimRejectionReason::SessionActive,
        ));
    }
    if worktree.branch.is_none() {
        return Ok(ReclaimEligibility::Rejected(
            ReclaimRejectionReason::Detached,
        ));
    }
    if current_directory_is_within_worktree(&worktree.path) {
        return Ok(ReclaimEligibility::Rejected(
            ReclaimRejectionReason::ContainsCurrentDirectory,
        ));
    }
    if !worktree_is_idle(git, worktree, policy.idle_before_reclaim) {
        return Ok(ReclaimEligibility::Rejected(
            ReclaimRejectionReason::NotIdle,
        ));
    }
    for submodule in git.submodule_paths()? {
        let path = worktree.path.join(&submodule);
        if path.exists() && git.is_dirty(&path)? {
            return Ok(ReclaimEligibility::Rejected(
                ReclaimRejectionReason::DirtySubmodule {
                    path: PathBuf::from(submodule),
                },
            ));
        }
    }
    if git.is_dirty(&worktree.path)? {
        return Ok(ReclaimEligibility::Rejected(ReclaimRejectionReason::Dirty));
    }
    if git.ahead_behind(&worktree.path, "main")?.0 != 0 {
        return Ok(ReclaimEligibility::Rejected(
            ReclaimRejectionReason::AheadOfMain,
        ));
    }
    Ok(ReclaimEligibility::Reclaimable)
}

pub fn reclaim_candidates(
    git: &WorktreeGit,
    activity: &dyn SessionActivity,
    policy: &ProvisionPolicy,
) -> Result<Vec<WorktreeRef>, WorktreeGitError> {
    let worktrees = registered_worktrees(git)?;
    reclaim_candidates_from_registered(git, activity, &worktrees, policy)
}

pub fn provision_for_session(
    git: &WorktreeGit,
    activity: &dyn SessionActivity,
    session_id: &str,
    policy: &ProvisionPolicy,
) -> Result<ProvisionOutcome, ProvisionError> {
    let short_id = session_short_id(session_id);
    let relative_path = PathBuf::from(session_id).join("session");
    let branch = format!("agent/{session_id}/session");
    let worktrees = registered_worktrees(git)?;
    let nested = worktrees
        .iter()
        .filter(|worktree| {
            matches!(
                worktree_layout(&git.main_checkout().join(".worktrees"), &worktree.path),
                Some(WorktreeLayout::Nested { session_id: candidate, .. }) if candidate == session_id
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    if nested.len() > 1 {
        return Err(ambiguous_session_worktree(session_id, nested));
    }
    if let Some(worktree) = nested.first() {
        return reuse_worktree(activity, session_id, worktree);
    }

    let prefix = format!("{short_id}-");
    let legacy = worktrees
        .iter()
        .filter(|worktree| {
            matches!(
                worktree_layout(&git.main_checkout().join(".worktrees"), &worktree.path),
                Some(WorktreeLayout::LegacyFlat { short_id: candidate, .. }) if candidate == short_id
            ) && worktree.name.starts_with(&prefix)
        })
        .cloned()
        .collect::<Vec<_>>();
    if legacy.len() > 1 {
        return Err(ambiguous_session_worktree(session_id, legacy));
    }
    if let Some(worktree) = legacy.first() {
        return reuse_worktree(activity, session_id, worktree);
    }

    for candidate in
        reclaim_candidates_from_registered(git, activity, &worktrees, policy)?
    {
        let previous_name = candidate.name.clone();
        // TODO(5e6cf4f8): update reclaimed worktrees from main in a later provisioning unit.
        match git.rename_worktree(&previous_name, &relative_path, &branch) {
            Ok(worktree) => {
                return Ok(ProvisionOutcome::Reclaimed {
                    worktree,
                    previous_name,
                });
            },
            Err(error) => eprintln!(
                "worktree reclaim failed for {}: {error}; trying another candidate or creating a fresh worktree",
                candidate.path.display()
            ),
        }
    }

    if worktrees.len() >= policy.max_worktrees {
        return Err(ProvisionError::CapReached {
            max_worktrees: policy.max_worktrees,
            current_count: worktrees.len(),
            reason: "all registered worktrees are active, dirty, ahead of main, detached, or outside .worktrees".to_string(),
        });
    }

    Ok(ProvisionOutcome::Created(git.create_worktree_at(
        &relative_path,
        &branch,
        &policy.base_ref,
    )?))
}

fn registered_worktrees(
    git: &WorktreeGit
) -> Result<Vec<WorktreeRef>, WorktreeGitError> {
    let root = git.main_checkout().join(".worktrees");
    Ok(git
        .list_worktrees()?
        .into_iter()
        .filter(|worktree| is_discoverable_worktree_path(&root, &worktree.path))
        .collect())
}

fn reuse_worktree(
    activity: &dyn SessionActivity,
    session_id: &str,
    worktree: &WorktreeRef,
) -> Result<ProvisionOutcome, ProvisionError> {
    match activity.worktree_ownership(&worktree.path) {
        WorktreeOwnership::Owned(owner_session_id)
            if owner_session_id == session_id =>
            Ok(ProvisionOutcome::AlreadyProvisioned(worktree.clone())),
        WorktreeOwnership::Owned(owner_session_id) =>
            Err(ProvisionError::SessionOwnershipConflict {
                worktree: worktree.path.clone(),
                session_id: session_id.to_string(),
                owner_session_id,
            }),
        WorktreeOwnership::Ambiguous =>
            Err(ProvisionError::AmbiguousSessionWorktreeOwnership {
                worktree: worktree.path.clone(),
            }),
        WorktreeOwnership::Unowned =>
            Ok(ProvisionOutcome::AlreadyProvisioned(worktree.clone())),
    }
}

fn ambiguous_session_worktree(
    session_id: &str,
    mut worktrees: Vec<WorktreeRef>,
) -> ProvisionError {
    worktrees.sort_by(|left, right| left.path.cmp(&right.path));
    ProvisionError::AmbiguousSessionWorktree {
        session_id: session_id.to_string(),
        candidates: worktrees
            .into_iter()
            .map(|worktree| worktree.path)
            .collect(),
    }
}

fn is_discoverable_worktree_path(
    root: &Path,
    path: &Path,
) -> bool {
    path.parent() == Some(root)
        || path.parent().and_then(Path::parent) == Some(root)
}

fn worktree_layout(
    root: &Path,
    path: &Path,
) -> Option<WorktreeLayout> {
    let relative = path.strip_prefix(root).ok()?;
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    match components.as_slice() {
        [name] => {
            let (short_id, slug) = name.split_once('-')?;
            (short_id.len() == 8 && !slug.is_empty()).then(|| {
                WorktreeLayout::LegacyFlat {
                    short_id: (*short_id).to_string(),
                    slug: (*slug).to_string(),
                }
            })
        },
        [session_id, slug] if !session_id.is_empty() && !slug.is_empty() =>
            Some(WorktreeLayout::Nested {
                session_id: (*session_id).to_string(),
                slug: (*slug).to_string(),
            }),
        _ => None,
    }
}

fn reclaim_candidates_from_registered(
    git: &WorktreeGit,
    activity: &dyn SessionActivity,
    worktrees: &[WorktreeRef],
    policy: &ProvisionPolicy,
) -> Result<Vec<WorktreeRef>, WorktreeGitError> {
    let mut candidates = Vec::new();
    for worktree in worktrees {
        if evaluate_reclaim_candidate(git, activity, worktree, policy)?
            == ReclaimEligibility::Reclaimable
        {
            candidates.push(worktree.clone());
        }
    }
    candidates.sort_by(reclaim_order);
    Ok(candidates)
}

fn current_directory_is_within_worktree(worktree: &Path) -> bool {
    std::env::current_dir()
        .ok()
        .is_some_and(|current_dir| path_is_within(&current_dir, worktree))
}

fn path_is_within(
    path: &Path,
    worktree: &Path,
) -> bool {
    let Some(path) = normalized_path(path) else {
        return false;
    };
    let Some(worktree) = normalized_path(worktree) else {
        return false;
    };
    path == worktree
        || path
            .strip_prefix(&worktree)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn worktree_is_idle(
    git: &WorktreeGit,
    worktree: &WorktreeRef,
    idle_before_reclaim: Duration,
) -> bool {
    let Some(last_activity) = worktree_last_activity(git, worktree) else {
        return false;
    };
    let Some(cutoff) = SystemTime::now().checked_sub(idle_before_reclaim)
    else {
        return false;
    };
    last_activity < cutoff
}

fn worktree_last_activity(
    git: &WorktreeGit,
    worktree: &WorktreeRef,
) -> Option<SystemTime> {
    // Git updates the linked-worktree admin directory and index for Git activity;
    // the worktree root tracks agent filesystem writes without scanning its tree.
    // Requiring all three cheap signals makes missing metadata fail closed.
    let admin = git
        .main_checkout()
        .join(".git/worktrees")
        .join(&worktree.name);
    [
        worktree.path.as_path(),
        admin.as_path(),
        admin.join("index").as_path(),
    ]
    .into_iter()
    .map(|path| fs::metadata(path).ok()?.modified().ok())
    .collect::<Option<Vec<_>>>()?
    .into_iter()
    .max()
}

fn reclaim_order(
    left: &WorktreeRef,
    right: &WorktreeRef,
) -> std::cmp::Ordering {
    modified_at(&left.path)
        .cmp(&modified_at(&right.path))
        .then_with(|| left.name.cmp(&right.name))
}

fn modified_at(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn session_short_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.parse().ok()
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.parse().ok()
}

fn session_record_is_active(
    path: &Path,
    worktree: &str,
    stale_after: Duration,
) -> bool {
    let Ok(record) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(record) = serde_json::from_str::<Value>(&record) else {
        return false;
    };
    let record_path = record
        .pointer("/metadata/worktree/path")
        .and_then(Value::as_str)
        .and_then(normalized_path);
    let timestamp = record
        .get("captured_at")
        .or_else(|| record.get("started_at"))
        .and_then(Value::as_str)
        .and_then(parse_timestamp);
    record_path.as_deref() == Some(worktree)
        && timestamp
            .is_some_and(|timestamp| timestamp_is_fresh(timestamp, stale_after))
}

fn session_record_owner(
    path: &Path,
    worktree: &str,
) -> Option<String> {
    let record = fs::read_to_string(path).ok()?;
    let record = serde_json::from_str::<Value>(&record).ok()?;
    let record_path = record
        .pointer("/metadata/worktree/path")
        .and_then(Value::as_str)
        .and_then(normalized_path)?;
    let session_id = record.get("session_id")?.as_str()?;
    (record_path == worktree && !session_id.is_empty())
        .then(|| session_id.to_string())
}

fn normalized_path(path: impl AsRef<Path>) -> Option<String> {
    fs::canonicalize(path)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/").to_lowercase())
}

fn parse_timestamp(timestamp: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(timestamp, &Rfc3339).ok()
}

fn timestamp_is_fresh(
    timestamp: OffsetDateTime,
    stale_after: Duration,
) -> bool {
    let now = OffsetDateTime::now_utc();
    if timestamp > now {
        return true;
    }
    (now - timestamp).unsigned_abs() <= stale_after
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{
            Path,
            PathBuf,
        },
        process::Command,
        time::{
            Duration,
            SystemTime,
        },
    };

    use filetime::{
        FileTime,
        set_file_mtime,
    };
    use time::{
        OffsetDateTime,
        format_description::well_known::Rfc3339,
    };

    use super::{
        NeverActive,
        ProvisionError,
        ProvisionOutcome,
        ProvisionPolicy,
        ReclaimEligibility,
        ReclaimRejectionReason,
        SessionActivity,
        SessionStoreActivity,
        evaluate_reclaim_candidate,
        provision_for_session,
        reclaim_candidates,
    };
    use crate::{
        WorktreeRef,
        tests::Fixture,
    };

    const SESSION_ID: &str = "12345678-1234-4234-8234-123456789abc";
    const SAME_PREFIX_SESSION_ID: &str = "12345678-5678-4678-9678-123456789abc";

    struct ActiveWorktree(PathBuf);

    impl SessionActivity for ActiveWorktree {
        fn is_active(
            &self,
            worktree: &Path,
        ) -> bool {
            worktree == self.0
        }
    }

    fn policy(max_worktrees: usize) -> ProvisionPolicy {
        ProvisionPolicy {
            max_worktrees,
            stale_after: Duration::from_secs(60),
            idle_before_reclaim: Duration::ZERO,
            base_ref: "main".to_string(),
        }
    }

    fn backdate_activity_signals(
        git: &crate::WorktreeGit,
        worktree: &WorktreeRef,
        age: Duration,
    ) {
        let timestamp = FileTime::from_system_time(SystemTime::now() - age);
        let admin = git
            .main_checkout()
            .join(".git/worktrees")
            .join(&worktree.name);
        for path in [
            worktree.path.as_path(),
            admin.as_path(),
            admin.join("index").as_path(),
        ] {
            set_file_mtime(path, timestamp).unwrap();
        }
    }

    fn commit(
        directory: &Path,
        message: &str,
    ) {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-am",
                message,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn assert_cap_reached(result: Result<ProvisionOutcome, ProvisionError>) {
        assert!(matches!(result, Err(ProvisionError::CapReached { .. })));
    }

    fn persist_worktree_owner(
        session_store: &Path,
        session_id: &str,
        worktree: &WorktreeRef,
    ) {
        let record = session_store
            .join("sessions")
            .join(session_id)
            .join("session.json");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        fs::write(
            record,
            serde_json::json!({
                "session_id": session_id,
                "metadata": {
                    "worktree": {
                        "path": worktree.path,
                    },
                },
            })
            .to_string(),
        )
        .unwrap();
    }

    fn persist_active_worktree_session(
        session_store: &Path,
        session_id: &str,
        worktree: &WorktreeRef,
    ) {
        let record = session_store
            .join("sessions")
            .join(session_id)
            .join("session.json");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        fs::write(
            record,
            serde_json::json!({
                "session_id": session_id,
                "captured_at": OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .unwrap(),
                "metadata": {
                    "worktree": {
                        "path": worktree.path,
                    },
                },
            })
            .to_string(),
        )
        .unwrap();
    }

    fn only_worktree(git: &crate::WorktreeGit) -> WorktreeRef {
        let mut worktrees = git.list_worktrees().unwrap();
        assert_eq!(worktrees.len(), 1);
        worktrees.remove(0)
    }

    #[test]
    fn evaluate_reclaim_candidate_reports_outside_worktree_root() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let candidate = WorktreeRef {
            name: "outside".to_string(),
            path: fixture.main.clone(),
            branch: Some("main".to_string()),
        };

        let eligibility = evaluate_reclaim_candidate(
            &git,
            &NeverActive,
            &candidate,
            &policy(1),
        )
        .unwrap();
        assert_eq!(
            eligibility,
            ReclaimEligibility::Rejected(
                ReclaimRejectionReason::OutsideWorktreeRoot
            )
        );
    }

    #[test]
    fn evaluate_reclaim_candidate_reports_session_active() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
        let activity = ActiveWorktree(worktree.path.clone());

        let eligibility = evaluate_reclaim_candidate(
            &git,
            &activity,
            &only_worktree(&git),
            &policy(1),
        )
        .unwrap();
        assert_eq!(
            eligibility,
            ReclaimEligibility::Rejected(ReclaimRejectionReason::SessionActive)
        );
    }

    #[test]
    fn federated_activity_and_unreadable_store_refuse_reclaim() {
        let active_fixture = Fixture::new();
        let active_git = active_fixture.git();
        let active_worktree = active_git
            .create_worktree("old", "agent/old", "main")
            .unwrap();
        let federated_store = active_fixture
            .main
            .join(".worktrees")
            .join("activity-session")
            .join("nested")
            .join(".session");
        persist_active_worktree_session(
            &federated_store,
            "session-visible-only-through-federation",
            &active_worktree,
        );
        let active_eligibility = evaluate_reclaim_candidate(
            &active_git,
            &SessionStoreActivity::new(
                active_fixture.main.join(".session"),
                Duration::from_secs(60),
            ),
            &only_worktree(&active_git),
            &policy(1),
        )
        .unwrap();
        assert_eq!(
            active_eligibility,
            ReclaimEligibility::Rejected(ReclaimRejectionReason::SessionActive)
        );

        let unreadable_fixture = Fixture::new();
        let unreadable_git = unreadable_fixture.git();
        unreadable_git
            .create_worktree("old", "agent/old", "main")
            .unwrap();
        let unreadable_sessions = unreadable_fixture
            .main
            .join(".worktrees")
            .join("unreadable-session")
            .join("nested")
            .join(".session")
            .join("sessions");
        fs::create_dir_all(unreadable_sessions.parent().unwrap()).unwrap();
        fs::write(
            &unreadable_sessions,
            "not a readable session directory
",
        )
        .unwrap();
        let unreadable_eligibility = evaluate_reclaim_candidate(
            &unreadable_git,
            &SessionStoreActivity::new(
                unreadable_fixture.main.join(".session"),
                Duration::from_secs(60),
            ),
            &only_worktree(&unreadable_git),
            &policy(1),
        )
        .unwrap();
        assert_eq!(
            unreadable_eligibility,
            ReclaimEligibility::Rejected(ReclaimRejectionReason::SessionActive)
        );
    }

    #[test]
    fn evaluate_reclaim_candidate_reports_detached() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
        let candidate = WorktreeRef {
            name: worktree.name,
            path: worktree.path,
            branch: None,
        };

        let eligibility = evaluate_reclaim_candidate(
            &git,
            &NeverActive,
            &candidate,
            &policy(1),
        )
        .unwrap();
        assert_eq!(
            eligibility,
            ReclaimEligibility::Rejected(ReclaimRejectionReason::Detached)
        );
    }

    #[test]
    fn evaluate_reclaim_candidate_reports_dirty() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
        fs::write(worktree.path.join("untracked.txt"), "dirty\n").unwrap();

        let eligibility = evaluate_reclaim_candidate(
            &git,
            &NeverActive,
            &only_worktree(&git),
            &policy(1),
        )
        .unwrap();
        assert_eq!(
            eligibility,
            ReclaimEligibility::Rejected(ReclaimRejectionReason::Dirty)
        );
    }

    #[test]
    fn evaluate_reclaim_candidate_reports_contains_current_directory() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(&worktree.path).unwrap();

        let eligibility = evaluate_reclaim_candidate(
            &git,
            &NeverActive,
            &only_worktree(&git),
            &policy(1),
        )
        .unwrap();

        std::env::set_current_dir(original).unwrap();
        assert_eq!(
            eligibility,
            ReclaimEligibility::Rejected(
                ReclaimRejectionReason::ContainsCurrentDirectory
            )
        );
    }

    #[test]
    fn evaluate_reclaim_candidate_reports_not_idle() {
        let fixture = Fixture::new();
        let git = fixture.git();
        git.create_worktree("old", "agent/old", "main").unwrap();
        let mut not_idle_policy = policy(1);
        not_idle_policy.idle_before_reclaim = Duration::from_secs(60 * 60);

        let eligibility = evaluate_reclaim_candidate(
            &git,
            &NeverActive,
            &only_worktree(&git),
            &not_idle_policy,
        )
        .unwrap();
        assert_eq!(
            eligibility,
            ReclaimEligibility::Rejected(ReclaimRejectionReason::NotIdle)
        );
    }

    #[test]
    fn evaluate_reclaim_candidate_reports_dirty_submodule() {
        let fixture = Fixture::new();
        fixture.add_submodule();
        let git = fixture.git();
        let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
        fs::write(
            worktree.path.join("nested").join("inner.txt"),
            "dirty submodule\n",
        )
        .unwrap();

        let eligibility = evaluate_reclaim_candidate(
            &git,
            &NeverActive,
            &only_worktree(&git),
            &policy(1),
        )
        .unwrap();
        assert_eq!(
            eligibility,
            ReclaimEligibility::Rejected(
                ReclaimRejectionReason::DirtySubmodule {
                    path: PathBuf::from("nested"),
                }
            )
        );
    }

    #[test]
    fn evaluate_reclaim_candidate_reports_ahead_of_main() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
        fs::write(worktree.path.join("tracked.txt"), "advance\n").unwrap();
        commit(&worktree.path, "advance");

        let eligibility = evaluate_reclaim_candidate(
            &git,
            &NeverActive,
            &only_worktree(&git),
            &policy(1),
        )
        .unwrap();
        assert_eq!(
            eligibility,
            ReclaimEligibility::Rejected(ReclaimRejectionReason::AheadOfMain)
        );
    }

    #[test]
    fn reclaim_candidates_are_ordered_by_oldest_mtime_then_name() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let old = git.create_worktree("a-old", "agent/a-old", "main").unwrap();
        let newer =
            git.create_worktree("b-new", "agent/b-new", "main").unwrap();
        let age = Duration::from_secs(2 * 60 * 60);
        backdate_activity_signals(&git, &old, age);

        let candidates =
            reclaim_candidates(&git, &NeverActive, &policy(8)).unwrap();
        let names = candidates
            .iter()
            .map(|worktree| worktree.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["a-old", "b-new"]);
        assert!(candidates.iter().any(|worktree| worktree.path == old.path));
        assert!(
            candidates
                .iter()
                .any(|worktree| worktree.path == newer.path)
        );
    }

    #[test]
    fn second_call_for_session_is_idempotent() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let policy = policy(8);
        let first =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy)
                .unwrap();
        let expected = match first {
            ProvisionOutcome::Created(worktree) => worktree,
            other => panic!("expected creation, got {other:?}"),
        };

        let second =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy)
                .unwrap();
        assert!(matches!(
            second,
            ProvisionOutcome::AlreadyProvisioned(worktree) if worktree == expected
        ));
        assert_eq!(git.list_worktrees().unwrap().len(), 1);
    }

    #[test]
    fn session_reuses_its_own_persisted_worktree_assignment() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let session_store = fixture.main.join(".session");
        let worktree = match provision_for_session(
            &git,
            &NeverActive,
            SESSION_ID,
            &policy(8),
        )
        .unwrap()
        {
            ProvisionOutcome::Created(worktree) => worktree,
            other => panic!("expected creation, got {other:?}"),
        };
        persist_worktree_owner(&session_store, SESSION_ID, &worktree);

        let fresh_activity =
            SessionStoreActivity::with_default_staleness(&session_store);
        let reuse = provision_for_session(
            &git,
            &fresh_activity,
            SESSION_ID,
            &policy(8),
        )
        .unwrap();

        assert!(matches!(
            reuse,
            ProvisionOutcome::AlreadyProvisioned(reused) if reused == worktree
        ));
    }

    #[test]
    fn sessions_sharing_prefix_do_not_receive_the_same_worktree() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let session_store = fixture.main.join(".session");
        let worktree = git
            .create_worktree(
                "12345678-session",
                "agent/12345678-session",
                "main",
            )
            .unwrap();
        persist_worktree_owner(&session_store, SESSION_ID, &worktree);

        let fresh_activity =
            SessionStoreActivity::with_default_staleness(&session_store);
        let result = provision_for_session(
            &git,
            &fresh_activity,
            SAME_PREFIX_SESSION_ID,
            &policy(8),
        );

        assert!(matches!(
            result,
            Err(ProvisionError::SessionOwnershipConflict {
                session_id,
                owner_session_id,
                ..
            }) if session_id == SAME_PREFIX_SESSION_ID
                && owner_session_id == SESSION_ID
        ));
        assert_eq!(git.list_worktrees().unwrap(), vec![worktree]);
    }

    #[test]
    fn foreign_owned_prefix_candidate_returns_ownership_conflict() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let session_store = fixture.main.join(".session");
        let worktree = git
            .create_worktree(
                "12345678-session",
                "agent/12345678-session",
                "main",
            )
            .unwrap();
        persist_worktree_owner(&session_store, SESSION_ID, &worktree);

        let fresh_activity =
            SessionStoreActivity::with_default_staleness(&session_store);
        let result = provision_for_session(
            &git,
            &fresh_activity,
            SAME_PREFIX_SESSION_ID,
            &policy(8),
        );

        assert!(matches!(
            result,
            Err(ProvisionError::SessionOwnershipConflict { worktree: path, .. })
                if path == worktree.path
        ));
    }

    #[test]
    fn unowned_legacy_prefix_candidate_remains_claimable() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let worktree = git
            .create_worktree(
                "12345678-session",
                "agent/12345678-session",
                "main",
            )
            .unwrap();
        let session_store = fixture.main.join(".session");
        let fresh_activity =
            SessionStoreActivity::with_default_staleness(&session_store);

        let reuse = provision_for_session(
            &git,
            &fresh_activity,
            SAME_PREFIX_SESSION_ID,
            &policy(8),
        )
        .unwrap();

        assert!(matches!(
            reuse,
            ProvisionOutcome::AlreadyProvisioned(reused) if reused == worktree
        ));
    }

    #[test]
    fn creates_named_worktree_when_no_candidate_exists() {
        let fixture = Fixture::new();
        let outcome = provision_for_session(
            &fixture.git(),
            &NeverActive,
            SESSION_ID,
            &policy(8),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            ProvisionOutcome::Created(worktree)
                if worktree.name == "session"
                    && worktree
                        .path
                        .ends_with(Path::new(SESSION_ID).join("session"))
                    && worktree.path.exists()
                    && worktree.branch.as_deref()
                        == Some("agent/12345678-1234-4234-8234-123456789abc/session")
        ));
    }

    #[test]
    fn discovery_reuses_legacy_flat_worktree() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let legacy = git
            .create_worktree("12345678-legacy", "agent/12345678-legacy", "main")
            .unwrap();

        let outcome =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy(8))
                .unwrap();

        assert!(matches!(
            outcome,
            ProvisionOutcome::AlreadyProvisioned(worktree) if worktree == legacy
        ));
    }

    #[test]
    fn discovery_reuses_nested_worktree_for_exact_session_id() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let nested_path = PathBuf::from(SESSION_ID).join("exact-match");
        let nested = git
            .create_worktree_at(
                &nested_path,
                "agent/12345678-1234-4234-8234-123456789abc/exact-match",
                "main",
            )
            .unwrap();

        let outcome =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy(8))
                .unwrap();

        assert!(matches!(
            outcome,
            ProvisionOutcome::AlreadyProvisioned(worktree) if worktree == nested
        ));
    }

    #[test]
    fn multiple_nested_slugs_return_sorted_ambiguity_error() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let alpha = git
            .create_worktree_at(
                &PathBuf::from(SESSION_ID).join("alpha"),
                "agent/12345678-1234-4234-8234-123456789abc/alpha",
                "main",
            )
            .unwrap();
        let beta = git
            .create_worktree_at(
                &PathBuf::from(SESSION_ID).join("beta"),
                "agent/12345678-1234-4234-8234-123456789abc/beta",
                "main",
            )
            .unwrap();

        let result =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy(8));

        assert!(matches!(
            result,
            Err(ProvisionError::AmbiguousSessionWorktree { candidates, .. })
                if candidates == vec![alpha.path, beta.path]
        ));
    }

    #[test]
    fn reclaims_clean_inactive_worktree_and_preserves_marker() {
        let fixture = Fixture::new();
        fs::write(fixture.main.join(".git/info/exclude"), "marker.txt\n")
            .unwrap();
        let git = fixture.git();
        let old = git.create_worktree("old", "agent/old", "main").unwrap();
        fs::write(old.path.join("marker.txt"), "keep\n").unwrap();
        assert!(!git.is_dirty(&old.path).unwrap());

        let outcome =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy(1))
                .unwrap();
        let reclaimed = match outcome {
            ProvisionOutcome::Reclaimed {
                worktree,
                previous_name,
            } => {
                assert_eq!(previous_name, "old");
                worktree
            },
            other => panic!("expected reclaim, got {other:?}"),
        };

        assert!(!old.path.exists());
        assert_eq!(
            fs::read_to_string(reclaimed.path.join("marker.txt")).unwrap(),
            "keep\n"
        );
        assert!(!git.branch_exists("agent/old").unwrap());
        assert!(
            git.branch_exists(
                "agent/12345678-1234-4234-8234-123456789abc/session"
            )
            .unwrap()
        );
    }

    #[test]
    fn dirty_worktree_is_not_reclaimed() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let old = git.create_worktree("old", "agent/old", "main").unwrap();
        fs::write(old.path.join("untracked.txt"), "preserve\n").unwrap();

        assert_cap_reached(provision_for_session(
            &git,
            &NeverActive,
            SESSION_ID,
            &policy(1),
        ));
        assert!(old.path.exists());
        assert!(git.branch_exists("agent/old").unwrap());
        assert!(
            !git.branch_exists(
                "agent/12345678-1234-4234-8234-123456789abc/session"
            )
            .unwrap()
        );
    }

    #[test]
    fn ahead_worktree_is_not_reclaimed() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let old = git.create_worktree("old", "agent/old", "main").unwrap();
        fs::write(old.path.join("tracked.txt"), "advance\n").unwrap();
        commit(&old.path, "advance");
        assert_eq!(git.ahead_behind(&old.path, "main").unwrap().0, 1);

        assert_cap_reached(provision_for_session(
            &git,
            &NeverActive,
            SESSION_ID,
            &policy(1),
        ));
        assert!(old.path.exists());
        assert!(git.branch_exists("agent/old").unwrap());
        assert!(!git.branch_exists("agent/12345678-session").unwrap());
    }

    #[test]
    fn active_worktree_is_not_reclaimed() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let old = git.create_worktree("old", "agent/old", "main").unwrap();
        let activity = ActiveWorktree(old.path.clone());

        assert_cap_reached(provision_for_session(
            &git,
            &activity,
            SESSION_ID,
            &policy(1),
        ));
        assert!(old.path.exists());
        assert!(git.branch_exists("agent/old").unwrap());
        assert!(!git.branch_exists("agent/12345678-session").unwrap());
    }

    #[test]
    fn cap_without_reclaim_candidate_returns_cap_reached() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let old = git.create_worktree("old", "agent/old", "main").unwrap();
        fs::write(old.path.join("untracked.txt"), "preserve\n").unwrap();

        let result =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy(1));
        assert_cap_reached(result);
        assert_eq!(git.list_worktrees().unwrap().len(), 1);
        assert!(
            !fixture
                .main
                .join(".worktrees/12345678-1234-4234-8234-123456789abc/session")
                .exists()
        );
    }

    #[test]
    fn cap_with_reclaim_candidate_reclaims_instead() {
        let fixture = Fixture::new();
        let git = fixture.git();
        git.create_worktree("old", "agent/old", "main").unwrap();

        let outcome =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy(1))
                .unwrap();
        assert!(matches!(outcome, ProvisionOutcome::Reclaimed { .. }));
        assert_eq!(git.list_worktrees().unwrap().len(), 1);
    }

    #[test]
    fn recently_active_worktree_is_not_reclaimed() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let old = git.create_worktree("old", "agent/old", "main").unwrap();
        let mut policy = policy(1);
        policy.idle_before_reclaim = Duration::from_secs(60 * 60);

        assert_cap_reached(provision_for_session(
            &git,
            &NeverActive,
            SESSION_ID,
            &policy,
        ));
        assert!(old.path.exists());
        assert!(
            !fixture
                .main
                .join(".worktrees/12345678-1234-4234-8234-123456789abc/session")
                .exists()
        );
    }

    #[test]
    fn idle_worktree_is_reclaimed_after_activity_signals_are_backdated() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let old = git.create_worktree("old", "agent/old", "main").unwrap();
        let mut policy = policy(1);
        policy.idle_before_reclaim = Duration::from_secs(60 * 60);
        backdate_activity_signals(&git, &old, Duration::from_secs(2 * 60 * 60));

        let outcome =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy)
                .unwrap();
        assert!(matches!(outcome, ProvisionOutcome::Reclaimed { .. }));
    }

    #[test]
    fn worktree_containing_the_current_directory_is_not_reclaimed() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let old = git.create_worktree("old", "agent/old", "main").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(&old.path).unwrap();

        let result =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy(1));

        std::env::set_current_dir(original).unwrap();
        assert_cap_reached(result);
        assert!(old.path.exists());
    }

    #[test]
    fn failed_reclaim_falls_through_to_fresh_creation() {
        let fixture = Fixture::new();
        fixture.add_submodule();
        let git = fixture.git();
        let old = git.create_worktree("old", "agent/old", "main").unwrap();
        fs::remove_dir_all(fixture.main.join("nested")).unwrap();

        let outcome =
            provision_for_session(&git, &NeverActive, SESSION_ID, &policy(2))
                .unwrap();
        assert!(matches!(outcome, ProvisionOutcome::Created(_)));
        assert!(old.path.exists());
        assert!(
            fixture
                .main
                .join(".worktrees/12345678-1234-4234-8234-123456789abc/session")
                .exists()
        );
    }

    #[test]
    fn failed_reclaim_at_cap_returns_cap_reached_without_creating() {
        let fixture = Fixture::new();
        fixture.add_submodule();
        let git = fixture.git();
        let old = git.create_worktree("old", "agent/old", "main").unwrap();
        fs::remove_dir_all(fixture.main.join("nested")).unwrap();

        assert_cap_reached(provision_for_session(
            &git,
            &NeverActive,
            SESSION_ID,
            &policy(1),
        ));
        assert!(old.path.exists());
        assert!(
            !fixture
                .main
                .join(".worktrees/12345678-1234-4234-8234-123456789abc/session")
                .exists()
        );
    }

    #[test]
    fn session_store_activity_honors_fresh_and_stale_records() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
        let record =
            fixture.main.join(".session/sessions/session/session.json");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        let fresh = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        let worktree_path = worktree.path.to_string_lossy().replace('\\', "/");
        fs::write(
            &record,
            format!(
                r#"{{"captured_at":"{fresh}","metadata":{{"worktree":{{"path":"{}"}}}}}}"#,
                worktree_path
            ),
        )
        .unwrap();

        let activity = SessionStoreActivity::new(
            fixture.main.join(".session"),
            Duration::from_secs(60),
        );
        assert!(activity.is_active(&worktree.path));

        fs::write(
            &record,
            format!(
                r#"{{"captured_at":"2000-01-01T00:00:00Z","metadata":{{"worktree":{{"path":"{}"}}}}}}"#,
                worktree_path
            ),
        )
        .unwrap();
        assert!(!activity.is_active(&worktree.path));
    }

    #[test]
    fn session_store_activity_prevents_reclaim_for_nested_worktree_record() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let worktree = git
            .create_worktree_at(
                &PathBuf::from("other-session").join("active"),
                "agent/other-session/active",
                "main",
            )
            .unwrap();
        let record =
            worktree.path.join(".session/sessions/active/session.json");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        let fresh = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        let worktree_path = worktree.path.to_string_lossy().replace('\\', "/");
        fs::write(
            record,
            format!(
                r#"{{"captured_at":"{fresh}","metadata":{{"worktree":{{"path":"{}"}}}}}}"#,
                worktree_path
            ),
        )
        .unwrap();

        let activity = SessionStoreActivity::new(
            fixture.main.join(".session"),
            Duration::from_secs(60),
        );
        let eligibility = evaluate_reclaim_candidate(
            &git,
            &activity,
            &only_worktree(&git),
            &policy(1),
        )
        .unwrap();

        assert_eq!(
            eligibility,
            ReclaimEligibility::Rejected(ReclaimRejectionReason::SessionActive)
        );
    }
}
