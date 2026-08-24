mod git;
mod gitlink;
mod sync;

use std::{
    collections::HashSet,
    env,
    path::{
        Path,
        PathBuf,
    },
    process::Command as ProcessCommand,
};

use clap::{
    Args,
    Parser,
    Subcommand,
};
use git2::Repository;
use session_worktree_provision::{
    ReclaimEligibility,
    ReclaimRejectionReason,
    SessionActivity,
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
        } => handle_bootstrap(
            &session_uuid,
            &slug,
            dry_run,
            preserve_main_changes,
        ),
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
        Command::Clean { selection, dry_run } =>
            handle_clean(selection, dry_run),
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
            return Err(
                "--all cannot be combined with explicit worktree selectors"
                    .to_owned(),
            );
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
    let activity = SessionStoreActivity::with_default_staleness(
        git.main_checkout().join(".session"),
    );
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
    let activity = SessionStoreActivity::with_default_staleness(
        git.main_checkout().join(".session"),
    );
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
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
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
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
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
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
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
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
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

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_BLUE: &str = "\x1b[34m";
const ANSI_MAGENTA: &str = "\x1b[35m";
const ANSI_CYAN: &str = "\x1b[36m";

#[derive(Debug)]
struct RepositoryState {
    branch: String,
    dirty: bool,
    ahead: Option<usize>,
    behind: Option<usize>,
}

fn handle_list(
    _dry_run: bool,
    verbose: bool,
) -> Result<(), String> {
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
        let lifecycle = lifecycle_status(&git, &activity, worktree, &policy)?;
        let superproject = live_repository_state(&git, &worktree.path)?;
        let submodules = live_submodule_states(&git, worktree)?;
        if verbose {
            print_verbose_worktree(
                &worktree.path,
                &lifecycle,
                &superproject,
                &submodules,
            );
        } else {
            print_compact_worktree(
                &git,
                worktree,
                &lifecycle,
                &superproject,
                &submodules,
            );
        }
    }

    for path in unregistered_worktree_debris(&git, &registered)? {
        if verbose {
            println!("worktree: {}", path.display());
            println!("  lifecycle: unregistered-debris");
        } else {
            println!(
                "{} {} {}",
                path.strip_prefix(git.main_checkout())
                    .unwrap_or(&path)
                    .display(),
                color(ANSI_RED, "[debris]"),
                color(ANSI_RED, "unregistered")
            );
        }
    }
    Ok(())
}

fn unregistered_worktree_debris(
    git: &WorktreeGit,
    registered: &[session_worktree_provision::WorktreeRef],
) -> Result<Vec<PathBuf>, String> {
    let worktree_root = git.main_checkout().join(".worktrees");
    if !worktree_root.is_dir() {
        return Ok(Vec::new());
    }
    let debris = std::fs::read_dir(worktree_root)
        .map_err(|error| error.to_string())?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && !registered.iter().any(|worktree| {
                    worktree.path == *path || worktree.path.starts_with(path)
                })
        })
        .collect();
    Ok(debris)
}

fn live_submodule_states(
    git: &WorktreeGit,
    worktree: &session_worktree_provision::WorktreeRef,
) -> Result<Vec<(String, Result<RepositoryState, String>)>, String> {
    git.submodule_paths()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|submodule| {
            let state =
                live_repository_state(&git, &worktree.path.join(&submodule));
            Ok((submodule, state))
        })
        .collect()
}

fn print_verbose_worktree(
    path: &Path,
    lifecycle: &str,
    superproject: &RepositoryState,
    submodules: &[(String, Result<RepositoryState, String>)],
) {
    println!("worktree: {}", path.display());
    println!("  lifecycle: {lifecycle}");
    println!("  superproject: {}", verbose_repository_state(superproject));
    if submodules.is_empty() {
        println!("  submodules: none");
    } else {
        println!("  submodules:");
        for (name, state) in submodules {
            match state {
                Ok(state) =>
                    println!("    {name}: {}", verbose_repository_state(state)),
                Err(error) => println!("    {name}: unavailable ({error})"),
            }
        }
    }
}

const COMPACT_WIDTH: usize = 100;

fn print_compact_worktree(
    git: &WorktreeGit,
    worktree: &session_worktree_provision::WorktreeRef,
    lifecycle: &str,
    superproject: &RepositoryState,
    submodules: &[(String, Result<RepositoryState, String>)],
) {
    let relative = worktree
        .path
        .strip_prefix(git.main_checkout())
        .unwrap_or(&worktree.path);
    let expected_branch = worktree_relative_path(git, worktree)
        .ok()
        .and_then(|path| branch_for_relative_path(&path).ok());
    let lifecycle = compact_lifecycle(lifecycle);
    println!("{} {}", relative.display(), lifecycle);
    print_wrapped("  ", compact_reason(superproject, submodules));

    let superproject =
        compact_repository_state(superproject, expected_branch.as_deref());
    let mut repositories = vec![compact_item("super", superproject)];
    repositories.extend(submodules.iter().map(|(name, state)| match state {
        Ok(state) => compact_item(name, compact_repository_state(state, None)),
        Err(_) => format!("{name}={}", color(ANSI_RED, "missing")),
    }));
    print_wrapped("  ", repositories);
}

