#[allow(unused_imports)]
use super::*;

pub(super) fn ensure_submodule_branch_rebased(
    path: &Path,
    branch: &str,
    submodule: &str,
) -> Result<(), String> {
    let repository = Repository::open(path).map_err(|error| error.to_string())?;
    let feature = repository
        .find_branch(branch, BranchType::Local)
        .map_err(|error| error.to_string())?
        .get()
        .target()
        .ok_or_else(|| format!("submodule {submodule} branch {branch} has no target"))?;
    let main = repository
        .find_branch("main", BranchType::Local)
        .map_err(|error| error.to_string())?
        .get()
        .target()
        .ok_or_else(|| format!("submodule {submodule} main has no target"))?;
    if feature == main
        || repository
            .graph_descendant_of(feature, main)
            .map_err(|error| error.to_string())?
    {
        Ok(())
    } else {
        Err(format!(
            "submodule {submodule} branch {branch} ({feature}) is not rebased onto local main ({main}); run `worktree-ctl rebase` before merging the superproject"
        ))
    }
}

pub(super) fn checkout_and_rebase(worktree: &Path, branch: &str) -> Result<(), String> {
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

pub(super) fn resolve_bridged_gitlink_conflicts(worktree: &Path) -> Result<bool, String> {
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
        let bridge = bridges
            .iter()
            .find(|bridge| bridge.path == *path && bridge.old == theirs && bridge.base == ours);
        let Some(bridge) = bridge else {
            return Ok(false);
        };
        let repository =
            Repository::open(worktree.join(path)).map_err(|error| error.to_string())?;
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

pub(super) fn reset_redundant_gitlink_only_branch(worktree: &Path) -> Result<bool, String> {
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

pub(super) fn resolve_redundant_gitlink_conflicts(worktree: &Path) -> Result<bool, String> {
    let paths = conflicted_paths(worktree)?;
    if paths.is_empty() {
        return Ok(false);
    }

    for path in &paths {
        let Some((ours, theirs)) = conflicted_gitlink_tips(worktree, path)? else {
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

pub(super) fn conflicted_paths(worktree: &Path) -> Result<Vec<String>, String> {
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
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}

pub(super) fn conflicted_gitlink_tips(
    worktree: &Path,
    path: &str,
) -> Result<Option<(Oid, Oid)>, String> {
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
    for entry in String::from_utf8_lossy(&entries.stdout).lines() {
        let fields = entry.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3 || fields[0] != "160000" {
            return Ok(None);
        }
        match fields[2] {
            "2" => ours = Some(Oid::from_str(fields[1]).map_err(|error| error.to_string())?),
            "3" => theirs = Some(Oid::from_str(fields[1]).map_err(|error| error.to_string())?),
            _ => {}
        }
    }
    Ok(ours.zip(theirs))
}

pub(super) fn continue_or_skip_rebase(worktree: &Path) -> Result<(), String> {
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
    let output = git_command(worktree)
        .env("GIT_EDITOR", "true")
        .args(command)
        .output()
        .map_err(|error| format!("failed to continue automatic rebase resolution: {error}"))?;
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

pub(super) fn commit_rebased_gitlink_bridges(
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

pub(super) fn discard_current_gitlink_bridge_pair(worktree: &Path) -> Result<(), String> {
    let repository = Repository::open(worktree).map_err(|error| error.to_string())?;
    let head = repository
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|error| error.to_string())?;
    if !head
        .message()
        .is_some_and(|message| message.starts_with(REBASED_GITLINK_TIP_COMMIT_PREFIX))
    {
        return Ok(());
    }
    let parent = head.parent(0).map_err(|error| error.to_string())?;
    if parent
        .message()
        .is_some_and(|message| message.starts_with(REBASED_GITLINK_BASE_COMMIT_PREFIX))
    {
        run_git(worktree, ["reset", "--soft", "HEAD^^"])?;
        run_git(worktree, ["reset"])?;
        println!("replaced previous generated gitlink bridge checkpoint");
    }
    Ok(())
}

pub(super) fn commit_index_if_changed(worktree: &Path, message: &str) -> Result<(), String> {
    let status = git_command(worktree)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map_err(|error| format!("failed to inspect generated gitlink checkpoint: {error}"))?;
    if status.success() {
        return Ok(());
    }
    if status.code() != Some(1) {
        return Err("failed to inspect generated gitlink checkpoint".to_owned());
    }
    run_git(worktree, ["commit", "-m", message])
}

pub(super) fn bridge_message(prefix: &str, bridges: &[RebasedGitlink]) -> String {
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

pub(super) fn recorded_gitlink_bridges(worktree: &Path) -> Result<Vec<RebasedGitlink>, String> {
    let output = git_command(worktree)
        .args(["log", "--format=%B", "ORIG_HEAD"])
        .output()
        .map_err(|error| format!("failed to inspect original rebase history: {error}"))?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_bridge_record)
        .collect()
}

pub(super) fn parse_bridge_record(line: &str) -> Option<Result<RebasedGitlink, String>> {
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
            _ => {}
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
