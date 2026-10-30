#[allow(unused_imports)]
use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorktreeRegistryEntry {
    pub(crate) agent_id: String,
    pub(crate) ticket_id: String,
    pub(crate) assignment: SessionWorktreeAssignment,
}

#[derive(Debug)]
pub(super) struct WorktreeFileSnapshot {
    pub(super) path: PathBuf,
    pub(super) contents: Option<Vec<u8>>,
}

impl WorktreeFileSnapshot {
    pub(super) fn capture(path: PathBuf) -> Result<Self, SessionError> {
        let contents = match fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => return Err(SessionError::Io { path, source }),
        };
        Ok(Self { path, contents })
    }

    pub(super) fn restore(&self) -> Result<(), SessionError> {
        match &self.contents {
            Some(contents) => write_worktree_bytes(&self.path, contents),
            None => match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(SessionError::Io {
                    path: self.path.clone(),
                    source,
                }),
            },
        }
    }
}

pub(super) fn write_worktree_bytes(path: &Path, contents: &[u8]) -> Result<(), SessionError> {
    let parent = path
        .parent()
        .ok_or_else(|| SessionError::InvalidStorePath(path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|source| SessionError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("session"),
        uuid::Uuid::new_v4()
    ));
    {
        let mut file = fs::File::create(&temporary).map_err(|source| SessionError::Io {
            path: temporary.clone(),
            source,
        })?;
        use std::io::Write;
        file.write_all(contents)
            .map_err(|source| SessionError::Io {
                path: temporary.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| SessionError::Io {
            path: temporary.clone(),
            source,
        })?;
    }
    fs::rename(&temporary, path).map_err(|source| SessionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreeCheckInFailurePoint {
    AfterSuccessorRegistry,
    AfterPredecessorUpdate,
    AfterUnassignManifest,
}

#[cfg(test)]
thread_local! {
    static WORKTREE_CHECK_IN_FAILURE: std::cell::Cell<Option<WorktreeCheckInFailurePoint>> =
        const { std::cell::Cell::new(None) };
}

pub(crate) fn registry_path(session_store_root: &Path, session_id: &str) -> PathBuf {
    session_store_root
        .join("local/worktrees")
        .join(format!("{session_id}.json"))
}

pub(super) fn main_session_store_root(reference_store: &Path, main_checkout: &Path) -> PathBuf {
    let is_legacy = reference_store.file_name().and_then(|name| name.to_str()) == Some(".session");
    let is_canonical = reference_store.file_name().and_then(|name| name.to_str())
        == Some("session")
        && reference_store
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(memory_kernel::workspace::CANONICAL_STORES_DIR);
    if is_legacy {
        main_checkout.join(".session")
    } else if is_canonical {
        memory_kernel::workspace::canonical_store_root(main_checkout, ".session")
    } else {
        memory_kernel::workspace::resolve_store_root_at_fixed_workspace(main_checkout, ".session")
    }
}

pub(super) fn receipt_from_registry(
    session_id: &str,
    entry: WorktreeRegistryEntry,
) -> Result<SessionWorktreeCheckInReceipt, SessionError> {
    Ok(SessionWorktreeCheckInReceipt {
        session_id: session_id.to_string(),
        owner_id: entry.agent_id,
        ticket_id: entry.ticket_id,
        worktree_path: entry.assignment.path,
        branch: entry.assignment.branch,
        allocation_mode: entry.assignment.allocation_mode,
        status: entry.assignment.status,
        predecessor_session_id: entry.assignment.predecessor_session_id,
        predecessor_path: entry.assignment.predecessor_path,
    })
}

impl SessionStoreConfig {
    pub(crate) fn worktree_registry_entry(
        &self,
        session_id: &str,
    ) -> Result<Option<WorktreeRegistryEntry>, SessionError> {
        let main_checkout = self.main_checkout_for_store()?;
        let main_store = main_session_store_root(&self.root, &main_checkout);
        match read_json(&registry_path(&main_store, session_id)) {
            Ok(entry) => Ok(Some(entry)),
            Err(SessionError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) fn main_checkout_for_store(&self) -> Result<PathBuf, SessionError> {
        let is_session_store = self.root.file_name().and_then(|name| name.to_str())
            == Some(".session")
            || (self.root.file_name().and_then(|name| name.to_str()) == Some("session")
                && self
                    .root
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    == Some(memory_kernel::workspace::CANONICAL_STORES_DIR));
        if is_session_store {
            let workspace = memory_kernel::workspace::resolve_workspace_root_from_store_root(
                &self.root, ".session",
            );
            return Ok(workspace
                .ancestors()
                .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(".worktrees"))
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or(workspace));
        }
        self.root
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| SessionError::InvalidStorePath(self.root.clone()))
    }

    pub(super) fn validate_managed_worktree(
        &self,
        request: &SessionWorktreeCheckInRequest,
    ) -> Result<(PathBuf, PathBuf), SessionError> {
        let worktree_path = canonicalize_worktree_path(&request.worktree_path)?;
        let repository = git2::Repository::discover(&worktree_path).map_err(|error| {
            SessionError::InvalidManagedWorktree {
                path: worktree_path.clone(),
                reason: error.message().to_string(),
            }
        })?;
        let repository_workdir =
            repository
                .workdir()
                .ok_or_else(|| SessionError::InvalidManagedWorktree {
                    path: worktree_path.clone(),
                    reason: "bare repositories cannot be session worktrees".to_string(),
                })?;
        if canonicalize_worktree_path(repository_workdir)? != worktree_path {
            return Err(SessionError::InvalidManagedWorktree {
                path: worktree_path,
                reason: "path is not the worktree root".to_string(),
            });
        }
        let main_checkout = repository.commondir().parent().ok_or_else(|| {
            SessionError::InvalidManagedWorktree {
                path: worktree_path.clone(),
                reason: "repository common directory has no checkout parent".to_string(),
            }
        })?;
        let main_checkout = canonicalize_worktree_path(main_checkout)?;
        let managed_root = main_checkout.join(".worktrees");
        let relative = worktree_path.strip_prefix(&managed_root).map_err(|_| {
            SessionError::InvalidManagedWorktree {
                path: worktree_path.clone(),
                reason: "worktree is outside the owning checkout's .worktrees directory"
                    .to_string(),
            }
        })?;
        let mut components = relative.components();
        let session_component = components
            .next()
            .and_then(|component| component.as_os_str().to_str());
        let slug_component = components
            .next()
            .and_then(|component| component.as_os_str().to_str());
        if session_component != Some(request.session_id.as_str())
            && request.predecessor_session_id.as_deref() != session_component
            || slug_component.is_none_or(str::is_empty)
            || components.next().is_some()
        {
            return Err(SessionError::InvalidManagedWorktree {
                path: worktree_path,
                reason: "managed path must be .worktrees/<session-id>/<slug>".to_string(),
            });
        }
        let branch = repository
            .head()
            .map_err(|error| SessionError::InvalidManagedWorktree {
                path: request.worktree_path.clone(),
                reason: error.message().to_string(),
            })?
            .shorthand()
            .map(str::to_string)
            .ok_or_else(|| SessionError::InvalidManagedWorktree {
                path: request.worktree_path.clone(),
                reason: "worktree must have a named branch".to_string(),
            })?;
        if branch != request.branch {
            return Err(SessionError::WorktreeBranchMismatch {
                path: worktree_path,
                expected: request.branch.clone(),
                actual: branch,
            });
        }
        Ok((main_checkout, worktree_path))
    }

    pub(super) fn persist_worktree_manifest(
        &self,
        request: &SessionWorktreeCheckInRequest,
        main_checkout: &Path,
    ) -> Result<(), SessionError> {
        let mut record = self
            .read_session_manifest(&request.session_id)
            .unwrap_or(SessionRecord {
                schema_version: SESSION_SCHEMA_VERSION,
                session_id: request.session_id.clone(),
                source: "session-worktree-check-in".to_string(),
                started_at: chrono::Utc::now(),
                captured_at: chrono::Utc::now(),
                metadata: SessionMetadata {
                    workspace_path: self.workspace_path.clone(),
                    conversation_id: None,
                    agent_id: None,
                    ticket_id: None,
                    model: None,
                    trigger: Some("session-check-in".to_string()),
                    provisioning: None,
                    producer: None,
                    copilot_version: None,
                    vscode_version: None,
                    protocol_version: None,
                    worktree: None,
                },
                turns: Vec::new(),
                links: SessionLinks::default(),
                track_id: None,
                anchor_ticket_id: None,
                parent_session_id: None,
                spawned_session_id: None,
                emitted_handoff_ids: Vec::new(),
                picked_up_handoff_ids: Vec::new(),
            });
        record.captured_at = chrono::Utc::now();
        record.metadata.agent_id = None;
        record.metadata.ticket_id = None;
        record.metadata.worktree = Some(SessionWorktreeAssignment {
            path: PathBuf::new(),
            branch: request.branch.clone(),
            allocation_mode: SessionWorktreeAllocationMode::New,
            status: SessionWorktreeStatus::Active,
            predecessor_session_id: None,
            predecessor_path: None,
        });
        self.persist_branch_only_manifest(&record)?;
        let main_store = main_session_store_root(&self.root, main_checkout);
        if main_store != self.root {
            SessionStoreConfig::new(main_store).persist_branch_only_manifest(&record)?;
        }
        Ok(())
    }

    pub(super) fn persist_branch_only_manifest(
        &self,
        record: &SessionRecord,
    ) -> Result<(), SessionError> {
        self.persist_record(record.clone())?;
        let paths = self.paths_for_session_id(&record.session_id)?;
        let mut manifest: serde_json::Value = read_json(&paths.manifest_path)?;
        let metadata =
            manifest["metadata"]
                .as_object_mut()
                .ok_or_else(|| SessionError::Deserialize {
                    path: paths.manifest_path.clone(),
                    source: serde_json::from_str::<serde_json::Value>("invalid").unwrap_err(),
                })?;
        metadata.remove("agent_id");
        metadata.remove("ticket_id");
        if let Some(worktree) = metadata
            .get_mut("worktree")
            .and_then(serde_json::Value::as_object_mut)
        {
            worktree.retain(|key, _| key == "branch");
        }
        write_json(&paths.manifest_path, &manifest)
    }

    pub(super) fn worktree_mutation_snapshots(
        &self,
        main_checkout: &Path,
        session_id: &str,
        predecessor_session_id: Option<&str>,
    ) -> Result<Vec<WorktreeFileSnapshot>, SessionError> {
        let mut paths = vec![
            registry_path(
                &main_session_store_root(&self.root, main_checkout),
                session_id,
            ),
            self.paths_for_session_id(session_id)?.manifest_path,
        ];
        let main_store = main_session_store_root(&self.root, main_checkout);
        if main_store != self.root {
            paths.push(
                SessionStoreConfig::new(&main_store)
                    .paths_for_session_id(session_id)?
                    .manifest_path,
            );
        }
        if let Some(predecessor_session_id) = predecessor_session_id {
            paths.push(registry_path(&main_store, predecessor_session_id));
        }
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(WorktreeFileSnapshot::capture)
            .collect()
    }

    pub(super) fn restore_worktree_snapshots(
        snapshots: &[WorktreeFileSnapshot],
    ) -> Result<(), SessionError> {
        for snapshot in snapshots.iter().rev() {
            snapshot.restore()?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_worktree_check_in_failure(&self, point: Option<WorktreeCheckInFailurePoint>) {
        WORKTREE_CHECK_IN_FAILURE.with(|failure| failure.set(point));
    }

    #[cfg(test)]
    pub(super) fn inject_worktree_check_in_failure(
        &self,
        point: WorktreeCheckInFailurePoint,
    ) -> Result<(), SessionError> {
        if WORKTREE_CHECK_IN_FAILURE.with(|failure| {
            if failure.get() == Some(point) {
                failure.set(None);
                true
            } else {
                false
            }
        }) {
            return Err(SessionError::Io {
                path: self.root.clone(),
                source: std::io::Error::other("injected worktree check-in failure"),
            });
        }
        Ok(())
    }
}

#[path = "worktree_runtime/assignment.rs"]
mod assignment;

#[path = "worktree_runtime/runtime_context.rs"]
mod runtime_context;
