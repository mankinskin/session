#[path = "git.rs"]
mod git;
#[path = "gitlink.rs"]
mod gitlink;
#[path = "sync.rs"]
mod sync;

use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use clap::{Args, Parser, Subcommand};
use git2::Repository;
use session_worktree_provision::{
    ReclaimEligibility, ReclaimRejectionReason, SessionActivity, SessionStoreActivity, WorktreeGit,
    evaluate_reclaim_candidate, policy::ProvisionPolicy,
};

fn canonical_session_store(workspace: &Path) -> PathBuf {
    workspace.join(".workflow-tools/session")
}

const WORKTREE_PATH_OUTPUT_PREFIX: &str = "WORKTREE_PATH=";
const FINISH_READY_TO_MERGE_MARKER: &str = "ready-to-merge";
const DIRTY_MAIN_UNCOMMITTED_CHANGES_MESSAGE: &str = "uncommitted changes";
const PRESERVE_MAIN_CHANGES_HINT: &str = "preserve-main-changes";
#[cfg(test)]
const WORKTREE_PATH_TEMPLATE: &str = ".worktrees/<full-session-uuid>/<slug>";
#[cfg(test)]
const BRANCH_TEMPLATE: &str = "agent/<full-session-uuid>/<slug>";

#[derive(Debug, Parser)]
#[command(name = "worktree-ctl", about = "Manage local Git worktree lifecycles")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Args, PartialEq, Eq)]
struct WorktreeSelection {
    #[arg(value_name = "WORKTREE")]
    names: Vec<String>,
    #[arg(long = "worktree", short = 'w', value_name = "WORKTREE")]
    worktrees: Vec<String>,
    #[arg(long)]
    all: bool,
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
        #[arg(long)]
        verbose: bool,
    },
    Rebase {
        #[command(flatten)]
        selection: WorktreeSelection,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        auto_commit: bool,
    },
    Merge {
        #[command(flatten)]
        selection: WorktreeSelection,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        auto_commit: bool,
    },
    Sync {
        #[command(flatten)]
        selection: WorktreeSelection,
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
    Clean {
        #[command(flatten)]
        selection: WorktreeSelection,
        #[arg(long)]
        dry_run: bool,
    },
    Commit {
        #[command(flatten)]
        selection: WorktreeSelection,
        #[arg(short, long, default_value = "worktree-ctl commit")]
        message: String,
        #[arg(last = true, value_name = "PATHSPEC")]
        paths: Vec<PathBuf>,
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
        } => handle_bootstrap(&session_uuid, &slug, dry_run, preserve_main_changes),
        Command::List { dry_run, verbose } => handle_list(dry_run, verbose),
        Command::Rebase {
            selection,
            dry_run,
            auto_commit,
        } => handle_rebase(selection, dry_run, auto_commit),
        Command::Merge {
            selection,
            dry_run,
            auto_commit,
        } => handle_merge(selection, dry_run, auto_commit),
        Command::Sync {
            selection,
            dry_run,
            auto_commit,
        } => handle_sync(selection, dry_run, auto_commit),
        Command::Remove {
            name,
            force,
            dry_run,
        } => handle_remove(&name, force, dry_run),
        Command::Clean { selection, dry_run } => handle_clean(selection, dry_run),
        Command::Commit {
            selection,
            message,
            paths,
            dry_run,
        } => handle_commit(selection, &message, &paths, dry_run),
        Command::Rename {
            source_name,
            target_name,
            dry_run,
        } => handle_rename(&source_name, &target_name, dry_run),
        Command::Finish { name, dry_run } => handle_finish(&name, dry_run),
        Command::Doctor { dry_run } => handle_doctor(dry_run),
    }
}

fn selected_worktrees(
    git: &WorktreeGit,
    selection: &WorktreeSelection,
) -> Result<Vec<session_worktree_provision::WorktreeRef>, String> {
    let selectors = selection
        .names
        .iter()
        .chain(&selection.worktrees)
        .collect::<Vec<_>>();
    if selection.all {
        if !selectors.is_empty() {
            return Err("--all cannot be combined with explicit worktree selectors".to_owned());
        }
        return git.list_worktrees().map_err(|error| error.to_string());
    }
    if selectors.is_empty() {
        return Err("select at least one worktree or pass --all".to_owned());
    }

    let mut paths = HashSet::new();
    let mut worktrees = Vec::new();
    for selector in selectors {
        let worktree = find_worktree(git, selector)?;
        if paths.insert(worktree.path.clone()) {
            worktrees.push(worktree);
        }
    }
    Ok(worktrees)
}

fn worktree_selector(
    git: &WorktreeGit,
    worktree: &session_worktree_provision::WorktreeRef,
) -> Result<String, String> {
    match worktree_relative_path(git, worktree) {
        Ok(path) => path
            .to_str()
            .map(|path| path.replace('\\', "/"))
            .ok_or_else(|| "worktree path must be valid UTF-8".to_owned()),
        Err(_) => Ok(worktree.name.clone()),
    }
}

