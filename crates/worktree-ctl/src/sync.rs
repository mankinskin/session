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
    checkpoint_owned_session_changes,
    checkpoint_session_mirror_changes,
    find_worktree,
    git::{
        git_command,
        run_git,
    },
    gitlink,
};

const AUTOSTASH_MESSAGE: &str = "worktree-ctl autostash";
const AUTO_COMMIT_MESSAGE: &str = "worktree-ctl auto-commit before sync";
const REBASED_GITLINK_BASE_COMMIT_PREFIX: &str =
    "rebase submodule bases onto local main";
const REBASED_GITLINK_TIP_COMMIT_PREFIX: &str =
    "rebase submodule tips onto local main";
const REBASED_GITLINK_BRIDGE_PREFIX: &str = "worktree-ctl-rebase-bridge";

#[derive(Debug)]
struct RebasedGitlink {
    path: String,
    old: Oid,
    base: Oid,
    tip: Oid,
}

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
    checkpoint_owned_session_changes(&git, &worktree, dry_run)?;
    let branch = worktree.branch.as_deref().ok_or_else(|| {
        format!("worktree {name} is detached and cannot be rebased")
    })?;
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
                println!(
                    "skip {submodule} because branch {branch} does not exist"
                );
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
    checkpoint_owned_session_changes(&git, &worktree, dry_run)?;
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
    let git = WorktreeGit::open(
        env::current_dir().map_err(|error| error.to_string())?,
    )
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
    plan.add(format!("stash uncommitted changes in {label} before mutating (restored afterward)"));
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

    if resolve_bridged_gitlink_conflicts(worktree)?
        || resolve_redundant_gitlink_conflicts(worktree)?
    {
        return continue_or_skip_rebase(worktree);
    }

    Err(format!(
        "git rebase main failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn resolve_bridged_gitlink_conflicts(worktree: &Path) -> Result<bool, String> {
    let bridges = recorded_gitlink_bridges(worktree)?;
    if bridges.is_empty() {
        return Ok(false);
    }
    let paths = conflicted_paths(worktree)?;
    if paths.is_empty() {
        return Ok(false);
    }

    for path in &paths {
        let (ours, theirs) = conflicted_gitlink_tips(worktree, path)?
            .ok_or_else(|| "conflict is not a gitlink".to_owned())?;
        let bridge = bridges.iter().find(|bridge| {
            bridge.path == *path && bridge.old == theirs && bridge.base == ours
        });
        let Some(bridge) = bridge else {
            return Ok(false);
        };
        let repository = Repository::open(worktree.join(path))
            .map_err(|error| error.to_string())?;
        if !repository
            .graph_descendant_of(bridge.tip, bridge.base)
            .map_err(|error| error.to_string())?
        {
            return Ok(false);
        }
    }

    for path in paths {
        run_git(worktree, ["checkout", "--ours", "--", &path])?;
        run_git(worktree, ["add", "--", &path])?;
        println!(
            "resolved stale controller gitlink checkpoint in {path}: retained the recorded rebased base"
        );
    }
    Ok(true)
}

fn reset_redundant_gitlink_only_branch(
    worktree: &Path
) -> Result<bool, String> {
    let output = git_command(worktree)
        .args(["diff", "--raw", "--no-abbrev", "main", "HEAD"])
        .output()
        .map_err(|error| {
            format!("failed to inspect gitlink rebase candidates: {error}")
        })?;
    if !output.status.success() {
        return Err(format!(
            "failed to inspect gitlink rebase candidates: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let changes = String::from_utf8_lossy(&output.stdout);
    let changes = changes
        .lines()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if changes.is_empty() {
        return Ok(false);
    }

    for change in &changes {
        let fields = change.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 6 || fields[0] != ":160000" || fields[1] != "160000" {
            return Ok(false);
        }
        let main =
            Oid::from_str(fields[2]).map_err(|error| error.to_string())?;
        let branch =
            Oid::from_str(fields[3]).map_err(|error| error.to_string())?;
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

fn resolve_redundant_gitlink_conflicts(
    worktree: &Path
) -> Result<bool, String> {
    let paths = conflicted_paths(worktree)?;
    if paths.is_empty() {
        return Ok(false);
    }

    for path in &paths {
        let Some((ours, theirs)) = conflicted_gitlink_tips(worktree, path)?
        else {
            return Ok(false);
        };
        let repository = match Repository::open(worktree.join(path)) {
            Ok(repository) => repository,
            Err(_) => return Ok(false),
        };
        if !repository
            .graph_descendant_of(ours, theirs)
            .map_err(|error| error.to_string())?
        {
            return Ok(false);
        }
    }

    for path in paths {
        run_git(worktree, ["checkout", "--ours", "--", &path])?;
        run_git(worktree, ["add", "--", &path])?;
        println!(
            "resolved redundant gitlink conflict in {path}: retained the rebase target's descendant commit"
        );
    }
    Ok(true)
}

fn conflicted_paths(worktree: &Path) -> Result<Vec<String>, String> {
    let output = git_command(worktree)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output()
        .map_err(|error| {
            format!("failed to inspect rebase conflicts: {error}")
        })?;
    if !output.status.success() {
        return Err(format!(
            "failed to inspect rebase conflicts: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}

fn conflicted_gitlink_tips(
    worktree: &Path,
    path: &str,
) -> Result<Option<(Oid, Oid)>, String> {
    let entries = git_command(worktree)
        .args(["ls-files", "-u", "--", path])
        .output()
        .map_err(|error| {
            format!("failed to inspect conflicted gitlink {path}: {error}")
        })?;
    if !entries.status.success() {
        return Err(format!(
            "failed to inspect conflicted gitlink {path}: {}",
            String::from_utf8_lossy(&entries.stderr).trim()
        ));
    }
    let mut ours = None;
    let mut theirs = None;
    for entry in String::from_utf8_lossy(&entries.stdout).lines() {
        let fields = entry.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3 || fields[0] != "160000" {
            return Ok(None);
        }
        match fields[2] {
            "2" =>
                ours = Some(
                    Oid::from_str(fields[1])
                        .map_err(|error| error.to_string())?,
                ),
            "3" =>
                theirs = Some(
                    Oid::from_str(fields[1])
                        .map_err(|error| error.to_string())?,
                ),
            _ => {},
        }
    }
    Ok(ours.zip(theirs))
}

fn continue_or_skip_rebase(worktree: &Path) -> Result<(), String> {
    let staged = git_command(worktree)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map_err(|error| {
            format!("failed to inspect resolved rebase: {error}")
        })?;
    let command = if staged.success() {
        ["rebase", "--skip"]
    } else if staged.code() == Some(1) {
        ["rebase", "--continue"]
    } else {
        return Err("failed to inspect resolved rebase".to_owned());
    };
    let output = git_command(worktree)
        .env("GIT_EDITOR", "true")
        .args(command)
        .output()
        .map_err(|error| {
            format!("failed to continue automatic rebase resolution: {error}")
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "automatic redundant gitlink resolution could not finish the rebase: {}; resolve the remaining conflict in {} and continue or abort the rebase",
            String::from_utf8_lossy(&output.stderr).trim(),
            worktree.display()
        ))
    }
}

fn commit_rebased_gitlink_bridges(
    worktree: &Path,
    bridges: &[RebasedGitlink],
) -> Result<(), String> {
    if bridges.is_empty() {
        return Ok(());
    }
    for bridge in bridges {
        let cache_info = format!("160000,{},{}", bridge.base, bridge.path);
        run_git(
            worktree,
            ["update-index", "--add", "--cacheinfo", &cache_info],
        )?;
    }
    commit_index_if_changed(
        worktree,
        &bridge_message(REBASED_GITLINK_BASE_COMMIT_PREFIX, bridges),
    )?;
    for bridge in bridges {
        run_git(worktree, ["add", "--", &bridge.path])?;
    }
    commit_index_if_changed(
        worktree,
        &bridge_message(REBASED_GITLINK_TIP_COMMIT_PREFIX, bridges),
    )?;
    Ok(())
}

fn discard_current_gitlink_bridge_pair(worktree: &Path) -> Result<(), String> {
    let repository =
        Repository::open(worktree).map_err(|error| error.to_string())?;
    let head = repository
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|error| error.to_string())?;
    if !head.message().is_some_and(|message| {
        message.starts_with(REBASED_GITLINK_TIP_COMMIT_PREFIX)
    }) {
        return Ok(());
    }
    let parent = head.parent(0).map_err(|error| error.to_string())?;
    if parent.message().is_some_and(|message| {
        message.starts_with(REBASED_GITLINK_BASE_COMMIT_PREFIX)
    }) {
        run_git(worktree, ["reset", "--soft", "HEAD^^"])?;
        run_git(worktree, ["reset"])?;
        println!("replaced previous generated gitlink bridge checkpoint");
    }
    Ok(())
}

fn commit_index_if_changed(
    worktree: &Path,
    message: &str,
) -> Result<(), String> {
    let status = git_command(worktree)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map_err(|error| {
            format!("failed to inspect generated gitlink checkpoint: {error}")
        })?;
    if status.success() {
        return Ok(());
    }
    if status.code() != Some(1) {
        return Err("failed to inspect generated gitlink checkpoint".to_owned());
    }
    run_git(worktree, ["commit", "-m", message])
}

fn bridge_message(
    prefix: &str,
    bridges: &[RebasedGitlink],
) -> String {
    let records = bridges
        .iter()
        .map(|bridge| {
            format!(
                "{REBASED_GITLINK_BRIDGE_PREFIX} path={} old={} base={} tip={}",
                bridge.path, bridge.old, bridge.base, bridge.tip
            )
        })
        .collect::<Vec<_>>();
    format!("{prefix}\n\n{}", records.join("\n"))
}

fn recorded_gitlink_bridges(
    worktree: &Path
) -> Result<Vec<RebasedGitlink>, String> {
    let output = git_command(worktree)
        .args(["log", "--format=%B", "ORIG_HEAD"])
        .output()
        .map_err(|error| {
            format!("failed to inspect original rebase history: {error}")
        })?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_bridge_record)
        .collect()
}

fn parse_bridge_record(line: &str) -> Option<Result<RebasedGitlink, String>> {
    let mut fields = line.split_whitespace();
    if fields.next()? != REBASED_GITLINK_BRIDGE_PREFIX {
        return None;
    }
    let mut path = None;
    let mut old = None;
    let mut base = None;
    let mut tip = None;
    for field in fields {
        let (key, value) = field.split_once('=')?;
        match key {
            "path" => path = Some(value.to_owned()),
            "old" => match Oid::from_str(value) {
                Ok(value) => old = Some(value),
                Err(error) => return Some(Err(error.to_string())),
            },
            "base" => match Oid::from_str(value) {
                Ok(value) => base = Some(value),
                Err(error) => return Some(Err(error.to_string())),
            },
            "tip" => match Oid::from_str(value) {
                Ok(value) => tip = Some(value),
                Err(error) => return Some(Err(error.to_string())),
            },
            _ => {},
        }
    }
    Some(match (path, old, base, tip) {
        (Some(path), Some(old), Some(base), Some(tip)) => Ok(RebasedGitlink {
            path,
            old,
            base,
            tip,
        }),
        _ => Err("incomplete generated gitlink bridge record".to_owned()),
    })
}

fn gitlink_at_head(
    worktree: &Path,
    path: &str,
) -> Result<Oid, String> {
    let repository =
        Repository::open(worktree).map_err(|error| error.to_string())?;
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
    let repository =
        Repository::open(repository).map_err(|error| error.to_string())?;
    repository
        .find_branch("main", BranchType::Local)
        .and_then(|branch| branch.get().peel_to_commit())
        .map(|commit| commit.id())
        .map_err(|error| error.to_string())
}

fn head_tip(repository: &Path) -> Result<Oid, String> {
    let repository =
        Repository::open(repository).map_err(|error| error.to_string())?;
    repository
        .head()
        .and_then(|head| head.peel_to_commit())
        .map(|commit| commit.id())
        .map_err(|error| error.to_string())
}

fn merge_ff_only(
    repository: &Path,
    branch: &str,
) -> Result<(), String> {
    run_git(repository, ["merge", "--ff-only", branch]).map_err(|error| format!("merge --ff-only failed for {} from {branch}: {error}; rebase the feature branch onto local main and retry", repository.display()))
}
