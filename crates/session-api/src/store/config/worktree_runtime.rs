#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorktreeRegistryEntry {
    pub(crate) agent_id: String,
    pub(crate) ticket_id: String,
    pub(crate) assignment: SessionWorktreeAssignment,
}

#[derive(Debug)]
struct WorktreeFileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl WorktreeFileSnapshot {
    fn capture(path: PathBuf) -> Result<Self, SessionError> {
        let contents = match fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => return Err(SessionError::Io { path, source }),
        };
        Ok(Self { path, contents })
    }

    fn restore(&self) -> Result<(), SessionError> {
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

fn write_worktree_bytes(path: &Path, contents: &[u8]) -> Result<(), SessionError> {
    let parent = path
        .parent()
        .ok_or_else(|| SessionError::InvalidStorePath(path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|source| SessionError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("session"),
        uuid::Uuid::new_v4()
    ));
    {
        let mut file = fs::File::create(&temporary).map_err(|source| SessionError::Io {
            path: temporary.clone(),
            source,
        })?;
        use std::io::Write;
        file.write_all(contents).map_err(|source| SessionError::Io {
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
}

#[cfg(test)]
thread_local! {
    static WORKTREE_CHECK_IN_FAILURE: std::cell::Cell<Option<WorktreeCheckInFailurePoint>> =
        const { std::cell::Cell::new(None) };
}

pub(crate) fn registry_path(main_checkout: &Path, session_id: &str) -> PathBuf {
    main_checkout
        .join(".session/local/worktrees")
        .join(format!("{session_id}.json"))
}

fn receipt_from_registry(
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
        match read_json(&registry_path(&main_checkout, session_id)) {
            Ok(entry) => Ok(Some(entry)),
            Err(SessionError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn main_checkout_for_store(&self) -> Result<PathBuf, SessionError> {
        let checkout = self.root.parent().ok_or_else(|| {
            SessionError::InvalidStorePath(self.root.clone())
        })?;
        Ok(checkout
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == ".worktrees"))
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| checkout.to_path_buf()))
    }

    fn main_checkout_for(&self, worktree_path: &Path) -> Result<PathBuf, SessionError> {
        if let Some(main_checkout) = worktree_path
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == ".worktrees"))
            .and_then(Path::parent)
        {
            return Ok(main_checkout.to_path_buf());
        }
        self.main_checkout_for_store()
    }

    fn validate_managed_worktree(
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
        let repository_workdir = repository.workdir().ok_or_else(|| {
            SessionError::InvalidManagedWorktree {
                path: worktree_path.clone(),
                reason: "bare repositories cannot be session worktrees".to_string(),
            }
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
                reason: "worktree is outside the owning checkout's .worktrees directory".to_string(),
            }
        })?;
        let mut components = relative.components();
        let session_component = components.next().and_then(|component| component.as_os_str().to_str());
        let slug_component = components.next().and_then(|component| component.as_os_str().to_str());
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
        let branch = repository.head().map_err(|error| SessionError::InvalidManagedWorktree {
            path: request.worktree_path.clone(),
            reason: error.message().to_string(),
        })?.shorthand().map(str::to_string).ok_or_else(|| SessionError::InvalidManagedWorktree {
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

    fn persist_worktree_manifest(
        &self,
        request: &SessionWorktreeCheckInRequest,
        main_checkout: &Path,
    ) -> Result<(), SessionError> {
        let mut record = self.read_session_manifest(&request.session_id).unwrap_or(SessionRecord {
            schema_version: SESSION_SCHEMA_VERSION,
            session_id: request.session_id.clone(),
            source: "session-worktree-check-in".to_string(),
            started_at: chrono::Utc::now(),
            captured_at: chrono::Utc::now(),
            metadata: SessionMetadata {
                workspace_slug: self.workspace_slug.clone(),
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
        let main_store = main_checkout.join(".session");
        if main_store != self.root {
            SessionStoreConfig::new(main_store, self.workspace_slug.clone())
                .persist_branch_only_manifest(&record)?;
        }
        Ok(())
    }

    fn persist_branch_only_manifest(
        &self,
        record: &SessionRecord,
    ) -> Result<(), SessionError> {
        self.persist_record(record.clone())?;
        let paths = self.paths_for_session_id(&record.session_id)?;
        let mut manifest: serde_json::Value = read_json(&paths.manifest_path)?;
        let metadata = manifest["metadata"].as_object_mut().ok_or_else(|| {
            SessionError::Deserialize {
                path: paths.manifest_path.clone(),
                source: serde_json::from_str::<serde_json::Value>("invalid").unwrap_err(),
            }
        })?;
        metadata.remove("agent_id");
        metadata.remove("ticket_id");
        if let Some(worktree) = metadata.get_mut("worktree").and_then(serde_json::Value::as_object_mut) {
            worktree.retain(|key, _| key == "branch");
        }
        write_json(&paths.manifest_path, &manifest)
    }

    fn worktree_mutation_snapshots(
        &self,
        main_checkout: &Path,
        session_id: &str,
        predecessor_session_id: Option<&str>,
    ) -> Result<Vec<WorktreeFileSnapshot>, SessionError> {
        let mut paths = vec![
            registry_path(main_checkout, session_id),
            self.paths_for_session_id(session_id)?.manifest_path,
        ];
        let main_store = main_checkout.join(".session");
        if main_store != self.root {
            paths.push(
                SessionStoreConfig::new(main_store, self.workspace_slug.clone())
                    .paths_for_session_id(session_id)?
                    .manifest_path,
            );
        }
        if let Some(predecessor_session_id) = predecessor_session_id {
            paths.push(registry_path(main_checkout, predecessor_session_id));
        }
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(WorktreeFileSnapshot::capture)
            .collect()
    }

    fn restore_worktree_snapshots(
        snapshots: &[WorktreeFileSnapshot],
    ) -> Result<(), SessionError> {
        for snapshot in snapshots.iter().rev() {
            snapshot.restore()?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_worktree_check_in_failure(
        &self,
        point: Option<WorktreeCheckInFailurePoint>,
    ) {
        WORKTREE_CHECK_IN_FAILURE.with(|failure| failure.set(point));
    }

    #[cfg(test)]
    fn inject_worktree_check_in_failure(
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

    pub fn check_in_worktree(
        &self,
        mut request: SessionWorktreeCheckInRequest,
    ) -> Result<SessionWorktreeCheckInReceipt, SessionError> {
        validate_worktree_request(&request)?;
        let (main_checkout, worktree_path) = self.validate_managed_worktree(&request)?;
        request.worktree_path = worktree_path;
        let registry_file = registry_path(&main_checkout, &request.session_id);
        let mut replaced_assignment = None;
        if let Ok(mut existing) = read_json::<WorktreeRegistryEntry>(&registry_file) {
            if existing.agent_id != request.owner_id || existing.ticket_id != request.ticket_id {
                return Err(SessionError::SessionOwnershipMismatch {
                    session_id: request.session_id,
                });
            }
            if can_reuse_assignment(&existing.assignment, &request) {
                existing.assignment.allocation_mode = SessionWorktreeAllocationMode::Reused;
                let snapshots = self.worktree_mutation_snapshots(
                    &main_checkout,
                    &request.session_id,
                    None,
                )?;
                let result = (|| {
                    write_json(&registry_file, &existing)?;
                    #[cfg(test)]
                    self.inject_worktree_check_in_failure(
                        WorktreeCheckInFailurePoint::AfterSuccessorRegistry,
                    )?;
                    self.persist_worktree_manifest(&request, &main_checkout)
                })();
                if let Err(error) = result {
                    Self::restore_worktree_snapshots(&snapshots)?;
                    return Err(error);
                }
                return receipt_from_registry(&request.session_id, existing);
            }
            if existing.assignment.status == SessionWorktreeStatus::Active
                && !existing.assignment.path.exists()
            {
                replaced_assignment = Some(existing.assignment);
            }
        }

        let mut predecessor_path = replaced_assignment
            .as_ref()
            .map(|assignment| assignment.path.clone());
        let mut predecessor_update = None;
        if let Some(predecessor_session_id) = &request.predecessor_session_id {
            let predecessor_registry = registry_path(&main_checkout, predecessor_session_id);
            let mut predecessor = read_json::<WorktreeRegistryEntry>(&predecessor_registry)?;
            let predecessor_assignment = predecessor.assignment.clone();

            if canonicalize_worktree_path(&predecessor_assignment.path)? == request.worktree_path {
                return Err(SessionError::CrossSessionReuseRequiresAdopt {
                    session_id: predecessor_session_id.clone(),
                    path: predecessor_assignment.path,
                });
            }

            predecessor_path = Some(predecessor_assignment.path.clone());
            predecessor.assignment.status = SessionWorktreeStatus::Superseded;
            predecessor_update = Some((predecessor_registry, predecessor));
        }

        self.ensure_no_active_worktree_conflict(
            &main_checkout,
            &request.worktree_path,
            &[
                request.session_id.as_str(),
                request.predecessor_session_id.as_deref().unwrap_or_default(),
            ],
        )?;

        let entry = WorktreeRegistryEntry {
            agent_id: request.owner_id.clone(),
            ticket_id: request.ticket_id.clone(),
            assignment: SessionWorktreeAssignment {
                path: request.worktree_path.clone(),
                branch: request.branch.clone(),
                allocation_mode: if request.predecessor_session_id.is_some()
                    || replaced_assignment.is_some()
                {
                    SessionWorktreeAllocationMode::Rotated
                } else {
                    SessionWorktreeAllocationMode::New
                },
                status: SessionWorktreeStatus::Active,
                predecessor_session_id: request.predecessor_session_id.clone(),
                predecessor_path,
            },
        };
        let snapshots = self.worktree_mutation_snapshots(
            &main_checkout,
            &request.session_id,
            request.predecessor_session_id.as_deref(),
        )?;
        let result = (|| {
            write_json(&registry_file, &entry)?;
            #[cfg(test)]
            self.inject_worktree_check_in_failure(
                WorktreeCheckInFailurePoint::AfterSuccessorRegistry,
            )?;
            self.persist_worktree_manifest(&request, &main_checkout)?;
            if let Some((predecessor_registry, predecessor)) = &predecessor_update {
                write_json(predecessor_registry, predecessor)?;
                #[cfg(test)]
                self.inject_worktree_check_in_failure(
                    WorktreeCheckInFailurePoint::AfterPredecessorUpdate,
                )?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            Self::restore_worktree_snapshots(&snapshots)?;
            return Err(error);
        }
        receipt_from_registry(&request.session_id, entry)
    }

    pub fn lookup_worktree(
        &self,
        session_id: &str,
    ) -> Result<SessionWorktreeCheckInReceipt, SessionError> {
        let manifest = self.read_session_manifest(session_id)?;
        let main_checkout = self.main_checkout_for_store()?;
        let registry_path = registry_path(&main_checkout, session_id);
        let entry: WorktreeRegistryEntry = read_json(&registry_path).map_err(|error| match error {
            SessionError::NotFound { .. } => SessionError::MissingWorktreeAssignment { session_id: session_id.to_string() },
            other => other,
        })?;
        if !entry.assignment.path.exists() {
            return Err(SessionError::RegisteredWorktreeMissing { session_id: session_id.to_string(), path: entry.assignment.path.clone() });
        }
        if manifest.metadata.worktree.as_ref().map(|worktree| &worktree.branch) != Some(&entry.assignment.branch) {
            return Err(SessionError::MissingWorktreeAssignment { session_id: session_id.to_string() });
        }
        receipt_from_registry(session_id, entry)
    }

    /// Return a bounded window of transcript turns for a persisted session.
    pub fn peek_range(
        &self,
        session_id: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<SessionTurnRange, SessionError> {
        let record = self.read_session(session_id)?;
        Ok(peek_turn_range(&record, start, end))
    }

    /// Return a body-stripped skeleton overview of a persisted session.
    pub fn peek_skeleton(
        &self,
        session_id: &str,
        preview_chars: usize,
    ) -> Result<SessionSkeleton, SessionError> {
        let record = self.read_session(session_id)?;
        Ok(peek_skeleton(&record, preview_chars))
    }

    /// Return a prompt-facing compact view of a persisted session transcript.
    pub fn peek_prompt_pack(
        &self,
        session_id: &str,
        options: PromptPackOptions,
    ) -> Result<SessionPromptPack, SessionError> {
        let record = self.read_session(session_id)?;
        Ok(peek_prompt_pack(&record, options))
    }

    pub fn init_runtime_context(
        &self,
        request: SessionRuntimeInitRequest,
    ) -> Result<SessionRuntimeInitResult, SessionError> {
        let now = chrono::Utc::now();
        let session_id =
            self.resolve_session_id(request.session_id)?;

        // Serialize lineage updates with every other runtime mutation and with
        // finish. Without the lock, a concurrent pin/workflow mutation (or a
        // second init/resume) could read the same context and clobber the run
        // lineage this call appends.
        let _lock = self.acquire_runtime_lock(&session_id)?;

        let mut created_workspace = false;
        let mut created_run = false;

        let mut context = match self.read_runtime_context(&session_id)
        {
            Ok(context) => context,
            Err(SessionError::RuntimeContextNotFound { .. }) => {
                created_workspace = true;
                created_run = true;
                let run = SessionRunLineage {
                    run_id: Uuid::new_v4().to_string(),
                    predecessor_run_id: request.predecessor_run_id.clone(),
                    captured_session_id: Some(session_id.clone()),
                    started_at: now,
                };

                SessionRuntimeContext {
                    session_id: session_id.clone(),
                    created_at: now,
                    updated_at: now,
                    active_run_id: run.run_id.clone(),
                    runs: vec![run],
                    pinned_entities: vec![],
                    workflow: Default::default(),
                }
            },
            Err(err) => return Err(err),
        };

        if !created_workspace
            && self
                .runtime_paths_for_workspace(&session_id)?
                .finish_path
                .exists()
        {
            if request.force_new_run || request.predecessor_run_id.is_some() {
                return Err(SessionError::WorkspaceFinished {
                    session_id,
                });
            }

            let run = context.active_run().cloned().ok_or_else(|| {
                SessionError::RuntimeContextNotFound {
                    session_id: session_id.clone(),
                }
            })?;
            return Ok(SessionRuntimeInitResult {
                context,
                run,
                created_workspace: false,
                created_run: false,
            });
        }

        if !created_workspace {
            let predecessor = request
                .predecessor_run_id
                .clone()
                .or_else(|| context.active_run().map(|run| run.run_id.clone()));

            if request.force_new_run || request.predecessor_run_id.is_some() {
                // Appending a new run is a lineage mutation; a finished workspace
                // is immutable, so reject it under the lock.
                self.ensure_workspace_not_finished(&session_id)?;
                let run = SessionRunLineage {
                    run_id: Uuid::new_v4().to_string(),
                    predecessor_run_id: predecessor,
                    captured_session_id: Some(context.canonical_session_id()),
                    started_at: now,
                };
                context.active_run_id = run.run_id.clone();
                context.runs.push(run);
                created_run = true;
            }

            context.updated_at = now;
        }

        self.persist_runtime_state(&context)?;

        let run = context.active_run().cloned().ok_or_else(|| {
            SessionError::RuntimeContextNotFound {
                session_id: session_id.clone(),
            }
        })?;

        Ok(SessionRuntimeInitResult {
            context,
            run,
            created_workspace,
            created_run,
        })
    }

    pub fn read_runtime_context(
        &self,
        session_id: &str,
    ) -> Result<SessionRuntimeContext, SessionError> {
        validate_session_id(session_id)?;
        let session_paths = self.paths_for_session_id(session_id)?;
        let legacy = self.read_legacy_runtime_context(session_id)?;
        let mut manifest = match read_json_if_exists(&session_paths.manifest_path)? {
            Some(manifest) => manifest,
            None if legacy.is_some() => PersistedSessionManifest {
                schema_version: SESSION_SCHEMA_VERSION,
                session_id: session_id.to_string(),
                source: "legacy-runtime-context".to_string(),
                started_at: chrono::Utc::now(),
                captured_at: chrono::Utc::now(),
                metadata: SessionMetadata {
                    workspace_slug: self.workspace_slug.clone(),
                    conversation_id: None,
                    agent_id: None,
                    ticket_id: None,
                    model: None,
                    trigger: None,
                    provisioning: None,
                    producer: None,
                    copilot_version: None,
                    vscode_version: None,
                    protocol_version: None,
                    worktree: None,
                },
                links: SessionLinks::default(),
                track_id: None,
                anchor_ticket_id: None,
                parent_session_id: None,
                spawned_session_id: None,
                emitted_handoff_ids: Vec::new(),
                picked_up_handoff_ids: Vec::new(),
                active_run_id: String::new(),
                runs: Vec::new(),
                pinned_entities: Vec::new(),
                workflow: SessionWorkflowGraph::default(),
            },
            None => {
                return Err(SessionError::RuntimeContextNotFound {
                    session_id: session_id.to_string(),
                });
            }
        };

        ensure_supported_schema_version(
            &session_paths.manifest_path,
            manifest.schema_version,
        )?;

        let mut created_at = manifest.started_at;
        let mut updated_at = manifest.captured_at;
        if let Some(legacy) = legacy {
            if manifest.active_run_id.is_empty() {
                manifest.active_run_id = legacy.active_run_id;
            }
            if manifest.runs.is_empty() {
                manifest.runs = legacy.runs;
            }
            if manifest.pinned_entities.is_empty() {
                manifest.pinned_entities = legacy.pinned_entities;
            }
            if manifest.workflow.is_empty() {
                manifest.workflow = legacy.workflow;
            }
            created_at = legacy.created_at.unwrap_or(created_at);
            updated_at = legacy.updated_at.unwrap_or(updated_at);
        }

        if manifest.active_run_id.is_empty() && manifest.runs.is_empty() {
            return Err(SessionError::RuntimeContextNotFound {
                session_id: session_id.to_string(),
            });
        }

        Ok(SessionRuntimeContext {
            session_id: session_id.to_string(),
            created_at,
            updated_at,
            active_run_id: manifest.active_run_id,
            runs: manifest.runs,
            pinned_entities: manifest.pinned_entities,
            workflow: manifest.workflow,
        })
    }

}