pub(crate) fn checkpoint_owned_session_changes(
    git: &WorktreeGit,
    worktree: &session_worktree_provision::WorktreeRef,
    dry_run: bool,
) -> Result<(), String> {
    let activity =
        SessionStoreActivity::with_default_staleness(canonical_session_store(git.main_checkout()));
    let session_worktree_provision::WorktreeOwnership::Owned(owner_session_id) =
        activity.worktree_ownership(&worktree.path)
    else {
        return Ok(());
    };
    if dry_run {
        println!(
            "[dry-run] checkpoint owned session {} in {} when it is the only dirty path",
            owner_session_id,
            worktree.path.display()
        );
        return Ok(());
    }
    if git
        .checkpoint_owned_session_changes(&worktree.path, &owner_session_id)
        .map_err(|error| error.to_string())?
    {
        println!(
            "checkpointed owned session {} in {}",
            owner_session_id,
            worktree.path.display()
        );
    }
    Ok(())
}

pub(crate) fn checkpoint_session_mirror_changes(
    git: &WorktreeGit,
    worktree: &session_worktree_provision::WorktreeRef,
    dry_run: bool,
) -> Result<(), String> {
    let activity =
        SessionStoreActivity::with_default_staleness(canonical_session_store(git.main_checkout()));
    let session_worktree_provision::WorktreeOwnership::Owned(owner_session_id) =
        activity.worktree_ownership(&worktree.path)
    else {
        return Ok(());
    };
    if dry_run {
        println!(
            "[dry-run] checkpoint main session mirror {} when it is dirty",
            owner_session_id
        );
        return Ok(());
    }
    if git
        .checkpoint_session_mirror_changes(&worktree.path, &owner_session_id)
        .map_err(|error| error.to_string())?
    {
        println!(
            "checkpointed main session mirror {} for {}",
            owner_session_id,
            worktree.path.display()
        );
    }
    Ok(())
}

fn handle_rebase(
    selection: WorktreeSelection,
    dry_run: bool,
    auto_commit: bool,
) -> Result<(), String> {
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    for worktree in selected_worktrees(&git, &selection)? {
        let selector = worktree_selector(&git, &worktree)?;
        if selection.all && worktree.branch.is_none() {
            println!("skip {selector} because the worktree is detached");
            continue;
        }
        sync::handle_rebase(&selector, dry_run, auto_commit)
            .map_err(|error| format!("rebase {selector} failed: {error}"))?;
    }
    Ok(())
}

fn handle_merge(
    selection: WorktreeSelection,
    dry_run: bool,
    auto_commit: bool,
) -> Result<(), String> {
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    for worktree in selected_worktrees(&git, &selection)? {
        let selector = worktree_selector(&git, &worktree)?;
        sync::handle_merge(&selector, dry_run, auto_commit)
            .map_err(|error| format!("merge {selector} failed: {error}"))?;
    }
    Ok(())
}

fn handle_sync(
    selection: WorktreeSelection,
    dry_run: bool,
    auto_commit: bool,
) -> Result<(), String> {
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    let mut ordered = selected_worktrees(&git, &selection)?
        .into_iter()
        .map(|worktree| {
            let modified = std::fs::metadata(&worktree.path)
                .and_then(|metadata| metadata.modified())
                .map_err(|error| {
                    format!(
                        "could not read modification time for {}: {error}",
                        worktree.path.display()
                    )
                })?;
            Ok((modified, worktree))
        })
        .collect::<Result<Vec<_>, String>>()?;
    ordered.sort_by(|(left_time, left), (right_time, right)| {
        left_time
            .cmp(right_time)
            .then_with(|| left.path.cmp(&right.path))
    });
    for (_, worktree) in ordered {
        let selector = worktree_selector(&git, &worktree)?;
        sync::handle_sync(&selector, dry_run, auto_commit)
            .map_err(|error| format!("sync {selector} failed: {error}"))?;
    }
    Ok(())
}

fn handle_commit(
    selection: WorktreeSelection,
    message: &str,
    paths: &[PathBuf],
    dry_run: bool,
) -> Result<(), String> {
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    for worktree in selected_worktrees(&git, &selection)? {
        if dry_run {
            let target = if paths.is_empty() {
                "all changes".to_owned()
            } else {
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            println!(
                "[dry-run] stage {target} and commit {} with message {message:?}",
                worktree.path.display()
            );
            continue;
        }
        let mut add = ProcessCommand::new("git");
        add.arg("add").current_dir(&worktree.path);
        if paths.is_empty() {
            add.arg("-A");
        } else {
            add.arg("--").args(paths);
        }
        let status = add.status().map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!(
                "could not stage changes in {}",
                worktree.path.display()
            ));
        }
        let status = ProcessCommand::new("git")
            .args(["commit", "-m", message])
            .current_dir(&worktree.path)
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!(
                "could not commit changes in {}",
                worktree.path.display()
            ));
        }
    }
    Ok(())
}

#[derive(Default)]
struct LifecyclePlan {
    actions: Vec<String>,
}

impl LifecyclePlan {
    fn add(&mut self, action: impl Into<String>) {
        self.actions.push(action.into());
    }

    fn emit(&self) {
        for action in &self.actions {
            println!("[dry-run] {action}");
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[path = "listing.rs"]
mod listing;
#[allow(unused_imports)]
pub use listing::*;

#[path = "creation.rs"]
mod creation;
#[allow(unused_imports)]
pub use creation::*;

#[path = "maintenance.rs"]
mod maintenance;
#[allow(unused_imports)]
pub use maintenance::*;

#[path = "doctor.rs"]
mod doctor;
#[allow(unused_imports)]
pub use doctor::*;
