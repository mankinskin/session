mod git;
mod gitlink;
mod sync;

use std::{
    env,
    path::{
        Path,
        PathBuf,
    },
    process::Command as ProcessCommand,
};

use clap::{
    Parser,
    Subcommand,
};
use git2::Repository;
use session_worktree_provision::{
    ReclaimEligibility,
    ReclaimRejectionReason,
    SessionStoreActivity,
    WorktreeGit,
    evaluate_reclaim_candidate,
    policy::ProvisionPolicy,
};

const WORKTREE_PATH_OUTPUT_PREFIX: &str = "WORKTREE_PATH=";
const FINISH_READY_TO_MERGE_MARKER: &str = "ready-to-merge";
const DIRTY_MAIN_UNCOMMITTED_CHANGES_MESSAGE: &str = "uncommitted changes";
const PRESERVE_MAIN_CHANGES_HINT: &str = "preserve-main-changes";
#[cfg(test)]
const WORKTREE_PATH_TEMPLATE: &str = ".worktrees/<full-session-uuid>/<slug>";
#[cfg(test)]
const BRANCH_TEMPLATE: &str = "agent/<full-session-uuid>/<slug>";

#[derive(Debug, Parser)]
#[command(
    name = "worktree-ctl",
    about = "Manage local Git worktree lifecycles"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
