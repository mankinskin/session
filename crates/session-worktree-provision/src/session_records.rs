#[allow(unused_imports)]
use super::*;

pub(super) fn owned_session_directory(
    main_checkout: &Path,
    worktree: &Path,
    owner_session_id: &str,
) -> Option<PathBuf> {
    let relative = worktree
        .strip_prefix(main_checkout.join(".worktrees"))
        .ok()?;
    let mut components = relative.components();
    let session_id = components.next()?.as_os_str().to_str()?;
    components.next()?;
    if components.next().is_some() || session_id != owner_session_id {
        return None;
    }
    let canonical = PathBuf::from(".workflow-tools/session/sessions").join(owner_session_id);
    let legacy = PathBuf::from(".session/sessions").join(owner_session_id);
    Some(if worktree.join(&canonical).exists() {
        canonical
    } else {
        legacy
    })
}

pub(super) fn existing_session_directory(workspace: &Path, session_id: &str) -> PathBuf {
    let canonical = workspace
        .join(".workflow-tools/session/sessions")
        .join(session_id);
    if canonical.exists() {
        canonical
    } else {
        workspace.join(".session/sessions").join(session_id)
    }
}

pub(super) fn session_record_matches_owner(
    record: &Path,
    owner_session_id: &str,
    worktree: &Path,
) -> bool {
    let Ok(record) = fs::read_to_string(record) else {
        return false;
    };
    let Ok(record) = serde_json::from_str::<Value>(&record) else {
        return false;
    };
    let session_id = record.get("session_id").and_then(Value::as_str);
    let record_worktree = record
        .pointer("/metadata/worktree/path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    session_id == Some(owner_session_id)
        && record_worktree.is_some_and(|path| paths_equal(&path, worktree))
}
