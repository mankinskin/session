//! Validates caller-supplied paths against the session checkout without
//! substituting paths or selecting a different execution directory.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use session_api::{SessionError, store::SessionStoreConfig};
use session_workspace_resolver::{
    ResolutionError, ResolveRequest, ResolverConfig, SessionWorkspaceResolver,
};

use super::gating::{ToolAccess, tool_access};

pub(crate) const MAIN_CHECKOUT_ENV: &str = "MCP_MAIN_CHECKOUT";
pub(crate) const DEFAULT_STORE_DIR: &str = ".session";

fn canonical_session_store_root(workspace: &Path) -> PathBuf {
    workspace.join(".workflow-tools/session")
}

/// Builds the resolver anchored on the checkout the servers were launched in.
///
/// The anchor is inferred from the process working directory, which is the
/// checkout the MCP servers were started in. `MCP_MAIN_CHECKOUT` remains an
/// override for callers that cannot control that working directory; it is not
/// required for normal operation.
pub(crate) fn anchored_resolver() -> Result<SessionWorkspaceResolver, String> {
    let config = match std::env::var(MAIN_CHECKOUT_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(override_path) => ResolverConfig {
            main_checkout: PathBuf::from(override_path),
            workspace_path: "default".to_string(),
        },
        None => ResolverConfig::from_working_dir("default").map_err(|error| error.to_string())?,
    };
    SessionWorkspaceResolver::new(config).map_err(|error| error.to_string())
}

fn resolve_workspace(
    session_id: &str,
    workspace: Option<&str>,
    access: ToolAccess,
) -> Result<(String, PathBuf), String> {
    let store_dir = DEFAULT_STORE_DIR.to_string();
    let resolver = anchored_resolver()?;
    let (canonical_target_root, store_root) = match resolver.resolve(ResolveRequest {
        session_id,
        relative_workspace: None,
        store_dir: &store_dir,
    }) {
        Ok(resolved) => {
            if access == ToolAccess::Mutation {
                resolved
                    .require_mutation_target()
                    .map_err(|error| error.to_string())?;
            }
            let store_root = resolved
                .store_root(&store_dir)
                .map_err(|error| error.to_string())?;
            let canonical_target_root =
                std::fs::canonicalize(resolved.target_root()).map_err(|error| {
                    format!(
                        "resolved session worktree '{}' could not be canonicalized: {error}",
                        resolved.target_root().display()
                    )
                })?;
            (canonical_target_root, store_root)
        }
        Err(ResolutionError::MissingSessionWorktree { .. }) => {
            resolve_unassigned_session_target(&resolver, session_id, &store_dir, access)?
        }
        Err(other) => return Err(other.to_string()),
    };
    let target_root = workspace
        .map(|value| {
            let workspace = Path::new(value);
            let canonical_workspace = canonicalize_tool_path(workspace)?;
            if !canonical_workspace.starts_with(&canonical_target_root) {
                return Err(format!(
                    "PATH_OUTSIDE_SESSION_WORKTREE: path '{}' (canonical '{}') is outside resolved session worktree '{}' for session '{}'. Supply a path inside the assigned checkout; paths are not rewritten.",
                    workspace.display(),
                    canonical_workspace.display(),
                    canonical_target_root.display(),
                    session_id
                ));
            }
            Ok(canonical_workspace)
        })
        .transpose()?
        .unwrap_or(canonical_target_root)
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .trim_end_matches('/')
        .to_string();
    Ok((target_root, store_root))
}

pub(crate) fn canonicalize_tool_path(path: &Path) -> Result<PathBuf, String> {
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot determine tool working directory: {error}"))?
            .join(path)
    };
    match std::fs::canonicalize(&absolute_path) {
        Ok(canonical) => Ok(canonical),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            match std::fs::symlink_metadata(&absolute_path) {
                Err(metadata_error) if metadata_error.kind() == ErrorKind::NotFound => {}
                _ => {
                    return Err(format!(
                        "cannot validate path '{}': {error}",
                        absolute_path.display()
                    ));
                }
            }
            let parent = absolute_path.parent().ok_or_else(|| {
                format!(
                    "cannot validate path '{}': {error}",
                    absolute_path.display()
                )
            })?;
            let name = absolute_path.file_name().ok_or_else(|| {
                format!(
                    "cannot validate path '{}': {error}",
                    absolute_path.display()
                )
            })?;
            Ok(canonicalize_tool_path(parent)?.join(name))
        }
        Err(error) => Err(format!(
            "cannot validate path '{}': {error}",
            absolute_path.display()
        )),
    }
}

/// Resolves the checkout scope for a session with no discoverable worktree
/// (never checked in, or an assignment that no longer resolves on disk).
///
/// Reads always fall back to the repository's main-checkout store: a stale
/// read has no destructive effect, so there is nothing to gate. Mutations
/// only fall back when the session genuinely never opted into worktree
/// isolation (`session_is_unassigned`); a session that did opt in but whose
/// assignment is now broken stays blocked from mutating the main checkout.
fn resolve_unassigned_session_target(
    resolver: &SessionWorkspaceResolver,
    session_id: &str,
    store_dir: &str,
    access: ToolAccess,
) -> Result<(PathBuf, PathBuf), String> {
    let candidates = resolver.refused_candidates(store_dir).unwrap_or_default();
    let looks_like_repository_root = candidates.len() == 1
        && candidates[0]
            .parent()
            .is_some_and(|root| root.join(".git").exists());
    if !looks_like_repository_root {
        return Err(ResolutionError::UnanchoredDefault {
            session_id: session_id.to_string(),
            candidates,
        }
        .to_string());
    }
    if access == ToolAccess::Mutation {
        let repository_root = candidates[0]
            .parent()
            .expect("looks_like_repository_root guarantees a parent");
        if !session_is_unassigned(repository_root, session_id)? {
            return Err(ResolutionError::MainCheckoutMutationBlocked.to_string());
        }
    }
    repository_root_target(resolver, store_dir)
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .to_string()
}

