use super::*;
use events::{
    canonicalize_captured_events,
    captured_event_key,
};
use links::extend_unique;

/// Discover the repository root so handoff package paths can be verified as
/// repo-root-relative, regardless of where the session store root happens to
/// live (a temp dir in tests, a nested `.session` dir in production).
///
/// `CARGO_MANIFEST_DIR` is baked in at compile time to this crate's own
/// checkout location. This repo nests submodules at multiple levels (e.g.
/// `context-engine` is itself a submodule of an outer repo, and `memory-api`
/// has its own `.git`, each also carrying its own `AGENTS.md`), so neither
/// `.git` nor `AGENTS.md` presence alone can identify the correct root.
/// `repo_map.toon` lives at this repo's root uniquely (see the ticket
/// `fb14754e` problem statement), so prefer it and fall back to the nearest
/// `.git` ancestor if it is absent.
pub(super) fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut dir = manifest_dir.as_path();
    loop {
        if dir.join("repo_map.toon").is_file() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    let mut dir = manifest_dir.as_path();
    loop {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return manifest_dir,
        }
    }
}

/// Normalize a path to forward-slash form so persisted handoff paths are
/// portable across platforms (AC1: repo-root-relative, forward-slash).
pub(super) fn normalize_repo_relative_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// Heuristic: does a `context_anchors` entry look like a physical path
/// (rather than a free-form identifier like `spec:5e52039d` or a URN like
/// `ce://default/ticket/<id>`)? Only path-shaped anchors are subject to
/// existence verification; the field also carries non-path context notes.
pub(super) fn looks_like_path(anchor: &str) -> bool {
    if anchor.contains("://") {
        return false;
    }
    if let Some(idx) = anchor.find(':') {
        if !anchor[..idx].contains('/') {
            return false;
        }
    }
    anchor.contains('/')
}

/// Verify a repo-root-relative path exists on disk under `root`, rejecting
/// absolute paths and parent-directory escapes (AC2: fail at creation time).
pub(super) fn verify_repo_relative_path_exists(
    root: &Path,
    normalized: &str,
) -> bool {
    if normalized.starts_with('/') || normalized.contains("..") {
        return false;
    }
    root.join(normalized).exists()
}

/// Remove `path` if it exists. Used to keep optional sidecar files (such as
/// `tool-metrics.json`) from lingering once they would only hold empty data.
pub(super) fn remove_file_if_exists(path: &Path) -> Result<(), SessionError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(SessionError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub(super) fn ensure_local_gitignore(
    store_root: &Path,
) -> Result<(), SessionError> {
    fs::create_dir_all(store_root).map_err(|source| SessionError::Io {
        path: store_root.to_path_buf(),
        source,
    })?;

    let ignore_path = store_root.join(".gitignore");
    let contents = match fs::read_to_string(&ignore_path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            String::new()
        }
        Err(source) => {
            return Err(SessionError::Io {
                path: ignore_path,
                source,
            });
        }
    };
    if contents.lines().any(|line| line.trim() == "local/") {
        return Ok(());
    }

    let separator = if contents.is_empty() || contents.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    fs::write(&ignore_path, format!("{contents}{separator}local/\n")).map_err(
        |source| SessionError::Io {
            path: ignore_path,
            source,
        },
    )
}

