#[allow(unused_imports)]
use super::*;

pub(super) fn handle_new(
    session_uuid: &str,
    slug: &str,
    dry_run: bool,
    preserve_main_changes: bool,
) -> Result<(), String> {
    validate_full_session_uuid(session_uuid)?;
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(&main_checkout).map_err(|error| error.to_string())?;
    let relative_path = Path::new(session_uuid).join(slug);
    let branch = format!("agent/{session_uuid}/{slug}");
    let worktree_path = git.main_checkout().join(".worktrees").join(&relative_path);
    let worktrees = git.list_worktrees().map_err(|error| error.to_string())?;

    if let Some(worktree) = worktrees
        .iter()
        .find(|worktree| worktree.path == worktree_path)
    {
        println!("{WORKTREE_PATH_OUTPUT_PREFIX}{}", worktree.path.display());
        return Ok(());
    }

    let nested_slugs = nested_slug_directories(git.main_checkout(), session_uuid)?;
    if !nested_slugs.is_empty() {
        return Err(format!(
            "ambiguous session worktree for {session_uuid}: nested slug directories already exist: {}; exactly one active slug is allowed",
            nested_slugs.join(", ")
        ));
    }

    let dirty_paths = git
        .dirty_paths(git.main_checkout())
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|path| !path.path.starts_with(".worktrees"))
        .collect::<Vec<_>>();
    if !dirty_paths.is_empty() && !preserve_main_changes {
        let paths = dirty_paths
            .iter()
            .map(|path| path.path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "{DIRTY_MAIN_UNCOMMITTED_CHANGES_MESSAGE} in main checkout: {paths}; pass --{PRESERVE_MAIN_CHANGES_HINT} to stash them"
        ));
    }

    let mut plan = LifecyclePlan::default();
    if !dirty_paths.is_empty() {
        let paths = dirty_paths
            .iter()
            .map(|path| path.path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        plan.add(format!(
            "stash main-checkout changes ({paths}) with {PRESERVE_MAIN_CHANGES_HINT}"
        ));
    }
    plan.add(format!(
        "create {} from local main on branch {branch}",
        worktree_path.display()
    ));
    for submodule in git.submodule_paths().map_err(|error| error.to_string())? {
        plan.add(format!(
            "populate {} from its recorded local gitlink",
            worktree_path.join(submodule).display()
        ));
    }

    if dry_run {
        plan.emit();
        return Ok(());
    }

    if !dirty_paths.is_empty() {
        let paths = dirty_paths
            .iter()
            .map(|path| path.path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("preserving main-checkout changes: {paths}");
        git.stash_push(PRESERVE_MAIN_CHANGES_HINT)
            .map_err(|error| error.to_string())?;
    }
    let worktree = git
        .create_worktree_at(&relative_path, &branch, "main")
        .map_err(|error| error.to_string())?;
    println!("{WORKTREE_PATH_OUTPUT_PREFIX}{}", worktree.path.display());
    Ok(())
}

pub(super) fn handle_bootstrap(
    session_uuid: &str,
    slug: &str,
    dry_run: bool,
    preserve_main_changes: bool,
) -> Result<(), String> {
    handle_new(session_uuid, slug, dry_run, preserve_main_changes)?;

    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    let worktree_path = git
        .main_checkout()
        .join(".worktrees")
        .join(session_uuid)
        .join(slug);

    if dry_run {
        println!(
            "[dry-run] initialize repository stores and Copilot surfaces in {} with init.sh",
            worktree_path.display()
        );
        return Ok(());
    }

    let init_script = worktree_path.join("init.sh");
    if !init_script.is_file() {
        return Err(format!(
            "worktree initializer is missing at {}; repair the worktree and rerun bootstrap",
            init_script.display()
        ));
    }

    let status = ProcessCommand::new("bash")
        .arg("init.sh")
        .current_dir(&worktree_path)
        .status()
        .map_err(|error| format!("could not run {}: {error}", init_script.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "worktree initializer failed in {}; repair the worktree and rerun bootstrap",
            worktree_path.display()
        ))
    }
}
