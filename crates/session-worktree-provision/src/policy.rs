use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde_json::Value;
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{WorktreeGit, WorktreeGitError, WorktreeRef};

const DEFAULT_MAX_WORKTREES: usize = 8;
const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(4 * 60 * 60);
const DEFAULT_IDLE_BEFORE_RECLAIM: Duration = Duration::from_secs(24 * 60 * 60);

/// Determines whether a live session currently owns a worktree.
pub trait SessionActivity {
    /// True when some live session currently owns this worktree.
    fn is_active(&self, worktree: &Path) -> bool;

    fn worktree_ownership(&self, _worktree: &Path) -> WorktreeOwnership {
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

/// Test activity implementation that never marks a worktree active.
#[derive(Debug, Default)]
pub struct NeverActive;

impl SessionActivity for NeverActive {
    fn is_active(&self, _worktree: &Path) -> bool {
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
            max_worktrees: env_usize("WORKTREE_MAX").unwrap_or(DEFAULT_MAX_WORKTREES),
            stale_after: Duration::from_secs(
                env_u64("WORKTREE_STALE_SECS").unwrap_or(DEFAULT_STALE_AFTER.as_secs()),
            ),
            idle_before_reclaim: Duration::from_secs(
                env_u64("WORKTREE_IDLE_SECS").unwrap_or(DEFAULT_IDLE_BEFORE_RECLAIM.as_secs()),
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
    #[error("session {session_id} has ambiguous worktree candidates: {candidates:?}")]
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
    let ownership = activity.worktree_ownership(&worktree.path);
    let ahead = git.ahead_behind(&worktree.path, "main")?.0;
    let checkpoint_only = matches!(
        &ownership,
        WorktreeOwnership::Owned(owner_session_id)
            if ahead == 1 && git.is_owned_session_checkpoint(&worktree.path, owner_session_id)?
    );
    if ahead != 0 && !checkpoint_only {
        return Ok(ReclaimEligibility::Rejected(
            ReclaimRejectionReason::AheadOfMain,
        ));
    }
    if let WorktreeOwnership::Owned(owner_session_id) = ownership {
        git.checkpoint_owned_session_changes(&worktree.path, &owner_session_id)?;
    }
    if git.is_dirty(&worktree.path)? {
        return Ok(ReclaimEligibility::Rejected(ReclaimRejectionReason::Dirty));
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

    for candidate in reclaim_candidates_from_registered(git, activity, &worktrees, policy)? {
        let previous_name = candidate.name.clone();
        // TODO(5e6cf4f8): update reclaimed worktrees from main in a later provisioning unit.
        match git.rename_worktree(&previous_name, &relative_path, &branch) {
            Ok(worktree) => {
                return Ok(ProvisionOutcome::Reclaimed {
                    worktree,
                    previous_name,
                });
            }
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

fn registered_worktrees(git: &WorktreeGit) -> Result<Vec<WorktreeRef>, WorktreeGitError> {
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
        WorktreeOwnership::Owned(owner_session_id) if owner_session_id == session_id => {
            Ok(ProvisionOutcome::AlreadyProvisioned(worktree.clone()))
        }
        WorktreeOwnership::Owned(owner_session_id) => {
            Err(ProvisionError::SessionOwnershipConflict {
                worktree: worktree.path.clone(),
                session_id: session_id.to_string(),
                owner_session_id,
            })
        }
        WorktreeOwnership::Ambiguous => Err(ProvisionError::AmbiguousSessionWorktreeOwnership {
            worktree: worktree.path.clone(),
        }),
        WorktreeOwnership::Unowned => Ok(ProvisionOutcome::AlreadyProvisioned(worktree.clone())),
    }
}

fn ambiguous_session_worktree(session_id: &str, mut worktrees: Vec<WorktreeRef>) -> ProvisionError {
    worktrees.sort_by(|left, right| left.path.cmp(&right.path));
    ProvisionError::AmbiguousSessionWorktree {
        session_id: session_id.to_string(),
        candidates: worktrees
            .into_iter()
            .map(|worktree| worktree.path)
            .collect(),
    }
}

fn is_discoverable_worktree_path(root: &Path, path: &Path) -> bool {
    path.parent() == Some(root) || path.parent().and_then(Path::parent) == Some(root)
}

fn worktree_layout(root: &Path, path: &Path) -> Option<WorktreeLayout> {
    let relative = path.strip_prefix(root).ok()?;
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    match components.as_slice() {
        [name] => {
            let (short_id, slug) = name.split_once('-')?;
            (short_id.len() == 8 && !slug.is_empty()).then(|| WorktreeLayout::LegacyFlat {
                short_id: (*short_id).to_string(),
                slug: (*slug).to_string(),
            })
        }
        [session_id, slug] if !session_id.is_empty() && !slug.is_empty() => {
            Some(WorktreeLayout::Nested {
                session_id: (*session_id).to_string(),
                slug: (*slug).to_string(),
            })
        }
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

fn path_is_within(path: &Path, worktree: &Path) -> bool {
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
    let Some(cutoff) = SystemTime::now().checked_sub(idle_before_reclaim) else {
        return false;
    };
    last_activity < cutoff
}

fn worktree_last_activity(git: &WorktreeGit, worktree: &WorktreeRef) -> Option<SystemTime> {
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

fn reclaim_order(left: &WorktreeRef, right: &WorktreeRef) -> std::cmp::Ordering {
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

#[cfg(test)]
#[path = "policy/tests.rs"]
mod tests;

#[path = "policy/activity.rs"]
mod activity;
#[allow(unused_imports)]
pub use activity::*;
