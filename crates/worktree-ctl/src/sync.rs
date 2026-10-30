use std::{env, path::Path};

use git2::{BranchType, Oid, Repository};
use session_worktree_provision::WorktreeGit;

use crate::{
    LifecyclePlan, checkpoint_owned_session_changes, checkpoint_session_mirror_changes,
    find_worktree,
    git::{git_command, run_git},
    gitlink,
};

const AUTOSTASH_MESSAGE: &str = "worktree-ctl autostash";
const AUTO_COMMIT_MESSAGE: &str = "worktree-ctl auto-commit before sync";
const REBASED_GITLINK_BASE_COMMIT_PREFIX: &str = "rebase submodule bases onto local main";
const REBASED_GITLINK_TIP_COMMIT_PREFIX: &str = "rebase submodule tips onto local main";
const REBASED_GITLINK_BRIDGE_PREFIX: &str = "worktree-ctl-rebase-bridge";

#[derive(Debug)]
struct RebasedGitlink {
    path: String,
    old: Oid,
    base: Oid,
    tip: Oid,
}

pub(crate) fn handle_rebase(name: &str, dry_run: bool, auto_commit: bool) -> Result<(), String> {
    let git = WorktreeGit::open(env::current_dir().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let worktree = find_worktree(&git, name)?;
    checkpoint_owned_session_changes(&git, &worktree, dry_run)?;
    let branch = worktree
        .branch
        .as_deref()
        .ok_or_else(|| format!("worktree {name} is detached and cannot be rebased"))?;
    let mut plan = LifecyclePlan::default();
    let mut rebased_gitlinks = Vec::new();

    if !dry_run {
        discard_current_gitlink_bridge_pair(&worktree.path)?;
    }

    for submodule in git.submodule_paths().map_err(|error| error.to_string())? {
        let nested = worktree.path.join(&submodule);
        if !repository_has_branch(&nested, branch)? {
            plan.add(format!(
                "skip {} because branch {branch} does not exist",
                nested.display()
            ));
            if !dry_run {
                println!("skip {submodule} because branch {branch} does not exist");
            }
            continue;
        }
        let old = gitlink_at_head(&worktree.path, &submodule)?;
        let base = local_main_tip(&nested)?;
        plan.add(format!(
            "checkout {branch} and rebase {} onto its local main",
            nested.display()
        ));
        let stashed = guard_dirty_tree(
            &git,
            &nested,
            &format!("submodule {submodule}"),
            auto_commit,
            dry_run,
            &mut plan,
        )?;
        if dry_run {
            continue;
        }
        let rebase = checkout_and_rebase(&nested, branch).map_err(|error| format!(
            "submodule {submodule} branch {branch} could not rebase onto local main: {error}; resolve the conflict in {} and continue or abort the rebase", nested.display()
        ));
        combine_results(rebase, restore_dirty_tree(&nested, stashed))?;
        let tip = head_tip(&nested)?;
        if old != tip {
            rebased_gitlinks.push(RebasedGitlink {
                path: submodule,
                old,
                base,
                tip,
            });
        }
    }

    if !dry_run {
        commit_rebased_gitlink_bridges(&worktree.path, &rebased_gitlinks)?;
    }

    plan.add(format!(
        "rebase {} onto local main",
        worktree.path.display()
    ));
    let stashed = guard_dirty_tree(
        &git,
        &worktree.path,
        "worktree",
        auto_commit,
        dry_run,
        &mut plan,
    )?;
    if dry_run {
        plan.emit();
        return Ok(());
    }
    combine_results(
        rebase_onto_local_main(&worktree.path),
        restore_dirty_tree(&worktree.path, stashed),
    )
}

pub(crate) fn handle_merge(name: &str, dry_run: bool, auto_commit: bool) -> Result<(), String> {
    let git = WorktreeGit::open(env::current_dir().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let worktree = find_worktree(&git, name)?;
    checkpoint_owned_session_changes(&git, &worktree, dry_run)?;
    let branch = worktree
        .branch
        .as_deref()
        .ok_or_else(|| format!("worktree {name} is detached and cannot be merged"))?;
    let mut plan = LifecyclePlan::default();
    let (fixable, blocking) = gitlink::partition_statuses(
        git.main_checkout(),
        gitlink::verify_gitlink_containment(git.main_checkout())?,
    )?;
    gitlink::reject_violations(&blocking)?;
    for status in &fixable {
        plan.add(format!("auto-fix gitlink: fast-forward submodule {} local main to recorded commit {} (only one possible resolution)", status.submodule_path, status.recorded_sha));
    }
    if !dry_run {
        for status in &fixable {
            let submodule = git.main_checkout().join(&status.submodule_path);
            run_git(&submodule, ["checkout", "main"])?;
            run_git(
                &submodule,
                ["merge", "--ff-only", &status.recorded_sha.to_string()],
            )?;
            println!(
                "auto-fixed gitlink: {} local main fast-forwarded to {}",
                status.submodule_path, status.recorded_sha
            );
        }
    }

    for submodule in git.submodule_paths().map_err(|error| error.to_string())? {
        let nested = worktree.path.join(&submodule);
        if !repository_has_branch(&nested, branch)? {
            plan.add(format!(
                "skip {} because branch {branch} does not exist",
                submodule
            ));
            continue;
        }
        let main_submodule = git.main_checkout().join(&submodule);
        ensure_submodule_branch_rebased(&main_submodule, branch, &submodule)?;
        plan.add(format!(
            "fast-forward {} local main from nested branch {branch}",
            main_submodule.display()
        ));
        let stashed = guard_dirty_tree(
            &git,
            &main_submodule,
            &format!("submodule {submodule}"),
            auto_commit,
            dry_run,
            &mut plan,
        )?;
        if !dry_run {
            combine_results(
                merge_ff_only(&main_submodule, branch),
                restore_dirty_tree(&main_submodule, stashed),
            )?;
        }
    }
    plan.add(format!(
        "fast-forward superproject local main from {branch}"
    ));
    let stashed = guard_dirty_tree(
        &git,
        git.main_checkout(),
        "superproject",
        auto_commit,
        dry_run,
        &mut plan,
    )?;
    if dry_run {
        plan.emit();
        return Ok(());
    }
    combine_results(
        merge_ff_only(git.main_checkout(), branch),
        restore_dirty_tree(git.main_checkout(), stashed),
    )?;
    gitlink::reject_violations(&gitlink::verify_gitlink_containment(git.main_checkout())?)
}

pub(crate) fn handle_sync(name: &str, dry_run: bool, auto_commit: bool) -> Result<(), String> {
    let git = WorktreeGit::open(env::current_dir().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let worktree = find_worktree(&git, name)?;
    checkpoint_owned_session_changes(&git, &worktree, dry_run)?;
    checkpoint_session_mirror_changes(&git, &worktree, dry_run)?;
    handle_rebase(name, dry_run, auto_commit)?;
    handle_merge(name, dry_run, auto_commit)
}

fn guard_dirty_tree(
    git: &WorktreeGit,
    path: &Path,
    label: &str,
    auto_commit: bool,
    dry_run: bool,
    plan: &mut LifecyclePlan,
) -> Result<bool, String> {
    if !git.is_dirty(path).map_err(|error| error.to_string())? {
        return Ok(false);
    }
    if auto_commit {
        plan.add(format!(
            "auto-commit uncommitted changes in {label} before mutating"
        ));
        if dry_run {
            return Ok(false);
        }
        run_git(path, ["add", "-A"])?;
        run_git(path, ["commit", "-m", AUTO_COMMIT_MESSAGE])?;
        println!("auto-committed uncommitted changes in {label}");
        return Ok(false);
    }
    plan.add(format!(
        "stash uncommitted changes in {label} before mutating (restored afterward)"
    ));
    if dry_run {
        return Ok(false);
    }
    let stash_before = stash_tip(path)?;
    run_git(
        path,
        [
            "stash",
            "push",
            "--include-untracked",
            "-m",
            AUTOSTASH_MESSAGE,
        ],
    )?;
    let stashed = stash_tip(path)? != stash_before;
    if stashed {
        println!("stashed uncommitted changes in {label} (restored afterward)");
    } else {
        println!("no stash created for {label}; working tree was unchanged");
    }
    Ok(stashed)
}

fn stash_tip(path: &Path) -> Result<Option<String>, String> {
    let output = git_command(path)
        .args(["rev-parse", "-q", "--verify", "refs/stash"])
        .output()
        .map_err(|error| format!("failed to inspect stash state: {error}"))?;
    if output.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ))
    } else if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(format!(
            "failed to inspect stash state: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn restore_dirty_tree(path: &Path, stashed: bool) -> Result<(), String> {
    if !stashed {
        return Ok(());
    }
    run_git(path, ["stash", "pop"]).map_err(|error| format!(
        "changes were stashed in {} before this operation but could not be restored automatically ({error}); run `git -C {} stash list` to recover them", path.display(), path.display()
    ))
}

fn combine_results(
    primary: Result<(), String>,
    secondary: Result<(), String>,
) -> Result<(), String> {
    match (primary, secondary) {
        (Err(primary), Err(secondary)) => Err(format!("{primary}; additionally, {secondary}")),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn repository_has_branch(path: &Path, branch: &str) -> Result<bool, String> {
    let repository = match Repository::open(path) {
        Ok(repository) => repository,
        Err(_) => return Ok(false),
    };
    match repository.find_branch(branch, BranchType::Local) {
        Ok(_) => Ok(true),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn gitlink_at_head(worktree: &Path, path: &str) -> Result<Oid, String> {
    let repository = Repository::open(worktree).map_err(|error| error.to_string())?;
    let commit = repository
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|error| error.to_string())?;
    commit
        .tree()
        .and_then(|tree| tree.get_path(Path::new(path)))
        .map(|entry| entry.id())
        .map_err(|error| error.to_string())
}

fn local_main_tip(repository: &Path) -> Result<Oid, String> {
    let repository = Repository::open(repository).map_err(|error| error.to_string())?;
    repository
        .find_branch("main", BranchType::Local)
        .and_then(|branch| branch.get().peel_to_commit())
        .map(|commit| commit.id())
        .map_err(|error| error.to_string())
}

fn head_tip(repository: &Path) -> Result<Oid, String> {
    let repository = Repository::open(repository).map_err(|error| error.to_string())?;
    repository
        .head()
        .and_then(|head| head.peel_to_commit())
        .map(|commit| commit.id())
        .map_err(|error| error.to_string())
}

fn merge_ff_only(repository: &Path, branch: &str) -> Result<(), String> {
    run_git(repository, ["merge", "--ff-only", branch]).map_err(|error| format!("merge --ff-only failed for {} from {branch}: {error}; rebase the feature branch onto local main and retry", repository.display()))
}

#[path = "sync/gitlink_rebase.rs"]
mod gitlink_rebase;
#[allow(unused_imports)]
pub use gitlink_rebase::*;
