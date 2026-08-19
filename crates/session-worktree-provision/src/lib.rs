use std::{
    fs,
    path::{
        Path,
        PathBuf,
    },
    time::Duration,
};

use git2::{
    Config,
    Repository,
    Status,
    StatusOptions,
};
use thiserror::Error;

pub mod policy;

pub use policy::{
    NeverActive,
    ProvisionError,
    ProvisionOutcome,
    ProvisionPolicy,
    ReclaimEligibility,
    ReclaimRejectionReason,
    SessionActivity,
    SessionStoreActivity,
    WorktreeOwnership,
    evaluate_reclaim_candidate,
    provision_for_session,
    reclaim_candidates,
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
    pub fn open(
        main_checkout: impl Into<PathBuf>
    ) -> Result<Self, WorktreeGitError> {
        let supplied = main_checkout.into();
        let main_checkout = fs::canonicalize(&supplied).map_err(|source| {
            WorktreeGitError::Io {
                path: supplied,
                source,
            }
        })?;
        Repository::open(&main_checkout).map_err(|_| {
            WorktreeGitError::InvalidMainCheckout {
                path: main_checkout.clone(),
            }
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
            let path = fs::canonicalize(worktree.path()).map_err(|source| {
                WorktreeGitError::Io {
                    path: worktree.path().to_path_buf(),
                    source,
                }
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

    pub fn branch_exists(
        &self,
        branch: &str,
    ) -> Result<bool, WorktreeGitError> {
        let repository = self.repository()?;
        match repository.find_branch(branch, git2::BranchType::Local) {
            Ok(_) => Ok(true),
            Err(error) if error.code() == git2::ErrorCode::NotFound =>
                Ok(false),
            Err(source) => Err(source.into()),
        }
    }

    pub fn is_dirty(
        &self,
        worktree: &Path,
    ) -> Result<bool, WorktreeGitError> {
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

    pub fn dirty_paths(
        &self,
        worktree: &Path,
    ) -> Result<Vec<DirtyPath>, WorktreeGitError> {
        let repository = Repository::open(worktree)?;
        let mut options = StatusOptions::new();
        options.include_untracked(true).recurse_untracked_dirs(true);
        let mut paths = repository
            .statuses(Some(&mut options))?
            .iter()
            .filter_map(|entry| {
                let status = entry.status();
                if status == Status::CURRENT || status.contains(Status::IGNORED)
                {
                    return None;
                }
                let path = entry.path()?;
                let kind =
                    if status.intersects(Status::INDEX_NEW | Status::WT_NEW) {
                        DirtyPathKind::Untracked
                    } else {
                        DirtyPathKind::Tracked
                    };
                Some(DirtyPath {
                    path: PathBuf::from(path),
                    kind,
                })
            })
            .collect::<Vec<_>>();
        paths.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(paths)
    }

    pub fn stash_push(
        &self,
        message: &str,
    ) -> Result<(), WorktreeGitError> {
        subprocess::run_arguments(
            &self.main_checkout,
            ["stash", "push", "-m", message],
        )
    }

    pub fn stash_contains_message(
        &self,
        message: &str,
    ) -> Result<bool, WorktreeGitError> {
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
        let repository = Repository::open(worktree)?;
        let tree = repository.head()?.peel_to_commit()?.tree()?;
        Ok(tree.get_path(Path::new(submodule_path))?.id().to_string())
    }

    pub fn submodule_paths(&self) -> Result<Vec<String>, WorktreeGitError> {
        let path = self.main_checkout.join(".gitmodules");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let config = Config::open(&path)?;
        let mut entries = config.entries(None)?;
        let mut paths = Vec::new();
        while let Some(entry) = entries.next() {
            let entry = entry?;
            let Some(name) = entry.name() else {
                continue;
            };
            if name
                .strip_prefix("submodule.")
                .and_then(|name| name.strip_suffix(".path"))
                .is_some()
                && let Some(value) = entry.value()
            {
                paths.push(value.to_string());
            }
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
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
        let submodule = self.main_checkout.join(submodule_path);
        if let Some(parent) = worktree.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                WorktreeGitError::Io {
                    path: parent.to_path_buf(),
                    source,
                }
            })?;
        }
        subprocess::run(
            &submodule,
            ["worktree", "add", "--detach"],
            [worktree, Path::new(sha)],
        )
    }

    fn submodule_worktree_remove_force(
        &self,
        submodule_path: &str,
        worktree: &Path,
    ) -> Result<(), WorktreeGitError> {
        subprocess::run(
            &self.main_checkout.join(submodule_path),
            ["worktree", "remove", "--force"],
            [worktree],
        )
    }

    pub fn worktree_remove_force(
        &self,
        path: &Path,
    ) -> Result<(), WorktreeGitError> {
        subprocess::run(
            &self.main_checkout,
            ["worktree", "remove", "--force"],
            [path],
        )
    }

    pub fn worktree_prune(&self) -> Result<(), WorktreeGitError> {
        subprocess::run(&self.main_checkout, ["worktree", "prune"], [])
    }

    pub fn worktree_move(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<(), WorktreeGitError> {
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

    pub fn branch_rename(
        &self,
        old: &str,
        new: &str,
    ) -> Result<(), WorktreeGitError> {
        subprocess::run(&self.main_checkout, ["branch", "-m", old, new], [])
    }

    pub fn branch_delete(
        &self,
        branch: &str,
        force: bool,
    ) -> Result<(), WorktreeGitError> {
        let flag = if force { "-D" } else { "-d" };
        subprocess::run(&self.main_checkout, ["branch", flag, branch], [])
    }

    pub fn create_worktree(
        &self,
        name: &str,
        branch: &str,
        base: &str,
    ) -> Result<WorktreeRef, WorktreeGitError> {
        self.create_worktree_at(Path::new(name), branch, base)
    }

    pub fn create_worktree_at(
        &self,
        relative_path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<WorktreeRef, WorktreeGitError> {
        validate_relative_worktree_path(relative_path)?;
        let name = relative_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("validated worktree path has a UTF-8 final component");
        let path = self.main_checkout.join(".worktrees").join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                WorktreeGitError::Io {
                    path: parent.to_path_buf(),
                    source,
                }
            })?;
        }
        self.worktree_add_new_branch(&path, branch, base)?;
        let result = self.populate_submodules_offline(&path).and_then(|_| {
            Ok(WorktreeRef {
                name: name.to_string(),
                path: fs::canonicalize(&path).map_err(|source| {
                    WorktreeGitError::Io {
                        path: path.clone(),
                        source,
                    }
                })?,
                branch: Some(branch.to_string()),
            })
        });
        result.map_err(|original| self.rollback_create(&path, branch, original))
    }

    pub fn rename_worktree(
        &self,
        old_name: &str,
        new_relative_path: &Path,
        new_branch: &str,
    ) -> Result<WorktreeRef, WorktreeGitError> {
        validate_name(old_name)?;
        validate_relative_worktree_path(new_relative_path)?;
        let new_name = new_relative_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("validated worktree path has a UTF-8 final component");
        let old = self
            .list_worktrees()?
            .into_iter()
            .find(|worktree| worktree.name == old_name)
            .ok_or_else(|| WorktreeGitError::WorktreeNotFound {
                name: old_name.to_string(),
            })?;
        let old_branch =
            old.branch
                .ok_or_else(|| WorktreeGitError::DetachedWorktree {
                    name: old_name.to_string(),
                })?;
        let new_path = self
            .main_checkout
            .join(".worktrees")
            .join(new_relative_path);
        if let Some(parent) = new_path.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                WorktreeGitError::Io {
                    path: parent.to_path_buf(),
                    source,
                }
            })?;
        }
        self.worktree_move(&old.path, &new_path)?;
        self.branch_rename(&old_branch, new_branch)?;
        Ok(WorktreeRef {
            name: new_name.to_string(),
            path: fs::canonicalize(&new_path).map_err(|source| {
                WorktreeGitError::Io {
                    path: new_path,
                    source,
                }
            })?,
            branch: Some(new_branch.to_string()),
        })
    }

    fn repository(&self) -> Result<Repository, WorktreeGitError> {
        Ok(Repository::open(&self.main_checkout)?)
    }

    fn repair_worktree(
        &self,
        worktree: &Path,
    ) -> Result<(), WorktreeGitError> {
        subprocess::run(
            &self.main_checkout,
            ["worktree", "repair"],
            [worktree],
        )?;
        for submodule in self.submodule_paths()? {
            let nested_worktree = worktree.join(&submodule);
            if nested_worktree.exists() {
                subprocess::run(
                    &self.main_checkout.join(submodule),
                    ["worktree", "repair"],
                    [&nested_worktree],
                )?;
            }
        }
        Ok(())
    }

    fn verify_worktree_move(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<(), WorktreeGitError> {
        subprocess::run(to, ["rev-parse", "--git-dir"], [])?;
        let worktrees = self.list_worktrees()?;
        if worktrees
            .iter()
            .any(|worktree| paths_equal(&worktree.path, from))
            || !worktrees
                .iter()
                .any(|worktree| paths_equal(&worktree.path, to))
        {
            return Err(WorktreeGitError::WorktreeNotFound {
                name: to.display().to_string(),
            });
        }
        Ok(())
    }

    fn rollback_worktree_move(
        &self,
        from: &Path,
        to: &Path,
        original: WorktreeGitError,
    ) -> WorktreeGitError {
        let rollback = relocate_directory(to, from)
            .and_then(|_| self.repair_worktree(from));
        match rollback {
            Ok(()) => original,
            Err(rollback) => WorktreeGitError::MoveRollbackFailed {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
                original: Box::new(original),
                rollback: rollback.to_string(),
            },
        }
    }

    fn populate_submodules_offline(
        &self,
        worktree: &Path,
    ) -> Result<(), WorktreeGitError> {
        // `submodule update` in a linked worktree repoints the shared
        // `.git/modules/<name>/core.worktree` and empties the main checkout.
        // Each nested linked worktree instead gets a private worktree git dir,
        // uses the already-present object store, and needs no network access.
        for submodule in self.submodule_paths()? {
            let source = self.main_checkout.join(&submodule);
            if !source.join(".git").exists() {
                eprintln!(
                    "warning: submodule {submodule} is not initialized in main checkout; skipping"
                );
                continue;
            }
            let sha = self.gitlink_sha(worktree, &submodule)?;
            self.submodule_worktree_add_detached(
                &submodule,
                &worktree.join(&submodule),
                &sha,
            )?;
        }
        Ok(())
    }

    fn rollback_create(
        &self,
        path: &Path,
        branch: &str,
        original: WorktreeGitError,
    ) -> WorktreeGitError {
        let mut failures = Vec::new();
        for submodule in self.submodule_paths().unwrap_or_default() {
            let nested = path.join(&submodule);
            if nested.exists()
                && self
                    .submodule_worktree_remove_force(&submodule, &nested)
                    .is_err()
            {
                failures.push(format!("remove nested {}", nested.display()));
            }
        }
        if path.exists() && self.worktree_remove_force(path).is_err() {
            failures.push(format!("remove {}", path.display()));
        }
        if self.worktree_prune().is_err() {
            failures.push("prune worktrees".to_string());
        }
        if self.branch_exists(branch).unwrap_or(false)
            && self.branch_delete(branch, true).is_err()
        {
            failures.push(format!("delete branch {branch}"));
        }
        if failures.is_empty() {
            original
        } else {
            WorktreeGitError::Rollback {
                original: Box::new(original),
                rollback: failures.join(", "),
            }
        }
    }
}

/// Rebuild the machine-local derived indexes carried by a newly created
/// worktree. Each store is handled independently so one failed rebuild does
/// not prevent the other entity type from becoming usable.
pub fn rebuild_entity_indexes(worktree: &Path) -> Vec<IndexRebuildOutcome> {
    vec![
        rebuild_ticket_index(&worktree.join(".ticket")),
        rebuild_spec_index(&worktree.join(".spec")),
    ]
}

fn rebuild_ticket_index(store_root: &Path) -> IndexRebuildOutcome {
    let started = std::time::Instant::now();
    if !store_root.is_dir() {
        return IndexRebuildOutcome::Skipped {
            store: EntityStore::Ticket,
            elapsed: started.elapsed(),
            reason: format!(
                "ticket store is absent at {}",
                store_root.display()
            ),
        };
    }

    match ticket_api::storage::TicketStore::init(store_root)
        .and_then(|store| store.scan(true))
    {
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

    match spec_api::SpecStore::init(store_root)
        .and_then(|mut store| store.scan(true))
    {
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

fn validate_name(name: &str) -> Result<(), WorktreeGitError> {
    if name.is_empty() || Path::new(name).components().count() != 1 {
        return Err(WorktreeGitError::InvalidWorktreeName {
            name: name.to_string(),
        });
    }
    Ok(())
}

fn validate_relative_worktree_path(
    path: &Path
) -> Result<(), WorktreeGitError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(component, std::path::Component::ParentDir)
        })
    {
        return Err(WorktreeGitError::InvalidWorktreeName {
            name: path.display().to_string(),
        });
    }
    Ok(())
}

fn branch_for_worktree(
    path: &Path
) -> Result<Option<String>, WorktreeGitError> {
    let repository = Repository::open(path)?;
    if repository.head_detached()? {
        return Ok(None);
    }
    let head = repository.head()?;
    Ok(head.shorthand().map(str::to_string))
}

fn relocate_directory(
    from: &Path,
    to: &Path,
) -> Result<(), WorktreeGitError> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_directory_recursively(from, to).map_err(|source| {
                WorktreeGitError::MoveFallback {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                    source,
                }
            })?;
            fs::remove_dir_all(from).map_err(|source| {
                WorktreeGitError::MoveFallback {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                    source,
                }
            })
        },
        Err(source) => Err(WorktreeGitError::Io {
            path: from.to_path_buf(),
            source,
        }),
    }
}

