use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::Value;

use git2::{Config, Repository, Status, StatusOptions};
use thiserror::Error;

pub mod policy;

pub use policy::{
    NeverActive, ProvisionError, ProvisionOutcome, ProvisionPolicy, ReclaimEligibility,
    ReclaimRejectionReason, SessionActivity, SessionStoreActivity, WorktreeOwnership,
    evaluate_reclaim_candidate, provision_for_session, reclaim_candidates,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRef {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
}

pub struct WorktreeGit {
    main_checkout: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyPathKind {
    Tracked,
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyPath {
    pub path: PathBuf,
    pub kind: DirtyPathKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityStore {
    Ticket,
    Spec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexRebuildOutcome {
    Rebuilt {
        store: EntityStore,
        elapsed: Duration,
    },
    Failed {
        store: EntityStore,
        elapsed: Duration,
        error: String,
    },
    Skipped {
        store: EntityStore,
        elapsed: Duration,
        reason: String,
    },
}

#[derive(Debug, Error)]
pub enum WorktreeGitError {
    #[error("invalid main checkout {}", path.display())]
    InvalidMainCheckout { path: PathBuf },
    #[error("invalid worktree name '{name}'")]
    InvalidWorktreeName { name: String },
    #[error("worktree '{name}' was not found")]
    WorktreeNotFound { name: String },
    #[error("worktree '{name}' is detached and has no branch to rename")]
    DetachedWorktree { name: String },
    #[error("git operation failed: {source}")]
    Git {
        #[from]
        source: git2::Error,
    },
    #[error("I/O failed for {}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("git command failed ({status}): {command}\nstderr: {stderr}")]
    CommandFailed {
        command: String,
        status: std::process::ExitStatus,
        stderr: String,
    },
    #[error("rollback failed after {original}: {rollback}")]
    Rollback {
        original: Box<Self>,
        rollback: String,
    },
    #[error(
        "worktree relocation from {} to {} failed: {original}; rollback also failed ({rollback}). Manual `git worktree repair` is required for both paths",
        from.display(),
        to.display()
    )]
    MoveRollbackFailed {
        from: PathBuf,
        to: PathBuf,
        original: Box<Self>,
        rollback: String,
    },
    #[error(
        "filesystem move from {} to {} crossed devices and recursive copy/delete fallback failed: {source}",
        from.display(),
        to.display()
    )]
    MoveFallback {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
}

impl WorktreeGit {
    pub fn open(main_checkout: impl Into<PathBuf>) -> Result<Self, WorktreeGitError> {
        let supplied = main_checkout.into();
        let main_checkout = fs::canonicalize(&supplied).map_err(|source| WorktreeGitError::Io {
            path: supplied,
            source,
        })?;
        Repository::open(&main_checkout).map_err(|_| WorktreeGitError::InvalidMainCheckout {
            path: main_checkout.clone(),
        })?;
        Ok(Self { main_checkout })
    }

    pub fn main_checkout(&self) -> &Path {
        &self.main_checkout
    }

    pub fn list_worktrees(&self) -> Result<Vec<WorktreeRef>, WorktreeGitError> {
        let repository = self.repository()?;
        let mut worktrees = Vec::new();
        for name in repository.worktrees()?.iter().flatten() {
            let worktree = repository.find_worktree(name)?;
            let path =
                fs::canonicalize(worktree.path()).map_err(|source| WorktreeGitError::Io {
                    path: worktree.path().to_path_buf(),
                    source,
                })?;
            let branch = branch_for_worktree(&path)?;
            worktrees.push(WorktreeRef {
                name: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(name)
                    .to_string(),
                path,
                branch,
            });
        }
        worktrees.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(worktrees)
    }

    pub fn branch_exists(&self, branch: &str) -> Result<bool, WorktreeGitError> {
        let repository = self.repository()?;
        match repository.find_branch(branch, git2::BranchType::Local) {
            Ok(_) => Ok(true),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(false),
            Err(source) => Err(source.into()),
        }
    }

    pub fn is_dirty(&self, worktree: &Path) -> Result<bool, WorktreeGitError> {
        let repository = Repository::open(worktree)?;
        let mut options = StatusOptions::new();
        options.include_untracked(true).recurse_untracked_dirs(true);
        Ok(repository
            .statuses(Some(&mut options))?
            .iter()
            .any(|entry| {
                let status = entry.status();
                status != Status::CURRENT && !status.contains(Status::IGNORED)
            }))
    }

    pub fn stash_push(&self, message: &str) -> Result<(), WorktreeGitError> {
        subprocess::run_arguments(&self.main_checkout, ["stash", "push", "-m", message])
    }

    pub fn stash_contains_message(&self, message: &str) -> Result<bool, WorktreeGitError> {
        let mut repository = self.repository()?;
        let mut found = false;
        repository.stash_foreach(|_, stash_message, _| {
            found = stash_message.contains(message);
            !found
        })?;
        Ok(found)
    }

    pub fn ahead_behind(
        &self,
        worktree: &Path,
        base: &str,
    ) -> Result<(usize, usize), WorktreeGitError> {
        let repository = Repository::open(worktree)?;
        let head = repository.head()?.peel_to_commit()?;
        let base = repository.revparse_single(base)?.peel_to_commit()?;
        Ok(repository.graph_ahead_behind(head.id(), base.id())?)
    }

    pub fn gitlink_sha(
        &self,
        worktree: &Path,
        submodule_path: &str,
    ) -> Result<String, WorktreeGitError> {
        gitlink_sha_at(worktree, submodule_path)
    }

    /// Direct submodule paths declared by this checkout's `.gitmodules`.
    /// Does not recurse into a submodule's own nested submodules; use
    /// `WorktreeGit::open` on that submodule path and call this again.
    pub fn submodule_paths(&self) -> Result<Vec<String>, WorktreeGitError> {
        submodule_paths_at(&self.main_checkout)
    }

    pub fn worktree_add_new_branch(
        &self,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<(), WorktreeGitError> {
        subprocess::run(
            &self.main_checkout,
            ["worktree", "add", "-b", branch],
            [path, Path::new(base)],
        )
    }

    pub fn submodule_worktree_add_detached(
        &self,
        submodule_path: &str,
        worktree: &Path,
        sha: &str,
    ) -> Result<(), WorktreeGitError> {
        submodule_worktree_add_detached_at(&self.main_checkout.join(submodule_path), worktree, sha)
    }

    pub fn worktree_remove_force(&self, path: &Path) -> Result<(), WorktreeGitError> {
        subprocess::run(
            &self.main_checkout,
            ["worktree", "remove", "--force"],
            [path],
        )
    }

    pub fn worktree_prune(&self) -> Result<(), WorktreeGitError> {
        subprocess::run(&self.main_checkout, ["worktree", "prune"], [])
    }

    pub fn worktree_move(&self, from: &Path, to: &Path) -> Result<(), WorktreeGitError> {
        if to.exists() {
            return Err(WorktreeGitError::Io {
                path: to.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "destination already exists",
                ),
            });
        }

        relocate_directory(from, to)?;
        if let Err(original) = self
            .repair_worktree(to)
            .and_then(|_| self.verify_worktree_move(from, to))
        {
            return Err(self.rollback_worktree_move(from, to, original));
        }
        Ok(())
    }

    pub fn branch_rename(&self, old: &str, new: &str) -> Result<(), WorktreeGitError> {
        subprocess::run(&self.main_checkout, ["branch", "-m", old, new], [])
    }

    pub fn branch_delete(&self, branch: &str, force: bool) -> Result<(), WorktreeGitError> {
        let flag = if force { "-D" } else { "-d" };
        subprocess::run(&self.main_checkout, ["branch", flag, branch], [])
    }

    fn repository(&self) -> Result<Repository, WorktreeGitError> {
        Ok(Repository::open(&self.main_checkout)?)
    }
}

/// Rebuild the machine-local derived indexes carried by a newly created
/// worktree. Each store is handled independently so one failed rebuild does
/// not prevent the other entity type from becoming usable.
pub fn rebuild_entity_indexes(worktree: &Path) -> Vec<IndexRebuildOutcome> {
    vec![
        rebuild_ticket_index(&store_root_for_workspace(worktree, "ticket")),
        rebuild_spec_index(&store_root_for_workspace(worktree, "spec")),
    ]
}

fn store_root_for_workspace(workspace: &Path, domain: &str) -> PathBuf {
    let canonical = workspace.join(".workflow-tools").join(domain);
    if canonical.is_dir() {
        canonical
    } else {
        workspace.join(format!(".{domain}"))
    }
}

fn rebuild_ticket_index(store_root: &Path) -> IndexRebuildOutcome {
    let started = std::time::Instant::now();
    if !store_root.is_dir() {
        return IndexRebuildOutcome::Skipped {
            store: EntityStore::Ticket,
            elapsed: started.elapsed(),
            reason: format!("ticket store is absent at {}", store_root.display()),
        };
    }

    match ticket_api::storage::TicketStore::init(store_root).and_then(|store| store.scan(true)) {
        Ok(_) => IndexRebuildOutcome::Rebuilt {
            store: EntityStore::Ticket,
            elapsed: started.elapsed(),
        },
        Err(error) => IndexRebuildOutcome::Failed {
            store: EntityStore::Ticket,
            elapsed: started.elapsed(),
            error: error.to_string(),
        },
    }
}

fn rebuild_spec_index(store_root: &Path) -> IndexRebuildOutcome {
    let started = std::time::Instant::now();
    if !store_root.is_dir() {
        return IndexRebuildOutcome::Skipped {
            store: EntityStore::Spec,
            elapsed: started.elapsed(),
            reason: format!("spec store is absent at {}", store_root.display()),
        };
    }

    match spec_api::SpecStore::init(store_root).and_then(|mut store| store.scan(true)) {
        Ok(_) => IndexRebuildOutcome::Rebuilt {
            store: EntityStore::Spec,
            elapsed: started.elapsed(),
        },
        Err(error) => IndexRebuildOutcome::Failed {
            store: EntityStore::Spec,
            elapsed: started.elapsed(),
            error: error.to_string(),
        },
    }
}

/// Git writes deliberately remain subprocess calls. With git2 0.20.4 and
/// libgit2-sys 0.18.7, `worktree add --detach <path> <sha>` cannot be expressed
/// because `WorktreeAddOptions::reference` accepts only a ref; remove has no
/// bound libgit2 symbol; branch-creating add is only partial; and `git worktree
/// move` refuses worktrees containing submodules outright, so move is replaced
/// with a filesystem relocation followed by Git's documented `worktree repair`.
/// Do not migrate these commands to git2: reads belong to git2, these writes do not.
mod subprocess {
    use std::{ffi::OsString, path::Path, process::Command};

    use super::WorktreeGitError;

    pub(super) fn run<const N: usize, const P: usize>(
        directory: &Path,
        arguments: [&str; N],
        paths: [&Path; P],
    ) -> Result<(), WorktreeGitError> {
        let mut command = Command::new("git");
        command.arg("-C").arg(git_path(directory));
        command.args(arguments);
        command.args(paths.into_iter().map(git_path));
        let rendered = render(&command);
        let output = command.output().map_err(|source| WorktreeGitError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(WorktreeGitError::CommandFailed {
                command: rendered,
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }

    pub(super) fn run_arguments<const N: usize>(
        directory: &Path,
        arguments: [&str; N],
    ) -> Result<(), WorktreeGitError> {
        let mut command = Command::new("git");
        command.arg("-C").arg(git_path(directory));
        command.args(arguments);
        let rendered = render(&command);
        let output = command.output().map_err(|source| WorktreeGitError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(WorktreeGitError::CommandFailed {
                command: rendered,
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }

    fn render(command: &Command) -> String {
        std::iter::once(command.get_program().to_os_string())
            .chain(command.get_args().map(OsString::from))
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn git_path(path: &Path) -> OsString {
        let path = path.as_os_str().to_string_lossy();
        let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
        OsString::from(path)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[path = "checkpoints.rs"]
mod checkpoints;

#[path = "lifecycle.rs"]
mod lifecycle;

#[path = "submodules.rs"]
mod submodules;
#[allow(unused_imports)]
pub use submodules::*;

#[path = "filesystem.rs"]
mod filesystem;
#[allow(unused_imports)]
pub use filesystem::*;

#[path = "session_records.rs"]
mod session_records;
#[allow(unused_imports)]
pub use session_records::*;
