#[allow(unused_imports)]
use super::*;

pub(super) fn handle_clean(selection: WorktreeSelection, dry_run: bool) -> Result<(), String> {
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    let activity =
        SessionStoreActivity::with_default_staleness(canonical_session_store(git.main_checkout()));
    let registered = selected_worktrees(&git, &selection)?;
    let mut removable = Vec::new();
    for worktree in registered {
        if worktree_relative_path(&git, &worktree).is_err() {
            println!(
                "preserved path={} reason=outside-worktree-root",
                worktree.path.display()
            );
            continue;
        }
        if activity.is_active(&worktree.path) {
            println!(
                "preserved path={} reason=session-active",
                worktree.path.display()
            );
            continue;
        }
        checkpoint_owned_session_changes(&git, &worktree, dry_run)?;
        match ensure_safe_to_remove(&git, &worktree) {
            Ok(()) => removable.push(worktree),
            Err(reason) => println!("preserved path={} reason={reason}", worktree.path.display()),
        }
    }
    let mut removable_debris = Vec::new();
    if selection.all {
        let registered = git.list_worktrees().map_err(|error| error.to_string())?;
        for path in unregistered_worktree_debris(&git, &registered)? {
            let mut entries = std::fs::read_dir(&path).map_err(|error| error.to_string())?;
            if entries.next().is_none() {
                removable_debris.push(path);
            } else {
                println!(
                    "preserved path={} reason=unregistered-debris-not-empty",
                    path.display()
                );
            }
        }
    }
    if dry_run {
        for worktree in &removable {
            println!("[dry-run] remove {} with force", worktree.path.display());
        }
        for path in &removable_debris {
            println!(
                "[dry-run] remove empty unregistered debris {}",
                path.display()
            );
        }
        return Ok(());
    }
    for worktree in &removable {
        git.worktree_remove_force(&worktree.path)
            .map_err(|error| error.to_string())?;
        remove_empty_nested_parent(git.main_checkout(), &worktree.path)?;
    }
    if !removable.is_empty() {
        git.worktree_prune().map_err(|error| error.to_string())?;
    }
    for path in &removable_debris {
        std::fs::remove_dir(path).map_err(|error| error.to_string())?;
    }
    println!("clean: removed {} safe worktree(s)", removable.len());
    if !removable_debris.is_empty() {
        println!(
            "clean: removed {} empty unregistered debris directory(s)",
            removable_debris.len()
        );
    }
    Ok(())
}

pub(super) fn ensure_safe_to_remove(
    git: &WorktreeGit,
    worktree: &session_worktree_provision::WorktreeRef,
) -> Result<(), String> {
    for submodule in git.submodule_paths().map_err(|error| error.to_string())? {
        let path = worktree.path.join(&submodule);
        if !path.is_dir() {
            return Err(format!("submodule {submodule} is not initialized"));
        }
        if git.is_dirty(&path).map_err(|error| error.to_string())? {
            return Err(format!("submodule {submodule} has uncommitted changes"));
        }
        let ahead = git
            .ahead_behind(&path, "main")
            .map_err(|error| error.to_string())?
            .0;
        if ahead != 0 {
            return Err(format!(
                "submodule {submodule} is {ahead} commits ahead of main"
            ));
        }
    }
    if git
        .is_dirty(&worktree.path)
        .map_err(|error| error.to_string())?
    {
        return Err("superproject has uncommitted changes".to_owned());
    }
    let ahead = git
        .ahead_behind(&worktree.path, "main")
        .map_err(|error| error.to_string())?
        .0;
    if ahead != 0 {
        return Err(format!("superproject is {ahead} commits ahead of main"));
    }
    Ok(())
}

pub(super) fn handle_remove(name: &str, force: bool, dry_run: bool) -> Result<(), String> {
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    let worktree = find_worktree(&git, name)?;
    if !force {
        ensure_safe_to_remove(&git, &worktree)?;
    }

    let mut plan = LifecyclePlan::default();
    plan.add(format!("remove {} with force", worktree.path.display()));
    plan.add("prune removed worktree registrations");
    if nested_worktree_parent(git.main_checkout(), &worktree.path).is_some() {
        plan.add(format!(
            "remove the session directory if {} is empty",
            worktree
                .path
                .parent()
                .expect("worktree has a parent")
                .display()
        ));
    }
    if dry_run {
        plan.emit();
        return Ok(());
    }

    git.worktree_remove_force(&worktree.path)
        .map_err(|error| error.to_string())?;
    git.worktree_prune().map_err(|error| error.to_string())?;
    remove_empty_nested_parent(git.main_checkout(), &worktree.path)
}