fn copy_directory_recursively(
    from: &Path,
    to: &Path,
) -> Result<(), std::io::Error> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.is_dir() {
            copy_directory_recursively(&source, &destination)?;
        } else if metadata.is_symlink() {
            copy_symlink(&source, &destination)?;
        } else {
            fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(
    source: &Path,
    destination: &Path,
) -> Result<(), std::io::Error> {
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)
}

#[cfg(windows)]
fn copy_symlink(
    source: &Path,
    destination: &Path,
) -> Result<(), std::io::Error> {
    let target = fs::read_link(source)?;
    if fs::metadata(source)?.is_dir() {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

fn paths_equal(
    left: &Path,
    right: &Path,
) -> bool {
    let normalize = |path: &Path| {
        fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase()
    };
    normalize(left) == normalize(right)
}

/// Git writes deliberately remain subprocess calls. With git2 0.20.4 and
/// libgit2-sys 0.18.7, `worktree add --detach <path> <sha>` cannot be expressed
/// because `WorktreeAddOptions::reference` accepts only a ref; remove has no
/// bound libgit2 symbol; branch-creating add is only partial; and `git worktree
/// move` refuses worktrees containing submodules outright, so move is replaced
/// with a filesystem relocation followed by Git's documented `worktree repair`.
/// Do not migrate these commands to git2: reads belong to git2, these writes do not.
mod subprocess {
    use std::{
        ffi::OsString,
        path::Path,
        process::Command,
    };

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
        let output =
            command.output().map_err(|source| WorktreeGitError::Io {
                path: directory.to_path_buf(),
                source,
            })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(WorktreeGitError::CommandFailed {
                command: rendered,
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .to_string(),
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
        let output =
            command.output().map_err(|source| WorktreeGitError::Io {
                path: directory.to_path_buf(),
                source,
            })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(WorktreeGitError::CommandFailed {
                command: rendered,
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .to_string(),
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
pub(crate) mod tests {
    use std::{
        fs,
        path::Path,
        process::Command,
    };

    use git2::{
        IndexAddOption,
        Repository,
        Signature,
    };
    use tempfile::TempDir;

    use super::WorktreeGit;

    pub(crate) struct Fixture {
        temp: TempDir,
        pub(crate) main: std::path::PathBuf,
    }

    impl Fixture {
        pub(crate) fn new() -> Self {
            let temp = TempDir::new().unwrap();
            let main = temp.path().join("main");
            let repository = Repository::init(&main).unwrap();
            repository.set_head("refs/heads/main").unwrap();
            fs::write(main.join("tracked.txt"), "initial\n").unwrap();
            commit_all(&repository, "initial");
            Self { temp, main }
        }

        pub(crate) fn git(&self) -> WorktreeGit {
            WorktreeGit::open(&self.main).unwrap()
        }

        pub(crate) fn add_submodule(&self) -> String {
            self.add_submodule_named("nested")
        }

        fn add_submodule_named(
            &self,
            name: &str,
        ) -> String {
            let inner = self.temp.path().join("inner");
            let repository = Repository::open(&inner).unwrap_or_else(|_| {
                let repository = Repository::init(&inner).unwrap();
                fs::write(inner.join("inner.txt"), "inner\n").unwrap();
                commit_all(&repository, "inner");
                repository
            });
            let sha = repository.head().unwrap().target().unwrap().to_string();
            command(
                &self.main,
                [
                    "-c",
                    "protocol.file.allow=always",
                    "submodule",
                    "add",
                    inner.to_str().unwrap(),
                    name,
                ],
            );
            command(&self.main, ["commit", "-am", "add nested"]);
            sha
        }
    }

    fn command<const N: usize>(
        directory: &Path,
        arguments: [&str; N],
    ) {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit_all(
        repository: &Repository,
        message: &str,
    ) {
        let mut index = repository.index().unwrap();
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repository.find_tree(tree_id).unwrap();
        let signature = Signature::now("test", "test@example.com").unwrap();
        let parents = repository
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok());
        let parent_refs = parents.iter().collect::<Vec<_>>();
        repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                &tree,
                &parent_refs,
            )
            .unwrap();
    }

    #[test]
    fn list_worktrees_and_branch_existence_use_git_metadata() {
        let fixture = Fixture::new();
        let git = fixture.git();
        git.create_worktree("one", "session-one", "HEAD").unwrap();
        git.create_worktree("two", "session-two", "HEAD").unwrap();
        assert_eq!(
            git.list_worktrees()
                .unwrap()
                .iter()
                .map(|worktree| worktree.name.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert!(git.branch_exists("session-one").unwrap());
        assert!(!git.branch_exists("missing").unwrap());
    }

    #[test]
    fn dirty_and_ahead_behind_report_worktree_state() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let worktree =
            git.create_worktree("one", "session-one", "HEAD").unwrap();
        assert!(!git.is_dirty(&worktree.path).unwrap());
        fs::write(worktree.path.join("untracked.txt"), "untracked\n").unwrap();
        assert!(git.is_dirty(&worktree.path).unwrap());
        fs::remove_file(worktree.path.join("untracked.txt")).unwrap();
        fs::write(worktree.path.join("tracked.txt"), "changed\n").unwrap();
        assert!(git.is_dirty(&worktree.path).unwrap());
        fs::write(worktree.path.join("tracked.txt"), "advanced\n").unwrap();
        assert_eq!(git.ahead_behind(&worktree.path, "HEAD~0").unwrap(), (0, 0));
        command(&worktree.path, ["add", "tracked.txt"]);
        command(
            &worktree.path,
            [
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "advance",
            ],
        );
        assert_eq!(git.ahead_behind(&worktree.path, "HEAD~1").unwrap(), (1, 0));
    }

    #[test]
    fn dirty_paths_report_tracked_and_untracked_files() {
        let fixture = Fixture::new();
        let git = fixture.git();
        fs::write(fixture.main.join("tracked.txt"), "changed\n").unwrap();
        fs::write(fixture.main.join("untracked.txt"), "new\n").unwrap();

        assert_eq!(
            git.dirty_paths(&fixture.main).unwrap(),
            vec![
                super::DirtyPath {
                    path: "tracked.txt".into(),
                    kind: super::DirtyPathKind::Tracked,
                },
                super::DirtyPath {
                    path: "untracked.txt".into(),
                    kind: super::DirtyPathKind::Untracked,
                },
            ]
        );
    }

    #[test]
    fn stash_push_creates_an_entry_with_the_requested_message() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let message = "preserve-main-changes";
        fs::write(fixture.main.join("tracked.txt"), "changed\n").unwrap();

        git.stash_push(message).unwrap();

        assert!(git.stash_contains_message(message).unwrap());
    }

    #[test]
    fn create_worktree_populates_submodules_and_reports_gitlink() {
        let fixture = Fixture::new();
        let sha = fixture.add_submodule();
        let git = fixture.git();
        let worktree = git
            .create_worktree("with-submodule", "session-submodule", "HEAD")
            .unwrap();
        assert!(worktree.path.is_dir());
        assert_eq!(worktree.branch.as_deref(), Some("session-submodule"));
        assert!(worktree.path.join("nested").is_dir());
        assert_eq!(git.gitlink_sha(&worktree.path, "nested").unwrap(), sha);
    }

    #[test]
    fn create_worktree_rolls_back_when_submodule_commit_is_unavailable() {
        let fixture = Fixture::new();
        fixture.add_submodule_named("good");
        fixture.add_submodule();
        let fake_sha = "0123456789012345678901234567890123456789";
        command(
            &fixture.main,
            [
                "update-index",
                "--cacheinfo",
                &format!("160000,{fake_sha},nested"),
            ],
        );
        command(&fixture.main, ["commit", "-m", "broken gitlink"]);
        let git = fixture.git();
        assert!(
            git.create_worktree("broken", "session-broken", "HEAD")
                .is_err()
        );
        assert!(!fixture.main.join(".worktrees/broken").exists());
        assert!(!git.branch_exists("session-broken").unwrap());
        assert!(git.list_worktrees().unwrap().is_empty());
    }

    #[test]
    fn worktree_move_repairs_a_worktree_containing_a_submodule() {
        let fixture = Fixture::new();
        fixture.add_submodule();
        fs::write(fixture.main.join(".git/info/exclude"), "marker.txt\n")
            .unwrap();
        let git = fixture.git();
        let old = git.create_worktree("old", "session-old", "HEAD").unwrap();
        fs::write(old.path.join("marker.txt"), "keep\n").unwrap();
        let new = fixture.main.join(".worktrees/new");

        git.worktree_move(&old.path, &new).unwrap();

        command(&new, ["rev-parse", "--git-dir"]);
        command(&new.join("nested"), ["rev-parse", "--git-dir"]);
        assert_eq!(
            fs::read_to_string(new.join("marker.txt")).unwrap(),
            "keep\n"
        );
        let paths = git
            .list_worktrees()
            .unwrap()
            .into_iter()
            .map(|worktree| worktree.path)
            .collect::<Vec<_>>();
        assert!(paths.iter().any(|path| super::paths_equal(path, &new)));
        assert!(!paths.iter().any(|path| super::paths_equal(path, &old.path)));
    }

    #[test]
    fn worktree_move_refuses_an_existing_destination() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let old = git.create_worktree("old", "session-old", "HEAD").unwrap();
        let destination = fixture.main.join(".worktrees/existing");
        fs::create_dir_all(&destination).unwrap();

        assert!(git.worktree_move(&old.path, &destination).is_err());
        assert!(old.path.exists());
    }

    #[test]
    fn worktree_move_rolls_back_when_nested_repair_fails() {
        let fixture = Fixture::new();
        fixture.add_submodule();
        let git = fixture.git();
        let old = git.create_worktree("old", "session-old", "HEAD").unwrap();
        let destination = fixture.main.join(".worktrees/new");
        fs::remove_dir_all(fixture.main.join("nested")).unwrap();

        assert!(git.worktree_move(&old.path, &destination).is_err());
        assert!(old.path.exists());
        command(&old.path, ["rev-parse", "--git-dir"]);
        assert!(!destination.exists());
    }

    #[test]
    fn rename_worktree_moves_in_place_and_preserves_marker() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let old = git.create_worktree("old", "session-old", "HEAD").unwrap();
        fs::write(old.path.join("marker.txt"), "keep\n").unwrap();
        let renamed = git
            .rename_worktree("old", Path::new("new"), "session-new")
            .unwrap();
        assert!(!old.path.exists());
        assert_eq!(
            fs::read_to_string(renamed.path.join("marker.txt")).unwrap(),
            "keep\n"
        );
        assert!(!git.branch_exists("session-old").unwrap());
        assert!(git.branch_exists("session-new").unwrap());
        assert_eq!(renamed.branch.as_deref(), Some("session-new"));
    }

    #[test]
    fn remove_and_prune_clear_worktree_registration() {
        let fixture = Fixture::new();
        let git = fixture.git();
        let worktree =
            git.create_worktree("one", "session-one", "HEAD").unwrap();
        git.worktree_remove_force(&worktree.path).unwrap();
        git.worktree_prune().unwrap();
        assert!(git.list_worktrees().unwrap().is_empty());
    }

    #[test]
    fn index_rebuild_skips_missing_store_paths() {
        let fixture = TempDir::new().unwrap();
        let outcomes = super::rebuild_entity_indexes(fixture.path());

        assert!(matches!(
            outcomes.as_slice(),
            [
                super::IndexRebuildOutcome::Skipped {
                    store: super::EntityStore::Ticket,
                    ..
                },
                super::IndexRebuildOutcome::Skipped {
                    store: super::EntityStore::Spec,
                    ..
                },
            ]
        ));
    }
}
