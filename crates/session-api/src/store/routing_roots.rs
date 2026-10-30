#[allow(unused_imports)]
use super::*;

pub(super) fn sibling_store_base(session_store_root: &Path) -> PathBuf {
    let is_legacy_session_store = session_store_root
        .file_name()
        .and_then(|name| name.to_str())
        == Some(".session");
    let is_canonical_session_store = session_store_root
        .file_name()
        .and_then(|name| name.to_str())
        == Some("session")
        && session_store_root
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(memory_kernel::workspace::CANONICAL_STORES_DIR);
    if is_legacy_session_store || is_canonical_session_store {
        memory_kernel::workspace::resolve_workspace_root_from_store_root(
            session_store_root,
            ".session",
        )
    } else {
        session_store_root.to_path_buf()
    }
}

pub(super) fn sibling_store_root(session_store_root: &Path, sibling_store_dir: &str) -> PathBuf {
    let workspace = sibling_store_base(session_store_root);
    memory_kernel::workspace::resolve_store_root_at_fixed_workspace(&workspace, sibling_store_dir)
}

/// Resolves a URN workspace slug to a sibling store root. The literal slug
/// `default` and the session's own workspace slug both resolve to the
/// existing sibling store (byte-identical to pre-cross-workspace behavior);
/// any other slug resolves to `<base>/<slug>/<sibling_store_dir>` and is
/// validated to reject empty, `.`, `..`, and path-separator segments before
/// any path is built.
pub(super) fn resolve_workspace_store_root(
    session_store_root: &Path,
    own_workspace_path: &str,
    workspace_path: &str,
    sibling_store_dir: &str,
) -> Result<PathBuf, String> {
    let workspace_path = Path::new(workspace_path);
    if workspace_path.is_absolute() {
        return Ok(memory_kernel::workspace::resolve_store_root_from(
            workspace_path,
            sibling_store_dir,
        ));
    }

    let workspace = workspace_path.to_string_lossy();
    if workspace == "default" || workspace == own_workspace_path {
        return Ok(sibling_store_root(session_store_root, sibling_store_dir));
    }
    if workspace.is_empty()
        || matches!(workspace.as_ref(), "." | "..")
        || workspace.contains(['/', '\\'])
    {
        return Err(format!(
            "workspace path `{workspace}` contains invalid path characters"
        ));
    }
    let workspace_root = sibling_store_base(session_store_root).join(workspace.as_ref());
    Ok(
        memory_kernel::workspace::resolve_store_root_at_fixed_workspace(
            &workspace_root,
            sibling_store_dir,
        ),
    )
}