pub(super) fn handle_rename(
    source_name: &str,
    target_name: &str,
    dry_run: bool,
) -> Result<(), String> {
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    let source = find_worktree(&git, source_name)?;
    let source_relative = worktree_relative_path(&git, &source)?;
    let target_relative = rename_target_path(&source_relative, target_name)?;
    let target_path = git
        .main_checkout()
        .join(".worktrees")
        .join(&target_relative);
    let target_branch = branch_for_relative_path(&target_relative)?;
    let mut plan = LifecyclePlan::default();
    plan.add(format!(
        "move {} to {}, repair Git metadata, and rename its branch to {target_branch}",
        source.path.display(),
        target_path.display()
    ));
    if dry_run {
        plan.emit();
        return Ok(());
    }

    git.rename_worktree(&source.name, &target_relative, &target_branch)
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn handle_finish(name: &str, dry_run: bool) -> Result<(), String> {
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    let worktree = find_worktree(&git, name)?;
    let mut plan = LifecyclePlan::default();
    plan.add(format!(
        "rebase {} onto local main",
        worktree.path.display()
    ));
    plan.add(format!("remove {} with force", worktree.path.display()));
    plan.add("prune removed worktree registrations");
    plan.add(FINISH_READY_TO_MERGE_MARKER);
    if dry_run {
        plan.emit();
        return Ok(());
    }

    sync::rebase_onto_local_main(&worktree.path)?;
    git.worktree_remove_force(&worktree.path)
        .map_err(|error| error.to_string())?;
    git.worktree_prune().map_err(|error| error.to_string())?;
    remove_empty_nested_parent(git.main_checkout(), &worktree.path)?;
    println!("{FINISH_READY_TO_MERGE_MARKER}");
    Ok(())
}

pub(super) fn find_worktree(
    git: &WorktreeGit,
    name: &str,
) -> Result<session_worktree_provision::WorktreeRef, String> {
    let worktrees = git.list_worktrees().map_err(|error| error.to_string())?;
    if name.contains('/') {
        let relative_path = nested_relative_path(name)?;
        let path = git.main_checkout().join(".worktrees").join(relative_path);
        return worktrees
            .into_iter()
            .find(|worktree| worktree.path == path)
            .ok_or_else(|| format!("worktree '{name}' was not found"));
    }

    let mut matches = worktrees
        .into_iter()
        .filter(|worktree| worktree.name == name);
    let Some(worktree) = matches.next() else {
        return Err(format!("worktree '{name}' was not found"));
    };
    if matches.next().is_some() {
        return Err(format!(
            "ambiguous worktree name '{name}'; use <full-session-uuid>/<slug> for nested worktrees"
        ));
    }
    Ok(worktree)
}

pub(super) fn validate_full_session_uuid(session_uuid: &str) -> Result<(), String> {
    let valid = session_uuid.len() == 36
        && session_uuid.chars().enumerate().all(|(index, character)| {
            matches!(index, 8 | 13 | 18 | 23) && character == '-'
                || !matches!(index, 8 | 13 | 18 | 23) && character.is_ascii_hexdigit()
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "session UUID must be a full UUID such as 12345678-1234-1234-1234-123456789abc; short id '{session_uuid}' is not accepted"
        ))
    }
}

pub(super) fn nested_relative_path(name: &str) -> Result<PathBuf, String> {
    let mut parts = name.split('/');
    let session_uuid = parts.next().unwrap_or_default();
    let slug = parts.next().unwrap_or_default();
    if parts.next().is_some() || slug.is_empty() {
        return Err(format!(
            "nested worktree name '{name}' must be <full-session-uuid>/<slug>"
        ));
    }
    validate_full_session_uuid(session_uuid)?;
    Ok(Path::new(session_uuid).join(slug))
}

pub(super) fn worktree_relative_path(
    git: &WorktreeGit,
    worktree: &session_worktree_provision::WorktreeRef,
) -> Result<PathBuf, String> {
    worktree
        .path
        .strip_prefix(git.main_checkout().join(".worktrees"))
        .map(Path::to_path_buf)
        .map_err(|_| format!("worktree {} is outside .worktrees", worktree.path.display()))
}

pub(super) fn rename_target_path(
    source_relative: &Path,
    target_name: &str,
) -> Result<PathBuf, String> {
    if source_relative.components().count() == 2 {
        let target_relative = nested_relative_path(target_name)?;
        if target_relative.parent() != source_relative.parent() {
            return Err("nested worktree rename must keep the same full session UUID".to_owned());
        }
        return Ok(target_relative);
    }
    if target_name.contains('/') {
        return Err("legacy worktree rename target must be a flat name".to_owned());
    }
    Ok(PathBuf::from(target_name))
}

pub(super) fn branch_for_relative_path(relative_path: &Path) -> Result<String, String> {
    let value = relative_path
        .to_str()
        .ok_or("worktree path must be valid UTF-8")?;
    Ok(format!("agent/{}", value.replace('\\', "/")))
}

pub(super) fn nested_slug_directories(
    main_checkout: &Path,
    session_uuid: &str,
) -> Result<Vec<String>, String> {
    let parent = main_checkout.join(".worktrees").join(session_uuid);
    if !parent.is_dir() {
        return Ok(Vec::new());
    }
    let mut slugs = std::fs::read_dir(parent)
        .map_err(|error| error.to_string())?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry)
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    slugs.sort();
    Ok(slugs)
}

pub(super) fn nested_worktree_parent(
    main_checkout: &Path,
    worktree_path: &Path,
) -> Option<PathBuf> {
    let relative = worktree_path
        .strip_prefix(main_checkout.join(".worktrees"))
        .ok()?;
    if relative.components().count() == 2 {
        worktree_path.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

pub(super) fn remove_empty_nested_parent(
    main_checkout: &Path,
    worktree_path: &Path,
) -> Result<(), String> {
    let Some(parent) = nested_worktree_parent(main_checkout, worktree_path) else {
        return Ok(());
    };
    if parent.is_dir()
        && std::fs::read_dir(&parent)
            .map_err(|error| error.to_string())?
            .next()
            .is_none()
    {
        std::fs::remove_dir(parent).map_err(|error| error.to_string())?;
    }
    Ok(())
}