fn print_wrapped(
    prefix: &str,
    items: Vec<String>,
) {
    let mut line = prefix.to_owned();
    let mut width = visible_width(prefix);
    for item in items {
        let item_width = visible_width(&item);
        let separator_width = usize::from(width > visible_width(prefix));
        if width + separator_width + item_width > COMPACT_WIDTH
            && width > visible_width(prefix)
        {
            println!("{line}");
            line = prefix.to_owned();
            width = visible_width(prefix);
        }
        if width > visible_width(prefix) {
            line.push(' ');
            width += 1;
        }
        line.push_str(&item);
        width += item_width;
    }
    if width > visible_width(prefix) {
        println!("{line}");
    }
}

fn visible_width(value: &str) -> usize {
    let bytes = value.as_bytes();
    let mut index = 0;
    let mut width = 0;
    while index < bytes.len() {
        if bytes[index] == b'\x1b' && bytes.get(index + 1) == Some(&b'[') {
            index += 2;
            while index < bytes.len() && !(b'@'..=b'~').contains(&bytes[index])
            {
                index += 1;
            }
            index += usize::from(index < bytes.len());
        } else {
            width += 1;
            index += 1;
        }
    }
    width
}
fn compact_item(
    name: &str,
    state: String,
) -> String {
    if state.is_empty() {
        color(ANSI_GREEN, name)
    } else {
        format!("{name}={state}")
    }
}
fn color(
    code: &str,
    value: impl std::fmt::Display,
) -> String {
    format!("{code}{value}{ANSI_RESET}")
}

fn compact_lifecycle(lifecycle: &str) -> String {
    if lifecycle == "reclaimable" {
        color(ANSI_GREEN, "[ready]")
    } else if lifecycle.contains("session-active") {
        color(ANSI_CYAN, "[active]")
    } else {
        color(ANSI_YELLOW, "[held]")
    }
}

fn compact_reason(
    superproject: &RepositoryState,
    submodules: &[(String, Result<RepositoryState, String>)],
) -> Vec<String> {
    let mut parts = Vec::new();
    if superproject.dirty {
        parts.push(color(ANSI_RED, "dirty:super"));
    }
    if let Some(ahead) = superproject.ahead.filter(|ahead| *ahead != 0) {
        parts.push(color(ANSI_YELLOW, format!("ahead:super+{ahead}")));
    }
    if let Some(behind) = superproject.behind.filter(|behind| *behind != 0) {
        parts.push(color(ANSI_BLUE, format!("behind:super-{behind}")));
    }
    for (name, state) in submodules {
        match state {
            Ok(state) => {
                if state.dirty {
                    parts.push(color(ANSI_RED, format!("dirty:{name}")));
                }
                if let Some(ahead) = state.ahead.filter(|ahead| *ahead != 0) {
                    parts.push(color(
                        ANSI_YELLOW,
                        format!("ahead:{name}+{ahead}"),
                    ));
                }
                if let Some(behind) = state.behind.filter(|behind| *behind != 0)
                {
                    parts.push(color(
                        ANSI_BLUE,
                        format!("behind:{name}-{behind}"),
                    ));
                }
            },
            Err(_) => parts.push(color(ANSI_RED, format!("missing:{name}"))),
        }
    }
    if parts.is_empty() {
        parts.push(color(ANSI_GREEN, "clean"));
    }
    parts
}
fn compact_repository_state(
    state: &RepositoryState,
    expected_branch: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    if state.branch != "HEAD"
        && state.branch != "main"
        && Some(state.branch.as_str()) != expected_branch
    {
        parts.push(color(ANSI_MAGENTA, &state.branch));
    }
    if state.dirty {
        parts.push(color(ANSI_RED, "dirty"));
    }
    if let Some(ahead) = state.ahead.filter(|ahead| *ahead != 0) {
        parts.push(color(ANSI_YELLOW, format!("+{ahead}")));
    }
    if let Some(behind) = state.behind.filter(|behind| *behind != 0) {
        parts.push(color(ANSI_BLUE, format!("-{behind}")));
    }
    parts.join(" ")
}
fn verbose_repository_state(state: &RepositoryState) -> String {
    let ahead = state
        .ahead
        .map_or("?".to_owned(), |value| value.to_string());
    let behind = state
        .behind
        .map_or("?".to_owned(), |value| value.to_string());
    let changes = if state.dirty { "dirty" } else { "clean" };
    format!(
        "branch={} changes={changes} ahead={ahead} behind={behind}",
        state.branch
    )
}

