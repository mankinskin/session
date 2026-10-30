#[allow(unused_imports)]
use super::*;

impl WorktreeGit {
    pub fn dirty_paths(&self, worktree: &Path) -> Result<Vec<DirtyPath>, WorktreeGitError> {
        let repository = Repository::open(worktree)?;
        let mut options = StatusOptions::new();
        options.include_untracked(true).recurse_untracked_dirs(true);
        let mut paths = repository
            .statuses(Some(&mut options))?
            .iter()
            .filter_map(|entry| {
                let status = entry.status();
                if status == Status::CURRENT || status.contains(Status::IGNORED) {
                    return None;
                }
                let path = entry.path()?;
                let kind = if status.intersects(Status::INDEX_NEW | Status::WT_NEW) {
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

    /// Commit session capture artifacts only when they belong to the session
    /// that owns this nested worktree. Any unrelated dirty path leaves the
    /// worktree untouched so callers retain the ordinary dirty-worktree gate.
    pub fn checkpoint_owned_session_changes(
        &self,
        worktree: &Path,
        owner_session_id: &str,
    ) -> Result<bool, WorktreeGitError> {
        let Some(expected) =
            owned_session_directory(self.main_checkout(), worktree, owner_session_id)
        else {
            return Ok(false);
        };
        let record = worktree.join(&expected).join("session.json");
        let record_text = match fs::read_to_string(&record) {
            Ok(text) => text,
            Err(_) => return Ok(false),
        };
        let recorded_owner = serde_json::from_str::<Value>(&record_text)
            .ok()
            .and_then(|value| value.get("session_id")?.as_str().map(str::to_owned));
        if recorded_owner.as_deref() != Some(owner_session_id) {
            return Ok(false);
        }

        let dirty = self.dirty_paths(worktree)?;
        if dirty.is_empty()
            || dirty
                .iter()
                .any(|path| !path.path.starts_with(&expected) && !expected.starts_with(&path.path))
        {
            return Ok(false);
        }

        subprocess::run(worktree, ["add", "--"], [&expected])?;
        subprocess::run_arguments(
            worktree,
            [
                "commit",
                "-m",
                "worktree-ctl checkpoint owned session record",
            ],
        )?;
        Ok(true)
    }

    /// Checkpoint the main checkout's mirror for a worktree-owned session.
    /// Unlike the worktree checkpoint, unrelated main-checkout changes may
    /// exist: this stages only the validated mirror directory so the caller
    /// can rebase the worktree on that commit before stashing other changes.
    pub fn checkpoint_session_mirror_changes(
        &self,
        worktree: &Path,
        owner_session_id: &str,
    ) -> Result<bool, WorktreeGitError> {
        let expected = existing_session_directory(&self.main_checkout, owner_session_id);
        let record = expected.join("session.json");
        if !session_record_matches_owner(&record, owner_session_id, worktree) {
            return Ok(false);
        }
        let relative = expected
            .strip_prefix(&self.main_checkout)
            .unwrap_or(&expected);
        let dirty = self.dirty_paths(&self.main_checkout)?;
        if !dirty
            .iter()
            .any(|path| path.path.starts_with(relative) || relative.starts_with(&path.path))
        {
            return Ok(false);
        }

        subprocess::run(&self.main_checkout, ["add", "--"], [relative])?;
        subprocess::run_arguments(
            &self.main_checkout,
            ["commit", "-m", "worktree-ctl checkpoint session mirror"],
        )?;
        Ok(true)
    }

    pub fn is_owned_session_checkpoint(
        &self,
        worktree: &Path,
        owner_session_id: &str,
    ) -> Result<bool, WorktreeGitError> {
        const CHECKPOINT_MESSAGE: &str = "worktree-ctl checkpoint owned session record";
        let Some(expected) =
            owned_session_directory(self.main_checkout(), worktree, owner_session_id)
        else {
            return Ok(false);
        };
        let repository = Repository::open(worktree)?;
        let head = repository.head()?.peel_to_commit()?;
        if head.message().map(str::trim_end) != Some(CHECKPOINT_MESSAGE) || head.parent_count() != 1
        {
            return Ok(false);
        }
        let parent = head.parent(0)?;
        let parent_tree = parent.tree()?;
        let head_tree = head.tree()?;
        let diff = repository.diff_tree_to_tree(Some(&parent_tree), Some(&head_tree), None)?;
        Ok(diff.deltas().all(|delta| {
            delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .is_some_and(|path| path.starts_with(&expected))
        }) && diff.deltas().len() != 0)
    }
}