/// Targets the repository's main-checkout store directly, bypassing worktree
/// resolution. Used for reads always, and for mutations from a session that
/// has never opted into worktree isolation via `session_check_in`.
///
/// Returns a canonicalized target root so callers can apply the same
/// absolute-workspace containment check used for a resolved session worktree.
fn repository_root_target(
    resolver: &SessionWorkspaceResolver,
    store_dir: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let store_root = resolver
        .refused_candidates(store_dir)
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
        .ok_or_else(|| "workspace resolution could not derive repository anchor".to_string())?;
    let target_root = store_root.parent().ok_or_else(|| {
        format!(
            "repository store root '{}' has no repository parent",
            normalized_path(&store_root)
        )
    })?;
    let canonical_target_root = std::fs::canonicalize(target_root).map_err(|error| {
        format!(
            "repository checkout '{}' could not be canonicalized: {error}",
            normalized_path(target_root)
        )
    })?;
    Ok((canonical_target_root, store_root))
}

/// A legacy assignment to the main checkout is not worktree isolation.
/// A missing assigned worktree remains assigned and blocks mutation fallback.
fn session_is_unassigned(repository_root: &Path, session_id: &str) -> Result<bool, String> {
    let config = SessionStoreConfig::new(canonical_session_store_root(repository_root));
    match config.read_session(session_id) {
        Ok(record) => Ok(match record.metadata.worktree {
            None => true,
            Some(assignment) => {
                assignment_targets_repository_root(repository_root, &assignment.path)
            }
        }),
        Err(SessionError::NotFound { .. }) => Ok(true),
        Err(error) => Err(error.to_string()),
    }
}

fn assignment_targets_repository_root(repository_root: &Path, assignment_path: &Path) -> bool {
    match (
        std::fs::canonicalize(repository_root),
        std::fs::canonicalize(assignment_path),
    ) {
        (Ok(repository_root), Ok(assignment_path)) => repository_root == assignment_path,
        _ => false,
    }
}

fn try_resolve_session_check_in_bootstrap_workspace(
    tool: &str,
    session_id: &str,
    workspace: Option<&str>,
) -> Result<Option<(String, PathBuf)>, String> {
    if tool != "session_check_in" {
        return Ok(None);
    }
    let Some(selector) = workspace else {
        return Ok(None);
    };
    if selector.is_empty() || selector == "default" {
        return Ok(None);
    }
    let workspace_path = PathBuf::from(selector);
    if !workspace_path.is_absolute() {
        return Ok(None);
    }

    let resolver = anchored_resolver()?;
    let canonical_workspace = std::fs::canonicalize(&workspace_path).map_err(|error| {
        format!(
            "workspace '{}' could not be canonicalized: {error}",
            workspace_path.display()
        )
    })?;
    let anchor_candidate = resolver
        .refused_candidates(DEFAULT_STORE_DIR)
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
        .ok_or_else(|| {
            "session_check_in bootstrap could not derive repository anchor".to_string()
        })?;
    let repository = anchor_candidate
        .parent()
        .ok_or_else(|| {
            format!(
                "session_check_in bootstrap anchor '{}' has no repository parent",
                normalized_path(&anchor_candidate)
            )
        })?
        .to_path_buf();
    let canonical_repository = std::fs::canonicalize(&repository).map_err(|error| {
        format!(
            "session_check_in bootstrap repository '{}' could not be canonicalized: {error}",
            normalized_path(&repository)
        )
    })?;
    if !session_is_unassigned(&canonical_repository, session_id)? {
        return Ok(None);
    }
    let canonical_worktrees = canonical_repository.join(".worktrees");
    let canonical_nested_parent = canonical_worktrees.join(session_id);
    let is_nested_child = canonical_workspace.parent() == Some(canonical_nested_parent.as_path());
    let is_legacy_flat_child = canonical_workspace.parent() == Some(canonical_worktrees.as_path());
    if !is_nested_child && !is_legacy_flat_child {
        return Err(format!(
            "session_check_in bootstrap workspace '{}' must be a direct child of '{}' or '{}'; received '{}'.",
            workspace_path.display(),
            normalized_path(&canonical_nested_parent),
            normalized_path(&canonical_worktrees),
            normalized_path(&canonical_workspace)
        ));
    }
    let git_entry = canonical_workspace.join(".git");
    if !git_entry.exists() {
        return Err(format!(
            "session_check_in bootstrap workspace '{}' is missing required '.git' entry",
            normalized_path(&canonical_workspace)
        ));
    }
    Ok(Some((
        normalized_path(&canonical_workspace),
        canonical_session_store_root(&canonical_workspace),
    )))
}

pub(crate) fn resolve_workspace_for_tool(
    tool: &str,
    session_id: &str,
    workspace: Option<&str>,
) -> Result<(String, PathBuf), String> {
    // `session_check_in` bootstrap enforces a stricter shape (a direct
    // `.worktrees/<session>/<slug>` child with a `.git` entry) than the
    // general containment check in `resolve_workspace`, and only applies to
    // an unassigned session naming an absolute workspace. Run it first so a
    // nested path that would otherwise pass the looser "anywhere under the
    // main checkout" containment check still gets rejected.
    if let Some(resolved) =
        try_resolve_session_check_in_bootstrap_workspace(tool, session_id, workspace)?
    {
        return Ok(resolved);
    }
    let access = tool_access(tool);
    resolve_workspace(session_id, workspace, access)
}