fn live_repository_state(
    git: &WorktreeGit,
    path: &Path,
) -> Result<RepositoryState, String> {
    let repository =
        Repository::open(path).map_err(|error| error.to_string())?;
    let branch = repository
        .head()
        .ok()
        .and_then(|head| head.shorthand().map(str::to_owned))
        .unwrap_or_else(|| "unborn".to_owned());
    let (ahead, behind) = match git.ahead_behind(path, "main") {
        Ok((ahead, behind)) => (Some(ahead), Some(behind)),
        Err(_) => (None, None),
    };
    Ok(RepositoryState {
        branch,
        dirty: git.is_dirty(path).map_err(|error| error.to_string())?,
        ahead,
        behind,
    })
}
fn handle_clean(
    selection: WorktreeSelection,
    dry_run: bool,
) -> Result<(), String> {
    let main_checkout =
        env::current_dir().map_err(|error| error.to_string())?;
    let git =
        WorktreeGit::open(main_checkout).map_err(|error| error.to_string())?;
    let activity = SessionStoreActivity::with_default_staleness(
        git.main_checkout().join(".session"),
    );
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
            Err(reason) => println!(
                "preserved path={} reason={reason}",
                worktree.path.display()
            ),
        }
    }
    let mut removable_debris = Vec::new();
    if selection.all {
        let registered =
            git.list_worktrees().map_err(|error| error.to_string())?;
        for path in unregistered_worktree_debris(&git, &registered)? {
            let mut entries =
                std::fs::read_dir(&path).map_err(|error| error.to_string())?;
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
fn ensure_safe_to_remove(
    git: &WorktreeGit,
    worktree: &session_worktree_provision::WorktreeRef,
) -> Result<(), String> {
    for submodule in git.submodule_paths().map_err(|error| error.to_string())? {
        let path = worktree.path.join(&submodule);
        if !path.is_dir() {
            return Err(format!("submodule {submodule} is not initialized"));
        }
        if git.is_dirty(&path).map_err(|error| error.to_string())? {
            return Err(format!(
                "submodule {submodule} has uncommitted changes"
            ));
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
        WorktreeSelection,
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

        assert_eq!(
            cli.command,
            Command::List {
                dry_run: false,
                verbose: false
            }
        );
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
                selection: WorktreeSelection {
                    names: vec!["example".to_owned()],
                    worktrees: Vec::new(),
                    all: false,
                },
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
                selection: WorktreeSelection {
                    names: vec!["example".to_owned()],
                    worktrees: Vec::new(),
                    all: false,
                },
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
                selection: WorktreeSelection {
                    names: vec!["example".to_owned()],
                    worktrees: Vec::new(),
                    all: false,
                },
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
                selection: WorktreeSelection {
                    names: vec!["example".to_owned()],
                    worktrees: Vec::new(),
                    all: false,
                },
                dry_run: false,
                auto_commit: true,
            }
        );
    }

    #[test]
    fn parses_repeated_worktree_selection_and_all() {
        let selected = Cli::try_parse_from([
            "worktree-ctl",
            "rebase",
            "--worktree",
            "first",
            "--worktree",
            "second",
        ])
        .unwrap();
        assert_eq!(
            selected.command,
            Command::Rebase {
                selection: WorktreeSelection {
                    names: Vec::new(),
                    worktrees: vec!["first".to_owned(), "second".to_owned()],
                    all: false,
                },
                dry_run: false,
                auto_commit: false,
            }
        );

        let all =
            Cli::try_parse_from(["worktree-ctl", "clean", "--all"]).unwrap();
        assert_eq!(
            all.command,
            Command::Clean {
                selection: WorktreeSelection {
                    names: Vec::new(),
                    worktrees: Vec::new(),
                    all: true,
                },
                dry_run: false,
            }
        );
    }

    #[test]
    fn parses_commit_pathspecs_after_double_dash() {
        let cli = Cli::try_parse_from([
            "worktree-ctl",
            "commit",
            "example",
            "--",
            "src/lib.rs",
            "README.md",
        ])
        .unwrap();
        assert_eq!(
            cli.command,
            Command::Commit {
                selection: WorktreeSelection {
                    names: vec!["example".to_owned()],
                    worktrees: Vec::new(),
                    all: false,
                },
                message: "worktree-ctl commit".to_owned(),
                paths: vec!["src/lib.rs".into(), "README.md".into()],
                dry_run: false,
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
