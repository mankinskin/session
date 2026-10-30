#[allow(unused_imports)]
use super::*;

/// Session activity backed by records in a `.session` store.
pub struct SessionStoreActivity {
    pub(super) session_store: PathBuf,
    pub(super) stale_after: Duration,
}

impl SessionStoreActivity {
    pub fn new(session_store: impl Into<PathBuf>, stale_after: Duration) -> Self {
        Self {
            session_store: session_store.into(),
            stale_after,
        }
    }

    pub fn with_default_staleness(session_store: impl Into<PathBuf>) -> Self {
        Self::new(session_store, DEFAULT_STALE_AFTER)
    }

    pub(super) fn session_stores(&self) -> Result<Vec<PathBuf>, std::io::Error> {
        let mut stores = BTreeSet::from([self.session_store.clone()]);
        let Some(main_checkout) = session_workspace_root(&self.session_store) else {
            return Ok(stores.into_iter().collect());
        };
        let worktree_root = main_checkout.join(".worktrees");
        let entries = match fs::read_dir(&worktree_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(stores.into_iter().collect());
            }
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let store = existing_session_store(&path);
            if store.is_dir() {
                stores.insert(store);
                continue;
            }
            for nested in fs::read_dir(path)? {
                let nested = nested?;
                let store = existing_session_store(&nested.path());
                if store.is_dir() {
                    stores.insert(store);
                }
            }
        }
        Ok(stores.into_iter().collect())
    }
}

pub(super) fn existing_session_store(workspace: &Path) -> PathBuf {
    let canonical = workspace.join(".workflow-tools/session");
    if canonical.is_dir() {
        canonical
    } else {
        workspace.join(".session")
    }
}

pub(super) fn session_workspace_root(store: &Path) -> Option<PathBuf> {
    let is_canonical = store.file_name().and_then(|name| name.to_str()) == Some("session")
        && store
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(".workflow-tools");
    if is_canonical {
        store.parent()?.parent().map(Path::to_path_buf)
    } else {
        store.parent().map(Path::to_path_buf)
    }
}

impl SessionActivity for SessionStoreActivity {
    fn is_active(&self, worktree: &Path) -> bool {
        let Some(worktree) = normalized_path(worktree) else {
            return false;
        };
        let Ok(stores) = self.session_stores() else {
            return true;
        };
        stores
            .into_iter()
            .any(|store| match fs::read_dir(store.join("sessions")) {
                Ok(entries) => entries.flatten().any(|entry| {
                    session_record_is_active(
                        &entry.path().join("session.json"),
                        &worktree,
                        self.stale_after,
                    )
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(_) => true,
            })
    }

    fn worktree_ownership(&self, worktree: &Path) -> WorktreeOwnership {
        let Some(worktree) = normalized_path(worktree) else {
            return WorktreeOwnership::Ambiguous;
        };
        let Ok(stores) = self.session_stores() else {
            return WorktreeOwnership::Ambiguous;
        };
        let mut unreadable_store = false;
        let owners = stores
            .into_iter()
            .filter_map(|store| match fs::read_dir(store.join("sessions")) {
                Ok(entries) => Some(entries),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => {
                    unreadable_store = true;
                    None
                }
            })
            .flat_map(|entries| entries.flatten())
            .filter_map(|entry| session_record_owner(&entry.path().join("session.json"), &worktree))
            .collect::<BTreeSet<_>>();
        if unreadable_store {
            return WorktreeOwnership::Ambiguous;
        }
        match owners.len() {
            0 => WorktreeOwnership::Unowned,
            1 => WorktreeOwnership::Owned(owners.into_iter().next().unwrap()),
            _ => WorktreeOwnership::Ambiguous,
        }
    }
}

pub(super) fn session_record_is_active(path: &Path, worktree: &str, stale_after: Duration) -> bool {
    let Ok(record) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(record) = serde_json::from_str::<Value>(&record) else {
        return false;
    };
    let record_path = record
        .pointer("/metadata/worktree/path")
        .and_then(Value::as_str)
        .and_then(normalized_path);
    let timestamp = record
        .get("captured_at")
        .or_else(|| record.get("started_at"))
        .and_then(Value::as_str)
        .and_then(parse_timestamp);
    record_path.as_deref() == Some(worktree)
        && timestamp.is_some_and(|timestamp| timestamp_is_fresh(timestamp, stale_after))
}

pub(super) fn session_record_owner(path: &Path, worktree: &str) -> Option<String> {
    let record = fs::read_to_string(path).ok()?;
    let record = serde_json::from_str::<Value>(&record).ok()?;
    let record_path = record
        .pointer("/metadata/worktree/path")
        .and_then(Value::as_str)
        .and_then(normalized_path)?;
    let session_id = record.get("session_id")?.as_str()?;
    (record_path == worktree && !session_id.is_empty()).then(|| session_id.to_string())
}

pub(super) fn normalized_path(path: impl AsRef<Path>) -> Option<String> {
    fs::canonicalize(path)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/").to_lowercase())
}

pub(super) fn parse_timestamp(timestamp: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(timestamp, &Rfc3339).ok()
}

pub(super) fn timestamp_is_fresh(timestamp: OffsetDateTime, stale_after: Duration) -> bool {
    let now = OffsetDateTime::now_utc();
    if timestamp > now {
        return true;
    }
    (now - timestamp).unsigned_abs() <= stale_after
}
