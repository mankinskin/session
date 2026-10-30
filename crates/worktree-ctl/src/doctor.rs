#[allow(unused_imports)]
use super::*;

pub(super) fn handle_doctor(dry_run: bool) -> Result<(), String> {
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(&main_checkout).map_err(|error| error.to_string())?;
    let mut plan = LifecyclePlan::default();

    scan_submodules_for_repairs(&git, &mut plan)?;
    plan.add("prune stale superproject worktree registrations");
    if dry_run {
        plan.emit();
        return Ok(());
    }

    repair_submodules(&git)?;
    git.worktree_prune().map_err(|error| error.to_string())?;
    println!("doctor: repairs complete");
    Ok(())
}

/// Recurse into each already-initialized submodule so a deinitialized
/// submodule-of-a-submodule (e.g. `workflow-tools`'s own `spec`/`session`)
/// is reported too, not just direct submodules of the invoked checkout.
pub(super) fn scan_submodules_for_repairs(
    git: &WorktreeGit,
    plan: &mut LifecyclePlan,
) -> Result<(), String> {
    for submodule in git.submodule_paths().map_err(|error| error.to_string())? {
        let path = git.main_checkout().join(&submodule);
        if let Some(config) = stale_worktree_config(git.main_checkout(), &submodule)? {
            println!(
                "submodule={submodule} status=stale-core-worktree path={}",
                config.display()
            );
            plan.add(format!(
                "unset stale core.worktree for submodule {submodule}"
            ));
            plan.add(format!(
                "prune nested worktree registrations for submodule {submodule}"
            ));
        }
        if Repository::open(&path).is_err() {
            println!("submodule={submodule} status=deinitialized");
            plan.add(format!(
                "initialize and update deinitialized submodule {submodule}"
            ));
            continue;
        }
        if let Ok(nested_git) = WorktreeGit::open(&path) {
            scan_submodules_for_repairs(&nested_git, plan)?;
        }
    }
    Ok(())
}

/// Repair pass matching `scan_submodules_for_repairs`: recurses into each
/// submodule only after it is confirmed initialized, since a deinitialized
/// submodule has no `.gitmodules` of its own to discover yet.
pub(super) fn repair_submodules(git: &WorktreeGit) -> Result<(), String> {
    for submodule in git.submodule_paths().map_err(|error| error.to_string())? {
        let path = git.main_checkout().join(&submodule);
        if stale_worktree_config(git.main_checkout(), &submodule)?.is_some() {
            unset_core_worktree(git.main_checkout(), &submodule)?;
            git::run_git(&path, ["worktree", "prune"])?;
        }
        if Repository::open(&path).is_err() {
            initialize_submodule(git.main_checkout(), &submodule)?;
        }
        if let Ok(nested_git) = WorktreeGit::open(&path) {
            repair_submodules(&nested_git)?;
        }
    }
    Ok(())
}

pub(super) fn lifecycle_status(
    git: &WorktreeGit,
    activity: &SessionStoreActivity,
    worktree: &session_worktree_provision::WorktreeRef,
    policy: &ProvisionPolicy,
) -> Result<String, String> {
    match evaluate_reclaim_candidate(git, activity, worktree, policy)
        .map_err(|error| error.to_string())?
    {
        ReclaimEligibility::Reclaimable => Ok("reclaimable".to_owned()),
        ReclaimEligibility::Rejected(reason) => {
            Ok(format!("preserved reason={}", rejection_reason(&reason)))
        }
    }
}

pub(super) fn rejection_reason(reason: &ReclaimRejectionReason) -> String {
    match reason {
        ReclaimRejectionReason::OutsideWorktreeRoot => "outside-worktree-root".to_owned(),
        ReclaimRejectionReason::SessionActive => "session-active".to_owned(),
        ReclaimRejectionReason::Detached => "detached".to_owned(),
        ReclaimRejectionReason::Dirty => "dirty".to_owned(),
        ReclaimRejectionReason::ContainsCurrentDirectory => "contains-current-directory".to_owned(),
        ReclaimRejectionReason::NotIdle => "not-idle".to_owned(),
        ReclaimRejectionReason::DirtySubmodule { path } => {
            format!("dirty-submodule:{}", path.display())
        }
        ReclaimRejectionReason::AheadOfMain => "ahead-of-main".to_owned(),
    }
}

pub(super) fn stale_worktree_config(
    main_checkout: &Path,
    submodule: &str,
) -> Result<Option<PathBuf>, String> {
    let config_path = main_checkout
        .join(".git")
        .join("modules")
        .join(submodule)
        .join("config");
    if !config_path.exists() {
        return Ok(None);
    }
    let config = git2::Config::open(&config_path).map_err(|error| error.to_string())?;
    let value = match config.get_string("core.worktree") {
        Ok(value) => value,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let configured_path = PathBuf::from(value);
    let resolved_path = if configured_path.is_absolute() {
        configured_path
    } else {
        config_path
            .parent()
            .ok_or("submodule config has no parent")?
            .join(configured_path)
    };
    Ok((!resolved_path.exists()).then_some(resolved_path))
}

pub(super) fn initialize_submodule(main_checkout: &Path, submodule: &str) -> Result<(), String> {
    let repository = Repository::open(main_checkout).map_err(|error| error.to_string())?;
    let mut handle = repository
        .find_submodule(submodule)
        .map_err(|error| error.to_string())?;
    handle.init(true).map_err(|error| error.to_string())?;
    handle.update(true, None).map_err(|error| error.to_string())
}

pub(super) fn unset_core_worktree(main_checkout: &Path, submodule: &str) -> Result<(), String> {
    let config = main_checkout
        .join(".git")
        .join("modules")
        .join(submodule)
        .join("config");
    let mut config = git2::Config::open(&config).map_err(|error| error.to_string())?;
    config
        .remove("core.worktree")
        .map_err(|error| error.to_string())
}