pub(super) fn write_json<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), SessionError> {
    let parent = path
        .parent()
        .ok_or_else(|| SessionError::InvalidStorePath(path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|source| SessionError::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    let encoded = serde_json::to_vec_pretty(value).map_err(|source| {
        SessionError::Serialize {
            path: path.to_path_buf(),
            source,
        }
    })?;

    let tmp_path = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("session"),
        uuid::Uuid::new_v4()
    ));

    {
        let mut file =
            fs::File::create(&tmp_path).map_err(|source| SessionError::Io {
                path: tmp_path.clone(),
                source,
            })?;
        use std::io::Write;
        file.write_all(&encoded)
            .map_err(|source| SessionError::Io {
                path: tmp_path.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| SessionError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    }

    // The temp file is fully written and synced before replacement. Replacement
    // semantics are those of `std::fs::rename` on the current platform; on error,
    // the previous destination is left untouched. This does not claim that Windows
    // replacement or the final directory entry is power-loss durable.
    fs::rename(&tmp_path, path).map_err(|source| SessionError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    #[cfg(unix)]
    {
        let parent_dir =
            fs::File::open(parent).map_err(|source| SessionError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        parent_dir.sync_all().map_err(|source| SessionError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    Ok(())
}

pub(super) fn read_json<T: DeserializeOwned>(
    path: &Path
) -> Result<T, SessionError> {
    let encoded = fs::read(path).map_err(|source| match source.kind() {
        ErrorKind::NotFound => SessionError::NotFound {
            path: path.to_path_buf(),
        },
        _ => SessionError::Io {
            path: path.to_path_buf(),
            source,
        },
    })?;
    serde_json::from_slice(&encoded).map_err(|source| {
        SessionError::Deserialize {
            path: path.to_path_buf(),
            source,
        }
    })
}

pub(super) fn read_json_if_exists<T: DeserializeOwned>(
    path: &Path
) -> Result<Option<T>, SessionError> {
    match fs::read(path) {
        Ok(encoded) =>
            serde_json::from_slice(&encoded)
                .map(Some)
                .map_err(|source| SessionError::Deserialize {
                    path: path.to_path_buf(),
                    source,
                }),
        Err(source) if source.kind() == ErrorKind::NotFound => Ok(None),
        Err(source) => Err(SessionError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub(super) fn merge_manifest(
    existing: Option<PersistedSessionManifest>,
    mut incoming: PersistedSessionManifest,
) -> PersistedSessionManifest {
    if let Some(existing) = existing {
        if existing.started_at < incoming.started_at {
            incoming.started_at = existing.started_at;
        }
        if existing.captured_at > incoming.captured_at {
            incoming.captured_at = existing.captured_at;
        }
        incoming.metadata =
            merge_metadata(existing.metadata, incoming.metadata);
        incoming.links = merge_links(existing.links, incoming.links);

        // Preserve track fields: prefer incoming non-None, else keep existing
        incoming.track_id = incoming.track_id.or(existing.track_id);
        incoming.anchor_ticket_id =
            incoming.anchor_ticket_id.or(existing.anchor_ticket_id);
        incoming.parent_session_id =
            incoming.parent_session_id.or(existing.parent_session_id);
        incoming.spawned_session_id =
            incoming.spawned_session_id.or(existing.spawned_session_id);
        extend_unique(
            &mut incoming.emitted_handoff_ids,
            existing.emitted_handoff_ids,
        );
        extend_unique(
            &mut incoming.picked_up_handoff_ids,
            existing.picked_up_handoff_ids,
        );
        if incoming.active_run_id.is_empty() {
            incoming.active_run_id = existing.active_run_id;
        }
        if incoming.runs.is_empty() {
            incoming.runs = existing.runs;
        }
        if incoming.pinned_entities.is_empty() {
            incoming.pinned_entities = existing.pinned_entities;
        }
        if incoming.workflow.is_empty() {
            incoming.workflow = existing.workflow;
        }
    }

    incoming
}

pub(super) fn merge_metadata(
    existing: SessionMetadata,
    incoming: SessionMetadata,
) -> SessionMetadata {
    SessionMetadata {
        workspace_slug: if incoming.workspace_slug.trim().is_empty() {
            existing.workspace_slug
        } else {
            incoming.workspace_slug
        },
        conversation_id: incoming.conversation_id.or(existing.conversation_id),
        agent_id: incoming.agent_id.or(existing.agent_id),
        ticket_id: incoming.ticket_id.or(existing.ticket_id),
        model: incoming.model.or(existing.model),
        trigger: incoming.trigger.or(existing.trigger),
        provisioning: match (existing.provisioning, incoming.provisioning) {
            (Some(existing), Some(incoming))
                if incoming
                    .hook_event_name
                    .eq_ignore_ascii_case("UserPromptSubmit")
                    && !existing
                        .hook_event_name
                        .eq_ignore_ascii_case("UserPromptSubmit") =>
                Some(incoming),
            (Some(existing), _) => Some(existing),
            (None, incoming) => incoming,
        },
        producer: incoming.producer.or(existing.producer),
        copilot_version: incoming.copilot_version.or(existing.copilot_version),
        vscode_version: incoming.vscode_version.or(existing.vscode_version),
        protocol_version: incoming
            .protocol_version
            .or(existing.protocol_version),
        worktree: incoming.worktree.or(existing.worktree),
    }
}

pub(super) fn validate_worktree_request(
    request: &SessionWorktreeCheckInRequest
) -> Result<(), SessionError> {
    validate_segment(&request.session_id, false)?;
    validate_session_id(&request.session_id)?;
    if request.owner_id.trim().is_empty() {
        return Err(SessionError::MissingOwnerId);
    }
    if request.ticket_id.trim().is_empty() {
        return Err(SessionError::MissingTicketId);
    }
    if request.worktree_path.as_os_str().is_empty() {
        return Err(SessionError::EmptyWorktreePath);
    }
    if request.branch.trim().is_empty() {
        return Err(SessionError::EmptyWorktreeBranch);
    }
    Ok(())
}

pub(super) fn canonicalize_worktree_path(
    path: &Path,
) -> Result<PathBuf, SessionError> {
    fs::canonicalize(path).map_err(|source| {
        SessionError::InvalidManagedWorktree {
            path: path.to_path_buf(),
            reason: source.to_string(),
        }
    })
}

pub(super) fn ensure_supported_schema_version(
    path: &Path,
    found: u32,
) -> Result<(), SessionError> {
    if found == SESSION_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(SessionError::SchemaVersionMismatch {
            path: path.to_path_buf(),
            found,
            expected: SESSION_SCHEMA_VERSION,
        })
    }
}

pub(super) fn can_reuse_assignment(
    existing: &SessionWorktreeAssignment,
    request: &SessionWorktreeCheckInRequest,
) -> bool {
    existing.status == SessionWorktreeStatus::Active
        && existing.path == request.worktree_path
        && existing.branch == request.branch
        && existing.path.exists()
}

pub(super) fn receipt_from_record(
    record: &SessionRecord
) -> Result<SessionWorktreeCheckInReceipt, SessionError> {
    let worktree = record.metadata.worktree.clone().ok_or_else(|| {
        SessionError::MissingWorktreeAssignment {
            session_id: record.session_id.clone(),
        }
    })?;

    Ok(SessionWorktreeCheckInReceipt {
        session_id: record.session_id.clone(),
        owner_id: record.metadata.agent_id.clone().unwrap_or_default(),
        ticket_id: record.metadata.ticket_id.clone().unwrap_or_default(),
        worktree_path: worktree.path,
        branch: worktree.branch,
        allocation_mode: worktree.allocation_mode,
        status: worktree.status,
        predecessor_session_id: worktree.predecessor_session_id,
        predecessor_path: worktree.predecessor_path,
    })
}

pub(super) fn merge_links(
    existing: SessionLinks,
    incoming: SessionLinks,
) -> SessionLinks {
    let mut merged = existing;
    extend_unique(&mut merged.ticket_ids, incoming.ticket_ids);
    extend_unique(&mut merged.spec_ids, incoming.spec_ids);
    extend_unique(&mut merged.doc_evidence_ids, incoming.doc_evidence_ids);
    extend_unique(&mut merged.log_ids, incoming.log_ids);
    merged
}

pub(super) fn merge_events(
    existing: Option<PersistedSessionEvents>,
    incoming: Option<PersistedSessionEvents>,
    session_id: String,
    captured_at: chrono::DateTime<chrono::Utc>,
) -> Result<Option<PersistedSessionEvents>, SessionError> {
    match (existing, incoming) {
        (None, None) => Ok(None),
        (Some(existing), None) => Ok(Some(existing)),
        (None, Some(mut incoming)) => {
            incoming.events = canonicalize_captured_events(incoming.events);
            Ok(Some(incoming))
        },
        (Some(mut existing), Some(incoming)) => {
            if existing.session_id != incoming.session_id {
                return Err(SessionError::TranscriptConflict {
                    session_id: incoming.session_id,
                    existing_turns: existing.events.len(),
                    incoming_turns: incoming.events.len(),
                });
            }

            existing.events = canonicalize_captured_events(existing.events);
            let incoming_events = canonicalize_captured_events(incoming.events);

            let mut known = std::collections::BTreeSet::new();
            for event in &existing.events {
                known.insert(captured_event_key(event));
            }
            for event in incoming_events {
                let key = captured_event_key(&event);
                if known.insert(key) {
                    existing.events.push(event);
                }
            }

            existing.session_id = session_id;
            if captured_at > existing.captured_at {
                existing.captured_at = captured_at;
            }

            Ok(Some(existing))
        },
    }
}

pub(super) fn merge_transcript(
    existing: Option<PersistedSessionTranscript>,
    incoming: PersistedSessionTranscript,
) -> Result<PersistedSessionTranscript, SessionError> {
    match existing {
        None => Ok(incoming),
        Some(mut existing) => {
            if existing.session_id != incoming.session_id {
                return Err(SessionError::TranscriptConflict {
                    session_id: incoming.session_id,
                    existing_turns: existing.turns.len(),
                    incoming_turns: incoming.turns.len(),
                });
            }

            let shared_prefix_len = existing
                .turns
                .iter()
                .zip(&incoming.turns)
                .take_while(|(left, right)| turns_match(left, right))
                .count();

            if shared_prefix_len < existing.turns.len()
                && shared_prefix_len < incoming.turns.len()
            {
                // Hook captures are periodic snapshots; when histories diverge,
                // keep the newest complete snapshot instead of rejecting sync.
                if incoming.turns.len() >= existing.turns.len() {
                    return Ok(incoming);
                }
                return Ok(existing);
            }

            if incoming.turns.len() > existing.turns.len() {
                existing.turns.extend(
                    incoming.turns.into_iter().skip(existing.turns.len()),
                );
            }

            if incoming.captured_at > existing.captured_at {
                existing.captured_at = incoming.captured_at;
            }

            Ok(existing)
        },
    }
}

pub(super) fn turns_match(
    left: &SessionTurn,
    right: &SessionTurn,
) -> bool {
    left.sequence == right.sequence
        && left.role == right.role
        && left.content == right.content
        && left.tool_name == right.tool_name
        && left.event_meta == right.event_meta
}

pub(super) fn session_matches_query(
    record: &SessionRecord,
    query: &SessionQuery,
) -> bool {
    if let Some(prefix) = &query.session_id_prefix {
        if !record.session_id.starts_with(prefix) {
            return false;
        }
    }

    if let Some(conversation_id) = &query.conversation_id {
        if record.metadata.conversation_id.as_deref()
            != Some(conversation_id.as_str())
        {
            return false;
        }
    }

    if let Some(agent_id) = &query.agent_id {
        if record.metadata.agent_id.as_deref() != Some(agent_id.as_str()) {
            return false;
        }
    }

    if let Some(text) = &query.text {
        let needle = text.to_ascii_lowercase();
        if !record
            .turns
            .iter()
            .any(|turn| turn.content.to_ascii_lowercase().contains(&needle))
        {
            return false;
        }
    }

    true
}

pub(super) fn validate_segment(
    value: &str,
    is_workspace_slug: bool,
) -> Result<(), SessionError> {
    let trimmed = value.trim();
    let invalid = ['/', '\\', ':'];
    // "." and ".." are only rejected for workspace slugs: they would otherwise
    // resolve to a path-traversal segment when joined onto a store base.
    let is_dot_segment =
        is_workspace_slug && (trimmed == "." || trimmed == "..");
    if trimmed.is_empty()
        || value.chars().any(|ch| invalid.contains(&ch))
        || is_dot_segment
    {
        return if is_workspace_slug {
            Err(SessionError::InvalidWorkspaceSlug(value.to_string()))
        } else {
            Err(SessionError::InvalidSessionId(value.to_string()))
        };
    }
    Ok(())
}
