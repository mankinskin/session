#[allow(unused_imports)]
use super::*;

impl WorktreeGit {
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
            fs::create_dir_all(parent).map_err(|source| WorktreeGitError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        self.worktree_add_new_branch(&path, branch, base)?;
        let result = self.populate_submodules_offline(&path).and_then(|_| {
            Ok(WorktreeRef {
                name: name.to_string(),
                path: fs::canonicalize(&path).map_err(|source| WorktreeGitError::Io {
                    path: path.clone(),
                    source,
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
        let old_branch = old
            .branch
            .ok_or_else(|| WorktreeGitError::DetachedWorktree {
                name: old_name.to_string(),
            })?;
        let new_path = self
            .main_checkout
            .join(".worktrees")
            .join(new_relative_path);
        if let Some(parent) = new_path.parent() {
            fs::create_dir_all(parent).map_err(|source| WorktreeGitError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        self.worktree_move(&old.path, &new_path)?;
        self.branch_rename(&old_branch, new_branch)?;
        Ok(WorktreeRef {
            name: new_name.to_string(),
            path: fs::canonicalize(&new_path).map_err(|source| WorktreeGitError::Io {
                path: new_path,
                source,
            })?,
            branch: Some(new_branch.to_string()),
        })
    }

    pub(super) fn repair_worktree(&self, worktree: &Path) -> Result<(), WorktreeGitError> {
        subprocess::run(&self.main_checkout, ["worktree", "repair"], [worktree])?;
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

    pub(super) fn verify_worktree_move(
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

    pub(super) fn rollback_worktree_move(
        &self,
        from: &Path,
        to: &Path,
        original: WorktreeGitError,
    ) -> WorktreeGitError {
        let rollback = relocate_directory(to, from).and_then(|_| self.repair_worktree(from));
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

    pub(super) fn populate_submodules_offline(
        &self,
        worktree: &Path,
    ) -> Result<(), WorktreeGitError> {
        populate_submodules_offline_at(&self.main_checkout, worktree)
    }

    pub(super) fn rollback_create(
        &self,
        path: &Path,
        branch: &str,
        original: WorktreeGitError,
    ) -> WorktreeGitError {
        let mut failures = Vec::new();
        remove_nested_submodule_worktrees_at(&self.main_checkout, path, &mut failures);
        if path.exists() && self.worktree_remove_force(path).is_err() {
            failures.push(format!("remove {}", path.display()));
        }
        if self.worktree_prune().is_err() {
            failures.push("prune worktrees".to_string());
        }
        if self.branch_exists(branch).unwrap_or(false) && self.branch_delete(branch, true).is_err()
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