enum Command {
    New {
        session_uuid: String,
        slug: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        preserve_main_changes: bool,
    },
    Bootstrap {
        session_uuid: String,
        slug: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        preserve_main_changes: bool,
    },
    List {
        #[arg(long)]
        dry_run: bool,
    },
    Rebase {
        name: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        auto_commit: bool,
    },
    Merge {
        name: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        auto_commit: bool,
    },
    Sync {
        name: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        auto_commit: bool,
    },
    Remove {
        name: String,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },
    Rename {
        source_name: String,
        target_name: String,
        #[arg(long)]
        dry_run: bool,
    },
    Finish {
        name: String,
        #[arg(long)]
        dry_run: bool,
    },
    Doctor {
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = dispatch(cli.command) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn dispatch(command: Command) -> Result<(), String> {
    match command {
        Command::New {
            session_uuid,
            slug,
            dry_run,
            preserve_main_changes,
        } => handle_new(&session_uuid, &slug, dry_run, preserve_main_changes),
        Command::Bootstrap {
            session_uuid,
            slug,
            dry_run,
            preserve_main_changes,
        } => handle_bootstrap(
            &session_uuid,
            &slug,
            dry_run,
            preserve_main_changes,
        ),
        Command::List { dry_run } => handle_list(dry_run),
        Command::Rebase {
            name,
            dry_run,
            auto_commit,
        } => sync::handle_rebase(&name, dry_run, auto_commit),
        Command::Merge {
            name,
            dry_run,
            auto_commit,
        } => sync::handle_merge(&name, dry_run, auto_commit),
        Command::Sync {
            name,
            dry_run,
            auto_commit,
        } => sync::handle_sync(&name, dry_run, auto_commit),
        Command::Remove {
            name,
            force,
            dry_run,
        } => handle_remove(&name, force, dry_run),
        Command::Rename {
            source_name,
            target_name,
            dry_run,
        } => handle_rename(&source_name, &target_name, dry_run),
        Command::Finish { name, dry_run } => handle_finish(&name, dry_run),
        Command::Doctor { dry_run } => handle_doctor(dry_run),
    }
}

#[derive(Default)]
struct LifecyclePlan {
    actions: Vec<String>,
}

impl LifecyclePlan {
    fn add(
        &mut self,
        action: impl Into<String>,
    ) {
        self.actions.push(action.into());
    }

    fn emit(&self) {
        for action in &self.actions {
            println!("[dry-run] {action}");
        }
    }
}

fn handle_new(
    session_uuid: &str,
    slug: &str,
    dry_run: bool,
    preserve_main_changes: bool,
) -> Result<(), String> {
    validate_full_session_uuid(session_uuid)?;
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(&main_checkout).map_err(|error| error.to_string())?;
    let relative_path = Path::new(session_uuid).join(slug);
    let branch = format!("agent/{session_uuid}/{slug}");
    let worktree_path =
        git.main_checkout().join(".worktrees").join(&relative_path);
    let worktrees = git.list_worktrees().map_err(|error| error.to_string())?;

    if let Some(worktree) = worktrees
        .iter()
        .find(|worktree| worktree.path == worktree_path)
    {
        println!("{WORKTREE_PATH_OUTPUT_PREFIX}{}", worktree.path.display());
        return Ok(());
    }

    let nested_slugs =
        nested_slug_directories(git.main_checkout(), session_uuid)?;
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

fn handle_bootstrap(
    session_uuid: &str,
    slug: &str,
    dry_run: bool,
    preserve_main_changes: bool,
) -> Result<(), String> {
    handle_new(session_uuid, slug, dry_run, preserve_main_changes)?;

    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
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
        .map_err(|error| {
            format!("could not run {}: {error}", init_script.display())
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "worktree initializer failed in {}; repair the worktree and rerun bootstrap",
            worktree_path.display()
        ))
    }
}

fn handle_list(_dry_run: bool) -> Result<(), String> {
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(&main_checkout).map_err(|error| error.to_string())?;
    let activity = SessionStoreActivity::with_default_staleness(
        git.main_checkout().join(".session"),
    );
    let policy = ProvisionPolicy::default();
    let registered = git.list_worktrees().map_err(|error| error.to_string())?;

    for worktree in &registered {
        let submodules = submodule_status(&git, &worktree.path)?;
        let lifecycle = lifecycle_status(&git, &activity, worktree, &policy)?;
        println!(
            "path={} branch={} submodules={} lifecycle={}",
            worktree.path.display(),
            worktree.branch.as_deref().unwrap_or("detached"),
            submodules,
            lifecycle
        );
    }

    let worktree_root = git.main_checkout().join(".worktrees");
    if worktree_root.is_dir() {
        for entry in std::fs::read_dir(&worktree_root)
            .map_err(|error| error.to_string())?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if path.is_dir()
                && !registered.iter().any(|worktree| {
                    worktree.path == path || worktree.path.starts_with(&path)
                })
            {
                println!(
                    "path={} lifecycle=unregistered-debris",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

fn handle_remove(
    name: &str,
    force: bool,
    dry_run: bool,
) -> Result<(), String> {
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    let worktree = find_worktree(&git, name)?;
    let dirty_paths = git
        .dirty_paths(&worktree.path)
        .map_err(|error| error.to_string())?;
    if !force && !dirty_paths.is_empty() {
        let paths = dirty_paths
            .iter()
            .map(|path| path.path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "worktree {name} has uncommitted changes: {paths}"
        ));
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

fn handle_rename(
    source_name: &str,
    target_name: &str,
    dry_run: bool,
) -> Result<(), String> {
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
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

fn handle_finish(
    name: &str,
    dry_run: bool,
) -> Result<(), String> {
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
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

fn find_worktree(
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

fn validate_full_session_uuid(session_uuid: &str) -> Result<(), String> {
    let valid = session_uuid.len() == 36
        && session_uuid.chars().enumerate().all(|(index, character)| {
            matches!(index, 8 | 13 | 18 | 23) && character == '-'
                || !matches!(index, 8 | 13 | 18 | 23)
                    && character.is_ascii_hexdigit()
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "session UUID must be a full UUID such as 12345678-1234-1234-1234-123456789abc; short id '{session_uuid}' is not accepted"
        ))
    }
}

fn nested_relative_path(name: &str) -> Result<PathBuf, String> {
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

fn worktree_relative_path(
    git: &WorktreeGit,
    worktree: &session_worktree_provision::WorktreeRef,
) -> Result<PathBuf, String> {
    worktree
        .path
        .strip_prefix(git.main_checkout().join(".worktrees"))
        .map(Path::to_path_buf)
        .map_err(|_| {
            format!(
                "worktree {} is outside .worktrees",
                worktree.path.display()
            )
        })
}

fn rename_target_path(
    source_relative: &Path,
    target_name: &str,
) -> Result<PathBuf, String> {
    if source_relative.components().count() == 2 {
        let target_relative = nested_relative_path(target_name)?;
        if target_relative.parent() != source_relative.parent() {
            return Err(
                "nested worktree rename must keep the same full session UUID"
                    .to_owned(),
            );
        }
        return Ok(target_relative);
    }
    if target_name.contains('/') {
        return Err(
            "legacy worktree rename target must be a flat name".to_owned()
        );
    }
    Ok(PathBuf::from(target_name))
}

fn branch_for_relative_path(relative_path: &Path) -> Result<String, String> {
    let value = relative_path
        .to_str()
        .ok_or("worktree path must be valid UTF-8")?;
    Ok(format!("agent/{}", value.replace('\\', "/")))
}

fn nested_slug_directories(
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

fn nested_worktree_parent(
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

fn remove_empty_nested_parent(
    main_checkout: &Path,
    worktree_path: &Path,
) -> Result<(), String> {
    let Some(parent) = nested_worktree_parent(main_checkout, worktree_path)
    else {
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

fn handle_doctor(dry_run: bool) -> Result<(), String> {
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(&main_checkout).map_err(|error| error.to_string())?;
    let mut plan = LifecyclePlan::default();

    for submodule in git.submodule_paths().map_err(|error| error.to_string())? {
        let path = git.main_checkout().join(&submodule);
        if let Some(config) =
            stale_worktree_config(git.main_checkout(), &submodule)?
        {
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
        }
    }
    plan.add("prune stale superproject worktree registrations");
    if dry_run {
        plan.emit();
        return Ok(());
    }

    for submodule in git.submodule_paths().map_err(|error| error.to_string())? {
        let path = git.main_checkout().join(&submodule);
        if stale_worktree_config(git.main_checkout(), &submodule)?.is_some() {
            unset_core_worktree(git.main_checkout(), &submodule)?;
            git::run_git(&path, ["worktree", "prune"])?;
        }
        if Repository::open(&path).is_err() {
            initialize_submodule(git.main_checkout(), &submodule)?;
        }
    }
    git.worktree_prune().map_err(|error| error.to_string())?;
    println!("doctor: repairs complete");
    Ok(())
}

fn submodule_status(
    git: &WorktreeGit,
    worktree: &Path,
) -> Result<String, String> {
    let missing = git
        .submodule_paths()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|submodule| Repository::open(worktree.join(submodule)).is_err())
        .collect::<Vec<_>>();
    Ok(if missing.is_empty() {
        "initialized".to_owned()
    } else {
        format!("missing({})", missing.join(","))
    })
}

fn lifecycle_status(
    git: &WorktreeGit,
    activity: &SessionStoreActivity,
    worktree: &session_worktree_provision::WorktreeRef,
    policy: &ProvisionPolicy,
) -> Result<String, String> {
    match evaluate_reclaim_candidate(git, activity, worktree, policy)
        .map_err(|error| error.to_string())?
    {
        ReclaimEligibility::Reclaimable => Ok("reclaimable".to_owned()),
        ReclaimEligibility::Rejected(reason) =>
            Ok(format!("preserved reason={}", rejection_reason(&reason))),
    }
}

fn rejection_reason(reason: &ReclaimRejectionReason) -> String {
    match reason {
        ReclaimRejectionReason::OutsideWorktreeRoot =>
            "outside-worktree-root".to_owned(),
        ReclaimRejectionReason::SessionActive => "session-active".to_owned(),
        ReclaimRejectionReason::Detached => "detached".to_owned(),
        ReclaimRejectionReason::Dirty => "dirty".to_owned(),
        ReclaimRejectionReason::ContainsCurrentDirectory =>
            "contains-current-directory".to_owned(),
        ReclaimRejectionReason::NotIdle => "not-idle".to_owned(),
        ReclaimRejectionReason::DirtySubmodule { path } =>
            format!("dirty-submodule:{}", path.display()),
        ReclaimRejectionReason::AheadOfMain => "ahead-of-main".to_owned(),
    }
}

fn stale_worktree_config(
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
    let config =
        git2::Config::open(&config_path).map_err(|error| error.to_string())?;
    let value = match config.get_string("core.worktree") {
        Ok(value) => value,
        Err(error) if error.code() == git2::ErrorCode::NotFound =>
            return Ok(None),
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

fn initialize_submodule(
    main_checkout: &Path,
    submodule: &str,
) -> Result<(), String> {
    let repository =
        Repository::open(main_checkout).map_err(|error| error.to_string())?;
    let mut handle = repository
        .find_submodule(submodule)
        .map_err(|error| error.to_string())?;
    handle.init(true).map_err(|error| error.to_string())?;
    handle.update(true, None).map_err(|error| error.to_string())
}

fn unset_core_worktree(
    main_checkout: &Path,
    submodule: &str,
) -> Result<(), String> {
    let config = main_checkout
        .join(".git")
        .join("modules")
        .join(submodule)
        .join("config");
    let mut config =
        git2::Config::open(&config).map_err(|error| error.to_string())?;
    config
        .remove("core.worktree")
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{
        BRANCH_TEMPLATE,
        Cli,
        Command,
        DIRTY_MAIN_UNCOMMITTED_CHANGES_MESSAGE,
        FINISH_READY_TO_MERGE_MARKER,
        PRESERVE_MAIN_CHANGES_HINT,
        WORKTREE_PATH_OUTPUT_PREFIX,
        WORKTREE_PATH_TEMPLATE,
    };

    #[test]
    fn defines_lifecycle_output_contract_constants() {
        assert_eq!(WORKTREE_PATH_OUTPUT_PREFIX, "WORKTREE_PATH=");
        assert_eq!(FINISH_READY_TO_MERGE_MARKER, "ready-to-merge");
        assert_eq!(
            DIRTY_MAIN_UNCOMMITTED_CHANGES_MESSAGE,
            "uncommitted changes"
        );
        assert_eq!(PRESERVE_MAIN_CHANGES_HINT, "preserve-main-changes");
        assert_eq!(
            WORKTREE_PATH_TEMPLATE,
            ".worktrees/<full-session-uuid>/<slug>"
        );
        assert_eq!(BRANCH_TEMPLATE, "agent/<full-session-uuid>/<slug>");
    }

    #[test]
    fn parses_new_with_all_flags() {
        let cli = Cli::try_parse_from([
            "worktree-ctl",
            "new",
            "12345678-1234-1234-1234-123456789abc",
            "worktree-ctl",
            "--dry-run",
            "--preserve-main-changes",
        ])
        .unwrap();

        assert_eq!(
            cli.command,
            Command::New {
                session_uuid: "12345678-1234-1234-1234-123456789abc".to_owned(),
                slug: "worktree-ctl".to_owned(),
                dry_run: true,
                preserve_main_changes: true,
            }
        );
    }

    #[test]
    fn parses_list() {
        let cli = Cli::try_parse_from(["worktree-ctl", "list"]).unwrap();

        assert_eq!(cli.command, Command::List { dry_run: false });
    }

    #[test]
    fn parses_rebase_with_dry_run() {
        let cli = Cli::try_parse_from([
            "worktree-ctl",
            "rebase",
            "example",
            "--dry-run",
        ])
        .unwrap();

        assert_eq!(
            cli.command,
            Command::Rebase {
                name: "example".to_owned(),
                dry_run: true,
                auto_commit: false,
            }
        );
    }

    #[test]
    fn parses_merge_with_dry_run() {
        let cli = Cli::try_parse_from([
            "worktree-ctl",
            "merge",
            "example",
            "--dry-run",
        ])
        .unwrap();

        assert_eq!(
            cli.command,
            Command::Merge {
                name: "example".to_owned(),
                dry_run: true,
                auto_commit: false,
            }
        );
    }

    #[test]
    fn parses_sync_with_dry_run() {
        let cli = Cli::try_parse_from([
            "worktree-ctl",
            "sync",
            "example",
            "--dry-run",
        ])
        .unwrap();

        assert_eq!(
            cli.command,
            Command::Sync {
                name: "example".to_owned(),
                dry_run: true,
                auto_commit: false,
            }
        );
    }

    #[test]
    fn parses_sync_with_auto_commit() {
        let cli = Cli::try_parse_from([
            "worktree-ctl",
            "sync",
            "example",
            "--auto-commit",
        ])
        .unwrap();

        assert_eq!(
            cli.command,
            Command::Sync {
                name: "example".to_owned(),
                dry_run: false,
                auto_commit: true,
            }
        );
    }

    #[test]
    fn parses_remove_with_force_and_dry_run() {
        let cli = Cli::try_parse_from([
            "worktree-ctl",
            "remove",
            "example",
            "--force",
            "--dry-run",
        ])
        .unwrap();

        assert_eq!(
            cli.command,
            Command::Remove {
                name: "example".to_owned(),
                force: true,
                dry_run: true,
            }
        );
    }

    #[test]
    fn parses_rename_with_dry_run() {
        let cli = Cli::try_parse_from([
            "worktree-ctl",
            "rename",
            "old-name",
            "new-name",
            "--dry-run",
        ])
        .unwrap();

        assert_eq!(
            cli.command,
            Command::Rename {
                source_name: "old-name".to_owned(),
                target_name: "new-name".to_owned(),
                dry_run: true,
            }
        );
    }

    #[test]
    fn parses_finish_with_dry_run() {
        let cli = Cli::try_parse_from([
            "worktree-ctl",
            "finish",
            "example",
            "--dry-run",
        ])
        .unwrap();

        assert_eq!(
            cli.command,
            Command::Finish {
                name: "example".to_owned(),
                dry_run: true,
            }
        );
    }

    #[test]
    fn parses_doctor_with_dry_run() {
        let cli = Cli::try_parse_from(["worktree-ctl", "doctor", "--dry-run"])
            .unwrap();

        assert_eq!(cli.command, Command::Doctor { dry_run: true });
    }

    #[test]
    fn accepts_dry_run_for_every_mutating_subcommand() {
        for args in [
            vec![
                "new",
                "12345678-1234-1234-1234-123456789abc",
                "slug",
                "--dry-run",
            ],
            vec!["rebase", "example", "--dry-run"],
            vec!["merge", "example", "--dry-run"],
            vec!["sync", "example", "--dry-run"],
            vec!["remove", "example", "--dry-run"],
            vec!["rename", "old", "new", "--dry-run"],
            vec!["finish", "example", "--dry-run"],
            vec!["doctor", "--dry-run"],
        ] {
            let mut command_line = vec!["worktree-ctl"];
            command_line.extend(args);
            assert!(Cli::try_parse_from(command_line).is_ok());
        }
    }
}
