#[allow(unused_imports)]
use super::*;

pub(super) const ANSI_RESET: &str = "\x1b[0m";

pub(super) const ANSI_RED: &str = "\x1b[31m";

pub(super) const ANSI_GREEN: &str = "\x1b[32m";

pub(super) const ANSI_YELLOW: &str = "\x1b[33m";

pub(super) const ANSI_BLUE: &str = "\x1b[34m";

pub(super) const ANSI_MAGENTA: &str = "\x1b[35m";

pub(super) const ANSI_CYAN: &str = "\x1b[36m";

#[derive(Debug)]
pub(super) struct RepositoryState {
    pub(super) branch: String,
    pub(super) dirty: bool,
    pub(super) ahead: Option<usize>,
    pub(super) behind: Option<usize>,
}

pub(super) fn handle_list(_dry_run: bool, verbose: bool) -> Result<(), String> {
    let main_checkout = env::current_dir().map_err(|error| error.to_string())?;
    let git = WorktreeGit::open(&main_checkout).map_err(|error| error.to_string())?;
    let activity =
        SessionStoreActivity::with_default_staleness(canonical_session_store(git.main_checkout()));
    let policy = ProvisionPolicy::default();
    let registered = git.list_worktrees().map_err(|error| error.to_string())?;

    for worktree in &registered {
        let lifecycle = lifecycle_status(&git, &activity, worktree, &policy)?;
        let superproject = live_repository_state(&git, &worktree.path)?;
        let submodules = live_submodule_states(&git, worktree)?;
        if verbose {
            print_verbose_worktree(&worktree.path, &lifecycle, &superproject, &submodules);
        } else {
            print_compact_worktree(&git, worktree, &lifecycle, &superproject, &submodules);
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

pub(super) fn unregistered_worktree_debris(
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
                && !registered
                    .iter()
                    .any(|worktree| worktree.path == *path || worktree.path.starts_with(path))
        })
        .collect();
    Ok(debris)
}

pub(super) fn live_submodule_states(
    git: &WorktreeGit,
    worktree: &session_worktree_provision::WorktreeRef,
) -> Result<Vec<(String, Result<RepositoryState, String>)>, String> {
    git.submodule_paths()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|submodule| {
            let state = live_repository_state(&git, &worktree.path.join(&submodule));
            Ok((submodule, state))
        })
        .collect()
}

pub(super) fn print_verbose_worktree(
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
                Ok(state) => println!("    {name}: {}", verbose_repository_state(state)),
                Err(error) => println!("    {name}: unavailable ({error})"),
            }
        }
    }
}

pub(super) const COMPACT_WIDTH: usize = 100;

pub(super) fn print_compact_worktree(
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

    let superproject = compact_repository_state(superproject, expected_branch.as_deref());
    let mut repositories = vec![compact_item("super", superproject)];
    repositories.extend(submodules.iter().map(|(name, state)| match state {
        Ok(state) => compact_item(name, compact_repository_state(state, None)),
        Err(_) => format!("{name}={}", color(ANSI_RED, "missing")),
    }));
    print_wrapped("  ", repositories);
}

pub(super) fn print_wrapped(prefix: &str, items: Vec<String>) {
    let mut line = prefix.to_owned();
    let mut width = visible_width(prefix);
    for item in items {
        let item_width = visible_width(&item);
        let separator_width = usize::from(width > visible_width(prefix));
        if width + separator_width + item_width > COMPACT_WIDTH && width > visible_width(prefix) {
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

pub(super) fn visible_width(value: &str) -> usize {
    let bytes = value.as_bytes();
    let mut index = 0;
    let mut width = 0;
    while index < bytes.len() {
        if bytes[index] == b'\x1b' && bytes.get(index + 1) == Some(&b'[') {
            index += 2;
            while index < bytes.len() && !(b'@'..=b'~').contains(&bytes[index]) {
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

pub(super) fn compact_item(name: &str, state: String) -> String {
    if state.is_empty() {
        color(ANSI_GREEN, name)
    } else {
        format!("{name}={state}")
    }
}

pub(super) fn color(code: &str, value: impl std::fmt::Display) -> String {
    format!("{code}{value}{ANSI_RESET}")
}

pub(super) fn compact_lifecycle(lifecycle: &str) -> String {
    if lifecycle == "reclaimable" {
        color(ANSI_GREEN, "[ready]")
    } else if lifecycle.contains("session-active") {
        color(ANSI_CYAN, "[active]")
    } else {
        color(ANSI_YELLOW, "[held]")
    }
}

pub(super) fn compact_reason(
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
                    parts.push(color(ANSI_YELLOW, format!("ahead:{name}+{ahead}")));
                }
                if let Some(behind) = state.behind.filter(|behind| *behind != 0) {
                    parts.push(color(ANSI_BLUE, format!("behind:{name}-{behind}")));
                }
            }
            Err(_) => parts.push(color(ANSI_RED, format!("missing:{name}"))),
        }
    }
    if parts.is_empty() {
        parts.push(color(ANSI_GREEN, "clean"));
    }
    parts
}

pub(super) fn compact_repository_state(
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

pub(super) fn verbose_repository_state(state: &RepositoryState) -> String {
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

pub(super) fn live_repository_state(
    git: &WorktreeGit,
    path: &Path,
) -> Result<RepositoryState, String> {
    let repository = Repository::open(path).map_err(|error| error.to_string())?;
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
