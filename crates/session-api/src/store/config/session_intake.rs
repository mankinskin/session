#[allow(unused_imports)]
use super::*;

pub(super) use crate::session_intake::{
    SESSION_INTAKE_SCHEMA_VERSION, SessionIntake, SessionIntakeLinkState, SessionIntakeLinkage,
    SessionIntakeRecord,
};

pub(super) const INTAKE_INPUT_FILE: &str = "input.bin";
pub(super) const INTAKE_SUBMISSION_FILE: &str = "submission.json";
pub(super) const INTAKE_LINKAGE_FILE: &str = "linkage.json";

pub(super) fn validate_intake_request_id(value: &str) -> Result<(), SessionError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(SessionError::InvalidIntakeRequestId(value.to_string()))
    }
}

pub(super) fn intake_sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl SessionStoreConfig {
    pub(super) fn intake_require_session(&self, session_id: &str) -> Result<(), SessionError> {
        validate_session_id(session_id)?;
        let paths = self.paths_for_session_id(session_id)?;
        ensure_real_directories(&self.root, &paths.session_dir)?;
        match fs::symlink_metadata(&paths.manifest_path) {
            Ok(meta) if meta.file_type().is_file() => Ok(()),
            Ok(_) => Err(SessionError::InvalidStorePath(paths.manifest_path)),
            Err(source) if source.kind() == ErrorKind::NotFound => {
                Err(SessionError::IntakeSessionNotFound {
                    session_id: session_id.to_string(),
                })
            }
            Err(source) => Err(SessionError::Io {
                path: paths.manifest_path,
                source,
            }),
        }
    }

    pub(super) fn intake_dir(
        &self,
        session_id: &str,
        request_id: &str,
    ) -> Result<PathBuf, SessionError> {
        let dir = self
            .runtime_paths_for_workspace(session_id)?
            .workspace_dir
            .join("intake")
            .join(request_id);
        ensure_real_directories(&self.root, &dir)?;
        Ok(dir)
    }

    pub(super) fn intake_is_finished(&self, session_id: &str) -> Result<bool, SessionError> {
        let path = self.runtime_paths_for_workspace(session_id)?.finish_path;
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() => Ok(true),
            Ok(_) => Err(SessionError::InvalidStorePath(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(SessionError::Io { path, source }),
        }
    }

    pub(super) fn load_intake(
        &self,
        session_id: &str,
        request_id: &str,
        dir: &Path,
    ) -> Result<Option<SessionIntake>, SessionError> {
        let submission_path = dir.join(INTAKE_SUBMISSION_FILE);
        let Some(bytes) = read_regular_file_if_exists(&submission_path)? else {
            return Ok(None);
        };
        let corrupt = |path: &Path, reason: String| SessionError::IntakeCorrupt {
            path: path.to_path_buf(),
            reason,
        };
        let record: SessionIntakeRecord = serde_json::from_slice(&bytes)
            .map_err(|error| corrupt(&submission_path, error.to_string()))?;
        if record.schema_version != SESSION_INTAKE_SCHEMA_VERSION
            || record.session_id != session_id
            || record.request_id != request_id
            || !is_sha256_hex(&record.input_digest)
        {
            return Err(corrupt(
                &submission_path,
                "unsupported schema, identity mismatch or malformed digest".into(),
            ));
        }
        let input_path = dir.join(INTAKE_INPUT_FILE);
        let input = read_regular_file_if_exists(&input_path)?
            .ok_or_else(|| corrupt(&input_path, "submitted input bytes are missing".into()))?;
        if input.len() as u64 != record.byte_len || intake_sha256_hex(&input) != record.input_digest
        {
            return Err(corrupt(
                &input_path,
                "submitted input bytes do not match the recorded digest".into(),
            ));
        }
        let linkage_path = dir.join(INTAKE_LINKAGE_FILE);
        let linkage = match read_regular_file_if_exists(&linkage_path)? {
            None => None,
            Some(bytes) => {
                let linkage: SessionIntakeLinkage = serde_json::from_slice(&bytes)
                    .map_err(|error| corrupt(&linkage_path, error.to_string()))?;
                let linked = linkage.state == SessionIntakeLinkState::Linked;
                if linkage.schema_version != SESSION_INTAKE_SCHEMA_VERSION
                    || linkage.mission_id.is_nil()
                    || !linkage.mission_workspace_root.is_absolute()
                    || linked != linkage.linked_at.is_some()
                {
                    return Err(corrupt(
                        &linkage_path,
                        "inconsistent linkage journal".into(),
                    ));
                }
                Some(linkage)
            }
        };
        Ok(Some(SessionIntake {
            record,
            input_path,
            linkage,
            newly_submitted: false,
        }))
    }

    pub(super) fn intake_missing(session_id: &str, request_id: &str) -> SessionError {
        SessionError::IntakeMissing {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
        }
    }

    /// Append-only submission; identical replay remains readable after finish.
    pub fn submit_intake(
        &self,
        session_id: &str,
        request_id: &str,
        bytes: &[u8],
    ) -> Result<SessionIntake, SessionError> {
        validate_intake_request_id(request_id)?;
        self.intake_require_session(session_id)?;
        let _lock = self.acquire_runtime_lock(session_id)?;
        self.submit_locked(session_id, request_id, bytes)
    }

    pub(super) fn submit_locked(
        &self,
        session_id: &str,
        request_id: &str,
        bytes: &[u8],
    ) -> Result<SessionIntake, SessionError> {
        let digest = intake_sha256_hex(bytes);
        let dir = self.intake_dir(session_id, request_id)?;
        if let Some(existing) = self.load_intake(session_id, request_id, &dir)? {
            if existing.record.input_digest != digest {
                return Err(SessionError::IntakeIdempotencyConflict {
                    session_id: session_id.to_string(),
                    request_id: request_id.to_string(),
                    existing_digest: existing.record.input_digest,
                    submitted_digest: digest,
                });
            }
            return Ok(existing);
        }
        if self.intake_is_finished(session_id)? {
            return Err(SessionError::WorkspaceFinished {
                session_id: session_id.to_string(),
            });
        }
        let input_path = dir.join(INTAKE_INPUT_FILE);
        match read_regular_file_if_exists(&input_path)? {
            Some(existing) if existing != bytes => {
                return Err(SessionError::IntakeIdempotencyConflict {
                    session_id: session_id.to_string(),
                    request_id: request_id.to_string(),
                    existing_digest: intake_sha256_hex(&existing),
                    submitted_digest: digest,
                });
            }
            Some(_) => {}
            None => {
                fs::create_dir_all(&dir).map_err(|source| SessionError::Io {
                    path: dir.clone(),
                    source,
                })?;
                ensure_real_directories(&self.root, &dir)?;
                if !write_new_synced(&input_path, bytes)? {
                    return Err(SessionError::IntakeCorrupt {
                        path: input_path,
                        reason: "input appeared while holding the session lock".into(),
                    });
                }
            }
        }
        let record = SessionIntakeRecord {
            schema_version: SESSION_INTAKE_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            input_digest: digest,
            byte_len: bytes.len() as u64,
            submitted_at: Utc::now(),
        };
        write_json(&dir.join(INTAKE_SUBMISSION_FILE), &record)?;
        Ok(SessionIntake {
            record,
            input_path,
            linkage: None,
            newly_submitted: true,
        })
    }

    /// Read and re-hash an immutable submission.
    pub fn read_intake(
        &self,
        session_id: &str,
        request_id: &str,
    ) -> Result<SessionIntake, SessionError> {
        validate_intake_request_id(request_id)?;
        self.intake_require_session(session_id)?;
        let dir = self.intake_dir(session_id, request_id)?;
        self.load_intake(session_id, request_id, &dir)?
            .ok_or_else(|| Self::intake_missing(session_id, request_id))
    }

    /// Reserve identity/root before Mission creation, without permitting rebinding.
    pub fn reserve_intake_mission(
        &self,
        session_id: &str,
        request_id: &str,
        mission_id: Uuid,
        workspace_root: &Path,
    ) -> Result<SessionIntake, SessionError> {
        validate_intake_request_id(request_id)?;
        self.intake_require_session(session_id)?;
        let _lock = self.acquire_runtime_lock(session_id)?;
        self.reserve_locked(session_id, request_id, mission_id, workspace_root)
    }

    pub(super) fn reserve_locked(
        &self,
        session_id: &str,
        request_id: &str,
        mission_id: Uuid,
        workspace_root: &Path,
    ) -> Result<SessionIntake, SessionError> {
        if !workspace_root.is_absolute() {
            return Err(SessionError::InvalidStorePath(workspace_root.to_path_buf()));
        }
        let conflict = |reason: String| SessionError::IntakeLinkageConflict {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            reason,
        };
        if mission_id.is_nil() {
            return Err(conflict("Mission id must not be nil".into()));
        }
        let dir = self.intake_dir(session_id, request_id)?;
        let mut intake = self
            .load_intake(session_id, request_id, &dir)?
            .ok_or_else(|| Self::intake_missing(session_id, request_id))?;
        if let Some(existing) = &intake.linkage {
            if existing.mission_id == mission_id
                && existing.mission_workspace_root == workspace_root
            {
                return Ok(intake);
            }
            return Err(conflict(format!(
                "already reserved for Mission {} under {}",
                existing.mission_id,
                existing.mission_workspace_root.display()
            )));
        }
        if self.intake_is_finished(session_id)? {
            return Err(SessionError::WorkspaceFinished {
                session_id: session_id.to_string(),
            });
        }
        let linkage = SessionIntakeLinkage {
            schema_version: SESSION_INTAKE_SCHEMA_VERSION,
            mission_id,
            mission_workspace_root: workspace_root.to_path_buf(),
            state: SessionIntakeLinkState::Reserved,
            reserved_at: Utc::now(),
            linked_at: None,
        };
        write_json(&dir.join(INTAKE_LINKAGE_FILE), &linkage)?;
        intake.linkage = Some(linkage);
        Ok(intake)
    }

    /// Commit only the matching reservation after Mission provenance is complete.
    pub fn complete_intake_link(
        &self,
        session_id: &str,
        request_id: &str,
        mission_id: Uuid,
    ) -> Result<SessionIntake, SessionError> {
        validate_intake_request_id(request_id)?;
        self.intake_require_session(session_id)?;
        let _lock = self.acquire_runtime_lock(session_id)?;
        self.complete_locked(session_id, request_id, mission_id)
    }

    pub(super) fn complete_locked(
        &self,
        session_id: &str,
        request_id: &str,
        mission_id: Uuid,
    ) -> Result<SessionIntake, SessionError> {
        let conflict = |reason: String| SessionError::IntakeLinkageConflict {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            reason,
        };
        let dir = self.intake_dir(session_id, request_id)?;
        let mut intake = self
            .load_intake(session_id, request_id, &dir)?
            .ok_or_else(|| Self::intake_missing(session_id, request_id))?;
        let Some(existing) = intake.linkage.clone() else {
            return Err(conflict("no Mission reservation was journaled".into()));
        };
        if existing.mission_id != mission_id {
            return Err(conflict(format!(
                "reserved Mission is {}, not {mission_id}",
                existing.mission_id
            )));
        }
        if existing.state == SessionIntakeLinkState::Linked {
            return Ok(intake);
        }
        if self.intake_is_finished(session_id)? {
            return Err(SessionError::WorkspaceFinished {
                session_id: session_id.to_string(),
            });
        }
        let linked = SessionIntakeLinkage {
            state: SessionIntakeLinkState::Linked,
            linked_at: Some(Utc::now()),
            ..existing
        };
        write_json(&dir.join(INTAKE_LINKAGE_FILE), &linked)?;
        intake.linkage = Some(linked);
        Ok(intake)
    }

    /// Hold the Session mutation lock across all cross-store intake steps.
    pub fn begin_intake_operation(
        &self,
        session_id: &str,
        request_id: &str,
        bytes: &[u8],
    ) -> Result<SessionIntakeOperation<'_>, SessionError> {
        validate_intake_request_id(request_id)?;
        self.intake_require_session(session_id)?;
        let lock = self.acquire_runtime_lock(session_id)?;
        if let Some(existing) = self.load_intake(
            session_id,
            request_id,
            &self.intake_dir(session_id, request_id)?,
        )? {
            let linked = existing
                .linkage
                .as_ref()
                .is_some_and(|l| l.state == SessionIntakeLinkState::Linked);
            if !linked && self.intake_is_finished(session_id)? {
                return Err(SessionError::WorkspaceFinished {
                    session_id: session_id.to_string(),
                });
            }
        }
        let intake = self.submit_locked(session_id, request_id, bytes)?;
        Ok(SessionIntakeOperation {
            store: self,
            intake,
            _lock: lock,
        })
    }
}

/// Lock-holding operation preventing finish from interleaving Mission creation.
pub struct SessionIntakeOperation<'a> {
    pub(super) store: &'a SessionStoreConfig,
    pub(super) intake: SessionIntake,
    pub(super) _lock: RuntimeMutationLock,
}

impl SessionIntakeOperation<'_> {
    pub fn intake(&self) -> &SessionIntake {
        &self.intake
    }

    pub fn reserve_mission(
        &mut self,
        mission_id: Uuid,
        workspace_root: &Path,
    ) -> Result<(), SessionError> {
        let (sid, rid) = (
            self.intake.record.session_id.clone(),
            self.intake.record.request_id.clone(),
        );
        self.intake = self
            .store
            .reserve_locked(&sid, &rid, mission_id, workspace_root)?;
        Ok(())
    }

    pub fn complete_link(&mut self, mission_id: Uuid) -> Result<(), SessionError> {
        let (sid, rid) = (
            self.intake.record.session_id.clone(),
            self.intake.record.request_id.clone(),
        );
        self.intake = self.store.complete_locked(&sid, &rid, mission_id)?;
        Ok(())
    }
}

#[path = "session_intake/filesystem.rs"]
mod filesystem;
#[allow(unused_imports)]
pub use filesystem::*;
