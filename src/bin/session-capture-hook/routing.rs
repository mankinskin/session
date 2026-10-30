#[allow(unused_imports)]
use super::*;

/// Returns the canonical session store, creating new capture state beneath
/// `.workflow-tools/session` rather than resurrecting a legacy `.session` tree.
pub(super) fn session_store_root_at(checkout: &Path) -> PathBuf {
    canonical_store_root(checkout, SESSION_STORE_DIR)
}

/// Returns the checkout that owns a resolved session store, stepping over the
/// extra `.workflow-tools` level present in the canonical layout.
pub(super) fn checkout_for_store_root(store_root: &Path) -> Option<&Path> {
    let parent = store_root.parent()?;
    if parent.file_name().and_then(|name| name.to_str()) == Some(CANONICAL_STORES_DIR) {
        parent.parent()
    } else {
        Some(parent)
    }
}

/// Resolves the checkout the hook was launched in, which anchors positional
/// worktree discovery.
///
/// `MCP_MAIN_CHECKOUT` stays available as an override for callers that cannot
/// control the working directory, but it is not required.
pub(super) fn anchor_checkout(current_dir: &Path) -> PathBuf {
    std::env::var_os("MCP_MAIN_CHECKOUT")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| current_dir.to_path_buf())
}

pub(super) fn initialize_session_routing(
    session_id: Option<&str>,
    store_root: Option<&Path>,
) -> Option<ProvisioningDiagnostic> {
    let Some(session_id) = session_id.filter(|session_id| !session_id.trim().is_empty()) else {
        return Some(ProvisioningDiagnostic::Skipped {
            reason: "missing_session_id",
            worktree: None,
        });
    };
    let current_dir = match std::env::current_dir() {
        Ok(current_dir) => current_dir,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] session routing skipped: could not determine current directory: {error}"
            );
            return Some(ProvisioningDiagnostic::Skipped {
                reason: "current_directory_unavailable",
                worktree: None,
            });
        }
    };
    let anchor = anchor_checkout(&current_dir);
    if !anchor.is_dir() {
        eprintln!(
            "[session-capture-hook] session routing skipped: anchor checkout '{}' does not exist",
            anchor.display()
        );
        return Some(ProvisioningDiagnostic::Skipped {
            reason: "anchor_checkout_invalid",
            worktree: None,
        });
    }
    let mut diagnostic = ProvisioningDiagnostic::Skipped {
        reason: "no_registered_worktree",
        worktree: None,
    };
    if store_root.is_some() {
        return Some(diagnostic);
    }
    let resolver = match SessionWorkspaceResolver::new(ResolverConfig {
        main_checkout: anchor,
        workspace_path: "default".to_string(),
    }) {
        Ok(resolver) => resolver,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] session routing skipped: could not configure session workspace resolver: {error}"
            );
            return Some(diagnostic);
        }
    };
    if let Ok(workspace) = resolver.resolve(ResolveRequest {
        session_id,
        relative_workspace: None,
        store_dir: SESSION_STORE_DIR,
    }) {
        if workspace.is_worktree() {
            diagnostic.set_worktree(workspace.target_root());
        }
    }
    Some(diagnostic)
}

pub(super) fn resolve_capture_store_root(
    store_root: Option<PathBuf>,
    session_id: Option<&str>,
) -> Option<PathBuf> {
    if let Some(store_root) = store_root {
        return Some(store_root);
    }

    let Some(session_id) = session_id.filter(|session_id| !session_id.trim().is_empty()) else {
        eprintln!(
            "[session-capture-hook] capture skipped: hook payload has no session id; refusing to write a default .session store"
        );
        return None;
    };
    let current_dir = match std::env::current_dir() {
        Ok(current_dir) => current_dir,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] capture skipped: could not determine current directory: {error}"
            );
            return None;
        }
    };
    let anchor = anchor_checkout(&current_dir);
    let main_store_root = session_store_root_at(&anchor);
    let resolver = match SessionWorkspaceResolver::new(ResolverConfig {
        main_checkout: anchor,
        workspace_path: "default".to_string(),
    }) {
        Ok(resolver) => resolver,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] capture skipped: could not configure session workspace resolver: {error}"
            );
            return None;
        }
    };
    match resolver.resolve(ResolveRequest {
        session_id,
        relative_workspace: None,
        store_dir: SESSION_STORE_DIR,
    }) {
        Ok(workspace) => match workspace.require_mutation_target() {
            Ok(()) => Some(session_store_root_at(workspace.target_root())),
            Err(error) => {
                eprintln!(
                    "[session-capture-hook] capture skipped: could not resolve worktree session store: {error}"
                );
                None
            }
        },
        Err(ResolutionError::MissingSessionWorktree { .. }) => Some(main_store_root),
        Err(error) => {
            eprintln!(
                "[session-capture-hook] capture skipped: registered workspace could not be resolved: {error}"
            );
            None
        }
    }
}

pub(super) fn infer_capture_worktree(
    config: &SessionStoreConfig,
    session_id: &str,
    store_root: &Path,
) -> Result<(), SessionError> {
    let Some(worktree_root) = checkout_for_store_root(store_root) else {
        return Ok(());
    };
    let main_checkout = std::env::current_dir()
        .map(|current_dir| anchor_checkout(&current_dir))
        .unwrap_or_else(|_| worktree_root.to_path_buf());
    config.infer_worktree_from_environment(session_id, worktree_root, &main_checkout)
}

/// Mirrors the worktree's own resolved assignment into the main checkout's
/// registry (ticket 842d74cb D1), best-effort: a missing or unreadable main
/// checkout must never fail capture, and re-reading the just-captured record
/// (rather than trusting the in-memory `plan`) picks up whatever
/// `infer_capture_worktree` just persisted.
pub(super) fn mirror_worktree_assignment_to_main(
    config: &SessionStoreConfig,
    session_id: &str,
    store_root: &Path,
) {
    let Some(worktree_root) = checkout_for_store_root(store_root) else {
        return;
    };
    let Some(anchor) = anchor_checkout_for_worktree(worktree_root) else {
        return;
    };
    let main_store_root = session_store_root_at(&anchor);
    let record = match config.read_session(session_id) {
        Ok(record) => record,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] main-checkout registry mirror skipped for session {session_id}: could not read worktree record: {error}"
            );
            return;
        }
    };
    let Some(assignment) = record.metadata.worktree else {
        return;
    };
    let main_config = SessionStoreConfig::new(main_store_root);
    if let Err(error) = main_config.register_provisioned_worktree(
        session_id,
        &assignment.path,
        &assignment.branch,
        assignment.allocation_mode,
    ) {
        eprintln!(
            "[session-capture-hook] main-checkout registry mirror failed for session {session_id}: {error}"
        );
    }
}

/// Resolves the main checkout above a nested `.worktrees/<uuid>/<slug>`
/// worktree directory. Returns `None` for a legacy flat layout or a worktree
/// path with no `.worktrees` ancestor rather than guessing.
pub(super) fn anchor_checkout_for_worktree(worktree_root: &Path) -> Option<PathBuf> {
    let session_dir = worktree_root.parent()?;
    let worktrees_dir = session_dir.parent()?;
    if worktrees_dir.file_name()?.to_str()? != ".worktrees" {
        return None;
    }
    worktrees_dir.parent().map(Path::to_path_buf)
}
