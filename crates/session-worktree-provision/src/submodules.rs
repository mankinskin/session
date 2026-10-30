#[allow(unused_imports)]
use super::*;

/// Direct submodule paths declared by `.gitmodules` in an arbitrary checkout.
pub(super) fn submodule_paths_at(checkout: &Path) -> Result<Vec<String>, WorktreeGitError> {
    let path = checkout.join(".gitmodules");
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

pub(super) fn gitlink_sha_at(
    worktree: &Path,
    submodule_path: &str,
) -> Result<String, WorktreeGitError> {
    let repository = Repository::open(worktree)?;
    let tree = repository.head()?.peel_to_commit()?.tree()?;
    Ok(tree.get_path(Path::new(submodule_path))?.id().to_string())
}

pub(super) fn submodule_worktree_add_detached_at(
    source_checkout: &Path,
    worktree: &Path,
    sha: &str,
) -> Result<(), WorktreeGitError> {
    if let Some(parent) = worktree.parent() {
        fs::create_dir_all(parent).map_err(|source| WorktreeGitError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    subprocess::run(
        source_checkout,
        ["worktree", "add", "--detach"],
        [worktree, Path::new(sha)],
    )
}

pub(super) fn submodule_worktree_remove_force_at(
    source_checkout: &Path,
    worktree: &Path,
) -> Result<(), WorktreeGitError> {
    subprocess::run(
        source_checkout,
        ["worktree", "remove", "--force"],
        [worktree],
    )
}

/// Recursively populate each submodule, and every submodule nested inside it,
/// as detached linked worktrees built entirely from locally available
/// objects. `source_checkout` is the already-initialized checkout whose
/// `.gitmodules` and gitlinks are read; `worktree` is the sibling linked
/// worktree being populated to match it.
pub(super) fn populate_submodules_offline_at(
    source_checkout: &Path,
    worktree: &Path,
) -> Result<(), WorktreeGitError> {
    for submodule in submodule_paths_at(source_checkout)? {
        let source = source_checkout.join(&submodule);
        if !source.join(".git").exists() {
            eprintln!(
                "warning: submodule {submodule} is not initialized in {}; skipping",
                source_checkout.display()
            );
            continue;
        }
        let target = worktree.join(&submodule);
        let sha = gitlink_sha_at(worktree, &submodule)?;
        submodule_worktree_add_detached_at(&source, &target, &sha)?;
        populate_submodules_offline_at(&source, &target)?;
    }
    Ok(())
}

/// Remove nested submodule worktrees depth-first (innermost first) so each
/// `git worktree remove` still finds its path intact when it runs.
pub(super) fn remove_nested_submodule_worktrees_at(
    source_checkout: &Path,
    worktree: &Path,
    failures: &mut Vec<String>,
) {
    let Ok(submodules) = submodule_paths_at(source_checkout) else {
        return;
    };
    for submodule in submodules {
        let source = source_checkout.join(&submodule);
        let nested = worktree.join(&submodule);
        if !nested.exists() {
            continue;
        }
        remove_nested_submodule_worktrees_at(&source, &nested, failures);
        if submodule_worktree_remove_force_at(&source, &nested).is_err() {
            failures.push(format!("remove nested {}", nested.display()));
        }
    }
}
