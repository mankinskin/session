impl SessionStoreConfig {
    /// Best-effort worktree/ticket inference at Copilot capture time
    /// (ticket bba9b313, root cause of e5f8a2c1's empty `sessions_for_ticket`
    /// results): the capture hook runs passively and never calls
    /// `check_in_worktree`, so a session otherwise carries no `branch`,
    /// `worktree_path`, or `ticket_id` at all.
    ///
    /// Resolves both from the current git environment using ONLY the branch
    /// name shape (never transcript text — spec e5f8a2c1 forbids transcript
    /// scanning for linkage at every tier) and reuses the backfill's
    /// short-id parser and ticket-store resolver so the two paths never
    /// diverge into separate parsing logic.
    ///
    /// A no-op whenever a worktree assignment already exists on the session:
    /// an explicit `check_in_worktree` (or a prior run of this same
    /// inference) always outranks a fresh guess. Never writes an
    /// unresolved ticket id — a branch shape that resolves to no real
    /// ticket leaves `ticket_id` untouched.
    ///
    /// Bootstraps a minimal record when the session has none yet: session
    /// initialization runs on the first user prompt, before any capture has
    /// persisted a record, and the assignment must exist before the first
    /// tool call routes on it. Nothing is written unless a branch resolves,
    /// so a non-git directory still leaves the store untouched.
    pub fn infer_worktree_from_environment(
        &self,
        session_id: &str,
        working_dir: &Path,
    ) -> Result<(), SessionError> {
        let mut record = match self.read_session(session_id) {
            Ok(record) => record,
            Err(SessionError::NotFound { .. }) => {
                self.new_inferred_record(session_id)
            },
            Err(error) => return Err(error),
        };
        if record.metadata.worktree.is_some()
            || self.worktree_registry_entry(session_id)?.is_some()
        {
            return Ok(());
        }

        let Some(branch) = current_git_branch(working_dir) else {
            return Ok(());
        };
        let worktree_path = current_git_toplevel(working_dir)
            .unwrap_or_else(|| working_dir.to_path_buf());

        let ticket_store_root = self.ticket_store_root();
        let ticket_store = if ticket_store_root.exists() {
            TicketStore::open(&ticket_store_root).ok()
        } else {
            None
        };
        let ticket_id = parse_agent_branch_short_id(&branch)
            .and_then(|short_id| {
                resolve_ticket_prefix(ticket_store.as_ref(), &short_id)
            });

        record.metadata.worktree = Some(SessionWorktreeAssignment {
            path: worktree_path,
            branch,
            allocation_mode: SessionWorktreeAllocationMode::New,
            status: SessionWorktreeStatus::Active,
            predecessor_session_id: None,
            predecessor_path: None,
        });
        if let Some(ticket_id) = ticket_id {
            record.metadata.ticket_id = Some(ticket_id);
        }
        record.captured_at = chrono::Utc::now();

        self.persist_record(record)?;
        Ok(())
    }

    /// Replaces a worktree inference only when the stored assignment points
    /// at the main checkout rather than a real worktree.
    ///
    /// Returns whether an existing main-checkout assignment was repaired.
    pub fn replace_main_worktree_inference(
        &self,
        session_id: &str,
        main_checkout: &Path,
        working_dir: &Path,
    ) -> Result<bool, SessionError> {
        let mut record = match self.read_session(session_id) {
            Ok(record) => record,
            Err(SessionError::NotFound { .. }) => return Ok(false),
            Err(error) => return Err(error),
        };
        let Some(existing_assignment) = &record.metadata.worktree else {
            return Ok(false);
        };
        if !paths_refer_to_same_directory(
            &existing_assignment.path,
            main_checkout,
        ) {
            return Ok(false);
        }

        let Some(branch) = current_git_branch(working_dir) else {
            return Ok(false);
        };
        let worktree_path = current_git_toplevel(working_dir)
            .unwrap_or_else(|| working_dir.to_path_buf());
        record.metadata.worktree = Some(SessionWorktreeAssignment {
            path: worktree_path,
            branch,
            allocation_mode: SessionWorktreeAllocationMode::New,
            status: SessionWorktreeStatus::Active,
            predecessor_session_id: None,
            predecessor_path: None,
        });
        record.captured_at = chrono::Utc::now();

        self.persist_record(record)?;
        Ok(true)
    }

    /// Writes a minimal registration record for a session the hook has just
    /// provisioned (or reused) a worktree for, on `self` — call this with a
    /// `SessionStoreConfig` pointed at the **main checkout's** `.session`
    /// store so the assignment is discoverable there even before, or
    /// regardless of, whatever the worktree's own store later captures
    /// (ticket 842d74cb D1: the main checkout is the authoritative
    /// session-to-worktree registry).
    ///
    /// A no-op when the session already carries a worktree assignment: this
    /// only ever bootstraps the first sighting of a session id and never
    /// overwrites a real capture or a later reassignment.
    pub fn register_provisioned_worktree(
        &self,
        session_id: &str,
        worktree_path: &Path,
        branch: &str,
        allocation_mode: SessionWorktreeAllocationMode,
    ) -> Result<(), SessionError> {
        let mut record = match self.read_session(session_id) {
            Ok(record) => record,
            Err(SessionError::NotFound { .. }) =>
                self.new_inferred_record(session_id),
            Err(error) => return Err(error),
        };
        if record.metadata.worktree.is_some() {
            return Ok(());
        }
        record.metadata.worktree = Some(SessionWorktreeAssignment {
            path: worktree_path.to_path_buf(),
            branch: branch.to_string(),
            allocation_mode,
            status: SessionWorktreeStatus::Active,
            predecessor_session_id: None,
            predecessor_path: None,
        });
        record.captured_at = chrono::Utc::now();
        self.persist_record(record)?;
        Ok(())
    }

    /// Minimal record for a session that has not been captured yet.
    fn new_inferred_record(
        &self,
        session_id: &str,
    ) -> SessionRecord {
        let now = chrono::Utc::now();
        SessionRecord {
            schema_version: SESSION_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            source: "session-worktree-inference".to_string(),
            started_at: now,
            captured_at: now,
            metadata: SessionMetadata {
                workspace_slug: self.workspace_slug.clone(),
                conversation_id: None,
                agent_id: None,
                ticket_id: None,
                model: None,
                trigger: Some("session-worktree-inference".to_string()),
                provisioning: None,
                producer: None,
                copilot_version: None,
                vscode_version: None,
                protocol_version: None,
                worktree: None,
            },
            turns: vec![],
            links: SessionLinks::default(),
            track_id: None,
            anchor_ticket_id: None,
            parent_session_id: None,
            spawned_session_id: None,
            emitted_handoff_ids: Vec::new(),
            picked_up_handoff_ids: Vec::new(),
        }
    }
}

/// Resolves the current branch via `git rev-parse --abbrev-ref HEAD`.
/// Returns `None` (quietly) for a non-git directory, a missing `git`
/// binary, or any other resolution failure. Returns `Some("HEAD")` for a
/// detached HEAD, which never matches the `agent/<short-id>-<slug>` shape
/// and so yields no ticket id downstream.
fn current_git_branch(working_dir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .current_dir(working_dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!branch.is_empty()).then_some(branch)
}

/// Resolves the working tree root via `git rev-parse --show-toplevel`.
/// Returns `None` when git is unavailable or the directory is not inside a
/// work tree; callers fall back to the raw working directory.
fn current_git_toplevel(working_dir: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .current_dir(working_dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!path.is_empty()).then_some(PathBuf::from(path))
}

fn paths_refer_to_same_directory(
    left: &Path,
    right: &Path,
) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left
            .to_string_lossy()
            .replace('\\', "/")
            .eq_ignore_ascii_case(&right.to_string_lossy().replace('\\', "/")),
    }
}
