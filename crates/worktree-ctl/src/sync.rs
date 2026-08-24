use std::{
    env,
    path::Path,
};

use git2::{
    BranchType,
    Oid,
    Repository,
};
use session_worktree_provision::WorktreeGit;

use crate::{
    LifecyclePlan,
    find_worktree,
    git::{
        git_command,
        run_git,
    },
    gitlink,
};

const AUTOSTASH_MESSAGE: &str = "worktree-ctl autostash";
const AUTO_COMMIT_MESSAGE: &str = "worktree-ctl auto-commit before sync";

pub(crate) fn handle_rebase(
    name: &str,
    dry_run: bool,
    auto_commit: bool,
) -> Result<(), String> {
    let git = WorktreeGit::open(
        env::current_dir().map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let worktree = find_worktree(&git, name)?;
    let branch = worktree.branch.as_deref().ok_or_else(|| {
        format!("worktree {name} is detached and cannot be rebased")
    })?;
    let mut plan = LifecyclePlan::default();

    for submodule in git.submodule_paths().map_err(|error| error.to_string())? {
        let nested = worktree.path.join(&submodule);
        if !repository_has_branch(&nested, branch)? {
            plan.add(format!(
                "skip {} because branch {branch} does not exist",
                nested.display()
            ));
            if !dry_run {
                println!(
                    "skip {submodule} because branch {branch} does not exist"
                );
            }
            continue;
        }
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
        commit_rebased_gitlink(&worktree.path, &submodule)?;
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

pub(crate) fn handle_merge(
    name: &str,
    dry_run: bool,
    auto_commit: bool,
) -> Result<(), String> {
    let git = WorktreeGit::open(
        env::current_dir().map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let worktree = find_worktree(&git, name)?;
    let branch = worktree.branch.as_deref().ok_or_else(|| {
        format!("worktree {name} is detached and cannot be merged")
    })?;
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
        reject_unmerged_submodule_branch(&main_submodule, branch, &submodule)?;
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
    gitlink::reject_violations(&gitlink::verify_gitlink_containment(
        git.main_checkout(),
    )?)
}

pub(crate) fn handle_sync(
    name: &str,
    dry_run: bool,
    auto_commit: bool,
) -> Result<(), String> {
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
    plan.add(format!("stash uncommitted changes in {label} before mutating (restored afterward)"));
    if dry_run {
        return Ok(false);
    }
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
    println!("stashed uncommitted changes in {label} (restored afterward)");
    Ok(true)
}

fn restore_dirty_tree(
    path: &Path,
    stashed: bool,
) -> Result<(), String> {
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
        (Err(primary), Err(secondary)) =>
            Err(format!("{primary}; additionally, {secondary}")),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn repository_has_branch(
    path: &Path,
    branch: &str,
) -> Result<bool, String> {
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

fn reject_unmerged_submodule_branch(
    path: &Path,
    branch: &str,
    submodule: &str,
) -> Result<(), String> {
    let repository =
        Repository::open(path).map_err(|error| error.to_string())?;
    let feature = repository
        .find_branch(branch, BranchType::Local)
        .map_err(|error| error.to_string())?
        .get()
        .target()
        .ok_or_else(|| {
            format!("submodule {submodule} branch {branch} has no target")
        })?;
    let main = repository
        .find_branch("main", BranchType::Local)
        .map_err(|error| error.to_string())?
        .get()
        .target()
        .ok_or_else(|| format!("submodule {submodule} main has no target"))?;
    if main == feature
        || repository
            .graph_descendant_of(main, feature)
            .map_err(|error| error.to_string())?
    {
        Ok(())
    } else {
        Err(format!(
            "submodule {submodule} branch {branch} ({feature}) is not contained in local main ({main}); run `git -C {submodule} checkout main && git -C {submodule} merge --ff-only {branch}` before merging the superproject"
        ))
    }
}

fn checkout_and_rebase(
    worktree: &Path,
    branch: &str,
) -> Result<(), String> {
    run_git(worktree, ["checkout", branch])?;
    rebase_onto_local_main(worktree)
}

pub(crate) fn rebase_onto_local_main(worktree: &Path) -> Result<(), String> {
    if reset_redundant_gitlink_only_branch(worktree)? {
        return Ok(());
    }
    let output = git_command(worktree)
        .args(["rebase", "main"])
        .output()
        .map_err(|error| format!("failed to start git rebase: {error}"))?;
    if output.status.success() {
        return Ok(());
    }

    if resolve_redundant_gitlink_conflicts(worktree)? {
        return continue_or_skip_rebase(worktree);
    }

    Err(format!(
        "git rebase main failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn reset_redundant_gitlink_only_branch(worktree: &Path) -> Result<bool, String> {
    let output = git_command(worktree)
        .args(["diff", "--raw", "--no-abbrev", "main", "HEAD"])
        .output()
        .map_err(|error| format!("failed to inspect gitlink rebase candidates: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to inspect gitlink rebase candidates: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let changes = String::from_utf8_lossy(&output.stdout);
    let changes = changes.lines().filter(|line| !line.is_empty()).collect::<Vec<_>>();
    if changes.is_empty() {
        return Ok(false);
    }

    for change in &changes {
        let fields = change.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 6 || fields[0] != ":160000" || fields[1] != "160000" {
            return Ok(false);
        }
        let main = Oid::from_str(fields[2]).map_err(|error| error.to_string())?;
        let branch = Oid::from_str(fields[3]).map_err(|error| error.to_string())?;
        let repository = match Repository::open(worktree.join(fields[5])) {
            Ok(repository) => repository,
            Err(_) => return Ok(false),
        };
        if !repository
            .graph_descendant_of(main, branch)
            .map_err(|error| error.to_string())?
        {
            return Ok(false);
        }
    }

    run_git(worktree, ["reset", "--hard", "main"])?;
    println!(
        "skipped redundant gitlink-only branch changes: main already records descendant submodule commits"
    );
    Ok(true)
}

fn resolve_redundant_gitlink_conflicts(worktree: &Path) -> Result<bool, String> {
    let output = git_command(worktree)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output()
        .map_err(|error| format!("failed to inspect rebase conflicts: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to inspect rebase conflicts: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let paths = String::from_utf8_lossy(&output.stdout);
    let paths = paths.lines().filter(|path| !path.is_empty()).collect::<Vec<_>>();
    if paths.is_empty() {
        return Ok(false);
    }

    for path in &paths {
        let entries = git_command(worktree)
            .args(["ls-files", "-u", "--", path])
            .output()
            .map_err(|error| format!("failed to inspect conflicted gitlink {path}: {error}"))?;
        if !entries.status.success() {
            return Err(format!(
                "failed to inspect conflicted gitlink {path}: {}",
                String::from_utf8_lossy(&entries.stderr).trim()
            ));
        }
        let mut ours = None;
        let mut theirs = None;
        let entries = String::from_utf8_lossy(&entries.stdout);
        for entry in entries.lines() {
            let fields = entry.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 3 || fields[0] != "160000" {
                return Ok(false);
            }
            match fields[2] {
                "2" => ours = Some(fields[1]),
                "3" => theirs = Some(fields[1]),
                _ => {}
            }
        }
        let (Some(ours), Some(theirs)) = (ours, theirs) else {
            return Ok(false);
        };
        let repository = match Repository::open(worktree.join(path)) {
            Ok(repository) => repository,
            Err(_) => return Ok(false),
        };
        let ours = Oid::from_str(ours).map_err(|error| error.to_string())?;
        let theirs = Oid::from_str(theirs).map_err(|error| error.to_string())?;
        if !repository
            .graph_descendant_of(ours, theirs)
            .map_err(|error| error.to_string())?
        {
            return Ok(false);
        }
    }

    for path in paths {
        run_git(worktree, ["checkout", "--ours", "--", path])?;
        run_git(worktree, ["add", "--", path])?;
        println!(
            "resolved redundant gitlink conflict in {path}: retained the rebase target's descendant commit"
        );
    }
    Ok(true)
}

fn continue_or_skip_rebase(worktree: &Path) -> Result<(), String> {
    let staged = git_command(worktree)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map_err(|error| format!("failed to inspect resolved rebase: {error}"))?;
    let command = if staged.success() {
        ["rebase", "--skip"]
    } else if staged.code() == Some(1) {
        ["rebase", "--continue"]
    } else {
        return Err("failed to inspect resolved rebase".to_owned());
    };
    run_git(worktree, command).map_err(|error| format!(
        "automatic redundant gitlink resolution could not finish the rebase: {error}; resolve the remaining conflict in {} and continue or abort the rebase",
        worktree.display()
    ))
}

fn commit_rebased_gitlink(
    worktree: &Path,
    submodule: &str,
) -> Result<(), String> {
    let repository =
        Repository::open(worktree).map_err(|error| error.to_string())?;
    let parent = repository
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|error| error.to_string())?;
    let mut index = repository.index().map_err(|error| error.to_string())?;
    index
        .add_path(Path::new(submodule))
        .map_err(|error| error.to_string())?;
    let tree = repository
        .find_tree(index.write_tree().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    if tree.id() == parent.tree_id() {
        return Ok(());
    }
    index.write().map_err(|error| error.to_string())?;
    let signature =
        repository.signature().map_err(|error| error.to_string())?;
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            &format!("rebase submodule {submodule} onto local main"),
            &tree,
            &[&parent],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn merge_ff_only(
    repository: &Path,
    branch: &str,
) -> Result<(), String> {
    run_git(repository, ["merge", "--ff-only", branch]).map_err(|error| format!("merge --ff-only failed for {} from {branch}: {error}; rebase the feature branch onto local main and retry", repository.display()))
}
