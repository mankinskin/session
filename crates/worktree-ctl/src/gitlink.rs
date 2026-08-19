use std::path::Path;

use git2::{
    BranchType,
    Oid,
    Repository,
};
use session_worktree_provision::WorktreeGit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitlinkState {
    Ok,
    Behind,
    Orphan,
    NotContained,
    Unresolvable,
}

#[derive(Debug)]
pub(crate) struct GitlinkStatus {
    pub(crate) submodule_path: String,
    pub(crate) recorded_sha: Oid,
    pub(crate) main_sha: Oid,
    pub(crate) state: GitlinkState,
}

pub(crate) fn verify_gitlink_containment(
    repo_root: &Path
) -> Result<Vec<GitlinkStatus>, String> {
    let superproject =
        Repository::open(repo_root).map_err(|error| error.to_string())?;
    let head = superproject.head().map_err(|error| error.to_string())?;
    let commit = head.peel_to_commit().map_err(|error| error.to_string())?;
    let tree = commit.tree().map_err(|error| error.to_string())?;
    let paths = WorktreeGit::open(repo_root)
        .map_err(|error| error.to_string())?
        .submodule_paths()
        .map_err(|error| error.to_string())?;

    paths
        .into_iter()
        .map(|submodule_path| {
            let recorded_sha = tree
                .get_path(Path::new(&submodule_path))
                .map_err(|error| error.to_string())?
                .id();
            let submodule = Repository::open(repo_root.join(&submodule_path))
                .map_err(|error| {
                    format!(
                        "failed to open submodule {submodule_path}: {error}"
                    )
                })?;
            let main = submodule
                .find_branch("main", BranchType::Local)
                .map_err(|error| {
                    format!(
                        "submodule {submodule_path} has no local main branch: {error}"
                    )
                })?;
            let main_sha = main.get().target().ok_or_else(|| {
                format!("submodule {submodule_path} main has no target")
            })?;
            let state = match submodule.find_commit(recorded_sha) {
                Ok(_) => {
                    let contained_in_main = main_sha == recorded_sha
                        || submodule
                            .graph_descendant_of(main_sha, recorded_sha)
                            .map_err(|error| error.to_string())?;
                    if contained_in_main {
                        if main_sha == recorded_sha {
                            GitlinkState::Ok
                        } else {
                            GitlinkState::Behind
                        }
                    } else if branch_contains(&submodule, recorded_sha)? {
                        GitlinkState::NotContained
                    } else {
                        GitlinkState::Orphan
                    }
                }
                Err(error) if error.code() == git2::ErrorCode::NotFound => {
                    GitlinkState::Unresolvable
                }
                Err(error) => return Err(error.to_string()),
            };
            Ok(GitlinkStatus {
                submodule_path,
                recorded_sha,
                main_sha,
                state,
            })
        })
        .collect()
}

pub(crate) fn reject_violations(
    statuses: &[GitlinkStatus]
) -> Result<(), String> {
    let violations = statuses
        .iter()
        .filter(|status| matches!(
            status.state,
            GitlinkState::Orphan
                | GitlinkState::NotContained
                | GitlinkState::Unresolvable
        ))
        .map(|status| match status.state {
            GitlinkState::Unresolvable => format!(
                "submodule {} recorded gitlink {} is not present in that submodule's object database; it was never fetched or has been garbage-collected. Fetch it (`git -C {} fetch <remote-or-local-path>`) or restore it from a rescue branch before merging.",
                status.submodule_path,
                status.recorded_sha,
                status.submodule_path,
            ),
            _ => format!(
                "submodule {} recorded {} is {:?}; local main is {}; run `git -C {} checkout main && git -C {} merge --ff-only {}` (a named feature branch, or the recorded commit sha itself if no branch points at it), then bump the gitlink",
                status.submodule_path,
                status.recorded_sha,
                status.state,
                status.main_sha,
                status.submodule_path,
                status.submodule_path,
                status.recorded_sha,
            ),
        })
        .collect::<Vec<_>>();
    if violations.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "gitlink containment failed:\n{}",
            violations.join("\n")
        ))
    }
}

pub(crate) fn partition_statuses(
    repo_root: &Path,
    statuses: Vec<GitlinkStatus>,
) -> Result<(Vec<GitlinkStatus>, Vec<GitlinkStatus>), String> {
    let mut fixable = Vec::new();
    let mut blocking = Vec::new();
    for status in statuses {
        if status.state == GitlinkState::Unresolvable {
            blocking.push(status);
            continue;
        }
        if !matches!(
            status.state,
            GitlinkState::Orphan | GitlinkState::NotContained
        ) {
            continue;
        }
        let submodule =
            Repository::open(repo_root.join(&status.submodule_path))
                .map_err(|error| error.to_string())?;
        let fast_forwardable = status.main_sha == status.recorded_sha
            || submodule
                .graph_descendant_of(status.recorded_sha, status.main_sha)
                .map_err(|error| error.to_string())?;
        if fast_forwardable {
            fixable.push(status);
        } else {
            blocking.push(status);
        }
    }
    Ok((fixable, blocking))
}

fn branch_contains(
    repository: &Repository,
    commit: Oid,
) -> Result<bool, String> {
    for branch in repository
        .branches(Some(BranchType::Local))
        .map_err(|error| error.to_string())?
    {
        let (branch, _) = branch.map_err(|error| error.to_string())?;
        let Some(tip) = branch.get().target() else {
            continue;
        };
        if tip == commit
            || repository
                .graph_descendant_of(tip, commit)
                .map_err(|error| error.to_string())?
        {
            return Ok(true);
        }
    }
    Ok(false)
}
