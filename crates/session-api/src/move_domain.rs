//! Session-domain adapter onto the domain-neutral move kernel.
//!
//! Sessions are persisted as one folder per session id under
//! `sessions/<session_id>`. The shared kernel is UUID-based,
//! so this adapter supports sessions whose ids are UUID strings and leaves
//! non-UUID legacy/session-provider ids to their existing read/query paths.

use std::path::{Path, PathBuf};

use memory_kernel::storage::move_kernel::{
    self, MoveDomain, MoveError, MoveOutcome, MovePlan, MoveReferences, MoveResult,
    MoveSetExecutionPhase, MoveSetOutcome, MoveSetPlan, load_move_set_journal,
};
use uuid::Uuid;

use crate::{SessionError, SessionStoreConfig};

const SESSION_INDEX_DIR: &str = ".session";

fn to_move_error(error: SessionError) -> MoveError {
    match error {
        SessionError::Io { source, .. } => MoveError::Io(source),
        other => MoveError::Domain(other.to_string()),
    }
}

fn from_move_error(error: MoveError) -> SessionError {
    match error {
        MoveError::Io(io) => SessionError::Move(io.to_string()),
        MoveError::Domain(message) => SessionError::Move(message),
        MoveError::InteroperabilityContract {
            artifact_class,
            detail,
        } => SessionError::Move(format!(
            "interoperability contract violation for {artifact_class}: {detail}"
        )),
    }
}

/// Session-domain implementation of the move kernel's [`MoveDomain`] trait.
pub struct SessionMoveDomain<'a> {
    store: &'a SessionStoreConfig,
    entity_subdir: String,
}

impl<'a> SessionMoveDomain<'a> {
    pub fn new(store: &'a SessionStoreConfig) -> Self {
        Self {
            store,
            entity_subdir: "sessions".to_string(),
        }
    }

    fn store_at(&self, root: &Path) -> SessionStoreConfig {
        SessionStoreConfig::new(root.to_path_buf())
    }
}

impl MoveDomain for SessionMoveDomain<'_> {
    fn entity_subdir(&self) -> &str {
        &self.entity_subdir
    }

    fn store_index_dir(&self) -> &str {
        SESSION_INDEX_DIR
    }

    fn source_store_root(&self) -> PathBuf {
        self.store.root.clone()
    }

    fn source_entity_path(&self, entity_id: &Uuid) -> MoveResult<Option<PathBuf>> {
        let session_id = entity_id.to_string();
        let paths = self
            .store
            .paths_for_session_id(&session_id)
            .map_err(to_move_error)?;
        Ok(paths.session_dir.exists().then_some(paths.session_dir))
    }

    fn source_entity_paths_for_set(
        &self,
        entity_ids: &[Uuid],
    ) -> MoveResult<std::collections::BTreeMap<Uuid, PathBuf>> {
        let sessions_root = self.store.root.join("sessions");
        Ok(entity_ids
            .iter()
            .filter_map(|entity_id| {
                let path = sessions_root.join(entity_id.to_string());
                path.is_dir().then_some((*entity_id, path))
            })
            .collect())
    }

    fn related_entities(&self, _entity_id: &Uuid) -> MoveResult<MoveReferences> {
        Ok(MoveReferences::default())
    }

    fn target_store_present(&self, target_store_root: &Path) -> MoveResult<bool> {
        Ok(target_store_root.is_dir())
    }

    fn entity_indexed_in(&self, store_root: &Path, entity_id: &Uuid) -> MoveResult<bool> {
        let store = self.store_at(store_root);
        match store.read_session(&entity_id.to_string()) {
            Ok(_) => Ok(true),
            Err(SessionError::NotFound { .. }) => Ok(false),
            Err(error) => Err(to_move_error(error)),
        }
    }

    fn scan_store(&self, _store_root: &Path) -> MoveResult<()> {
        Ok(())
    }
}

impl SessionStoreConfig {
    /// Build a read-only preflight plan for moving a UUID-addressed session to
    /// `target_workspace_root`, reusing the domain-neutral move kernel.
    pub fn plan_move_preflight(
        &self,
        session_id: &Uuid,
        target_workspace_root: &Path,
    ) -> Result<MovePlan, SessionError> {
        let domain = SessionMoveDomain::new(self);
        move_kernel::plan_move(&domain, session_id, target_workspace_root).map_err(from_move_error)
    }

    /// Execute a supported session move with a fresh journal.
    pub fn execute_move_with_journal(&self, plan: &MovePlan) -> Result<MoveOutcome, SessionError> {
        let domain = SessionMoveDomain::new(self);
        move_kernel::execute_move(&domain, plan).map_err(from_move_error)
    }

    /// Resume an interrupted session move from its journal id.
    pub fn resume_move_with_journal(&self, journal_id: Uuid) -> Result<MoveOutcome, SessionError> {
        let domain = SessionMoveDomain::new(self);
        move_kernel::resume_move(&domain, journal_id).map_err(from_move_error)
    }

    /// Roll back a session move from its journal id.
    pub fn rollback_move_with_journal(
        &self,
        journal_id: Uuid,
    ) -> Result<MoveOutcome, SessionError> {
        let domain = SessionMoveDomain::new(self);
        move_kernel::rollback_move(&domain, journal_id).map_err(from_move_error)
    }

    /// Build one normalized read-only preflight plan for a set of UUID
    /// session ids, reusing the domain-neutral kernel's set-level batching
    /// (shared store root/git topology resolution instead of per-entity
    /// recomputation). Rejects an empty selection; deterministically
    /// dedupes/sorts the rest.
    pub fn plan_move_set(
        &self,
        session_ids: &[Uuid],
        target_workspace_root: &Path,
    ) -> Result<MoveSetPlan, SessionError> {
        let domain = SessionMoveDomain::new(self);
        move_kernel::plan_move_set(&domain, session_ids, target_workspace_root)
            .map_err(from_move_error)
    }

    /// Execute a supported normalized set move with one shared lock
    /// lifecycle covering every session in the set.
    pub fn execute_move_set(&self, plan: &MoveSetPlan) -> Result<MoveSetOutcome, SessionError> {
        let domain = SessionMoveDomain::new(self);
        move_kernel::execute_move_set(&domain, plan).map_err(from_move_error)
    }

    /// Resume an interrupted set move from its journal id. A journal that
    /// already reached `Validated`/`RolledBack` short-circuits to its
    /// recorded outcome instead of re-entering the kernel's execution loop
    /// with an empty (already-cleared) entity-plan list.
    pub fn resume_move_set(&self, journal_id: Uuid) -> Result<MoveSetOutcome, SessionError> {
        let existing = load_move_set_journal(&self.root, journal_id).map_err(from_move_error)?;
        if matches!(
            existing.phase,
            MoveSetExecutionPhase::Validated | MoveSetExecutionPhase::RolledBack
        ) {
            let session_ids = existing.entity_ids.clone();
            return Ok(MoveSetOutcome {
                journal: existing,
                entity_ids: session_ids,
                entity_outcomes: Vec::new(),
            });
        }
        let domain = SessionMoveDomain::new(self);
        move_kernel::resume_move_set(&domain, journal_id).map_err(from_move_error)
    }

    /// Roll back a completed or partially completed set move, identified by
    /// the set journal id.
    pub fn rollback_move_set(&self, journal_id: Uuid) -> Result<MoveSetOutcome, SessionError> {
        let domain = SessionMoveDomain::new(self);
        move_kernel::rollback_move_set(&domain, journal_id).map_err(from_move_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_kernel::storage::move_kernel::MoveExecutionPhase;
    use std::process::Command;
    use tempfile::tempdir;

    use crate::{CopilotHookMessage, CopilotHookPayload, SessionCaptureRequest, SessionRole};

    fn run_git(repo_root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(repo_root)
            .args(args)
            .status()
            .expect("git command");
        assert!(status.success(), "git {args:?} failed: {status}");
    }

    fn sample_request(session_id: &Uuid) -> SessionCaptureRequest {
        SessionCaptureRequest::copilot(CopilotHookPayload {
            session_id: session_id.to_string(),
            workspace_path: "context-engine".to_string(),
            captured_at: chrono::Utc::now(),
            conversation_id: Some("conversation-1".to_string()),
            agent_id: Some("github-copilot".to_string()),
            model: Some("GPT".to_string()),
            trigger: Some("test".to_string()),
            provisioning: None,
            messages: vec![CopilotHookMessage {
                role: SessionRole::User,
                content: "move this session".to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: None,
            }],
            events: vec![],
            runtime: None,
        })
    }

    fn canonical_target_session_store(workspace_root: &Path) -> PathBuf {
        workspace_root
            .join(memory_kernel::workspace::CANONICAL_STORES_DIR)
            .join("session")
    }

    #[test]
    fn session_store_reuses_move_kernel_between_stores() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init"]);

        let source_workspace = repo.join("source");
        let target_workspace = repo.join("target");
        std::fs::create_dir_all(source_workspace.join(SESSION_INDEX_DIR)).unwrap();
        std::fs::create_dir_all(canonical_target_session_store(&target_workspace)).unwrap();

        let session_id = Uuid::new_v4();
        let source_store = SessionStoreConfig::new(source_workspace.join(SESSION_INDEX_DIR));
        source_store
            .persist_capture(sample_request(&session_id))
            .unwrap();

        let plan = source_store
            .plan_move_preflight(&session_id, &target_workspace)
            .unwrap();
        assert!(plan.supported(), "unexpected blockers: {:?}", plan.blockers);

        let outcome = source_store.execute_move_with_journal(&plan).unwrap();
        assert_eq!(outcome.journal.phase, MoveExecutionPhase::Validated);

        let target_store =
            SessionStoreConfig::new(canonical_target_session_store(&target_workspace));
        assert!(matches!(
            source_store.read_session(&session_id.to_string()),
            Err(SessionError::NotFound { .. })
        ));
        assert_eq!(
            target_store
                .read_session(&session_id.to_string())
                .unwrap()
                .session_id,
            session_id.to_string()
        );
    }

    #[test]
    fn plan_move_set_rejects_empty_selection() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init"]);

        let source_workspace = repo.join("source");
        let target_workspace = repo.join("target");
        std::fs::create_dir_all(source_workspace.join(SESSION_INDEX_DIR)).unwrap();
        std::fs::create_dir_all(target_workspace.join(SESSION_INDEX_DIR)).unwrap();

        let source_store = SessionStoreConfig::new(source_workspace.join(SESSION_INDEX_DIR));

        let error = source_store
            .plan_move_set(&[], &target_workspace)
            .unwrap_err();
        assert!(error.to_string().contains("empty"));
    }

    #[test]
    fn plan_move_set_normalizes_and_dedupes_selection() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init"]);

        let source_workspace = repo.join("source");
        let target_workspace = repo.join("target");
        std::fs::create_dir_all(source_workspace.join(SESSION_INDEX_DIR)).unwrap();
        std::fs::create_dir_all(target_workspace.join(SESSION_INDEX_DIR)).unwrap();

        let source_store = SessionStoreConfig::new(source_workspace.join(SESSION_INDEX_DIR));

        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        source_store
            .persist_capture(sample_request(&first_id))
            .unwrap();
        source_store
            .persist_capture(sample_request(&second_id))
            .unwrap();

        // Deliberately unsorted, with a duplicate entry.
        let selection = [second_id, first_id, second_id];
        let plan = source_store
            .plan_move_set(&selection, &target_workspace)
            .unwrap();

        let mut expected = vec![first_id, second_id];
        expected.sort();
        assert_eq!(plan.entity_ids, expected);
        assert_eq!(plan.entity_plans.len(), 2);
    }

    #[test]
    fn execute_move_set_preserves_uuid_identity_and_transcript_files() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init"]);

        let source_workspace = repo.join("source");
        let target_workspace = repo.join("target");
        std::fs::create_dir_all(source_workspace.join(SESSION_INDEX_DIR)).unwrap();
        std::fs::create_dir_all(canonical_target_session_store(&target_workspace)).unwrap();

        let source_store = SessionStoreConfig::new(source_workspace.join(SESSION_INDEX_DIR));

        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        source_store
            .persist_capture(sample_request(&first_id))
            .unwrap();
        source_store
            .persist_capture(sample_request(&second_id))
            .unwrap();

        let plan = source_store
            .plan_move_set(&[first_id, second_id], &target_workspace)
            .unwrap();
        assert!(
            plan.supported(),
            "unexpected blockers: {:?}",
            plan.entity_plans
        );

        let outcome = source_store.execute_move_set(&plan).unwrap();
        assert_eq!(outcome.entity_ids, plan.entity_ids);
        assert_eq!(outcome.entity_outcomes.len(), 2);
        for entity_outcome in &outcome.entity_outcomes {
            assert_eq!(entity_outcome.journal.phase, MoveExecutionPhase::Validated);
        }

        let target_store =
            SessionStoreConfig::new(canonical_target_session_store(&target_workspace));
        for session_id in [first_id, second_id] {
            assert!(matches!(
                source_store.read_session(&session_id.to_string()),
                Err(SessionError::NotFound { .. })
            ));

            let moved = target_store.read_session(&session_id.to_string()).unwrap();
            assert_eq!(moved.session_id, session_id.to_string());

            let paths = target_store
                .paths_for_session_id(&session_id.to_string())
                .unwrap();
            assert!(paths.manifest_path.is_file());
            assert!(paths.transcript_path.is_file());
        }
    }

    #[test]
    fn resume_move_set_short_circuits_after_validated() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init"]);

        let source_workspace = repo.join("source");
        let target_workspace = repo.join("target");
        std::fs::create_dir_all(source_workspace.join(SESSION_INDEX_DIR)).unwrap();
        std::fs::create_dir_all(canonical_target_session_store(&target_workspace)).unwrap();

        let source_store = SessionStoreConfig::new(source_workspace.join(SESSION_INDEX_DIR));

        let session_id = Uuid::new_v4();
        source_store
            .persist_capture(sample_request(&session_id))
            .unwrap();

        let plan = source_store
            .plan_move_set(&[session_id], &target_workspace)
            .unwrap();
        let outcome = source_store.execute_move_set(&plan).unwrap();
        let journal_id = outcome.journal.id;

        let resumed = source_store.resume_move_set(journal_id).unwrap();
        assert_eq!(resumed.journal.id, journal_id);
        assert_eq!(resumed.journal.phase, MoveSetExecutionPhase::Validated);
        assert!(resumed.entity_outcomes.is_empty());
    }

    #[test]
    fn rollback_move_set_restores_sessions_by_journal_id() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init"]);

        let source_workspace = repo.join("source");
        let target_workspace = repo.join("target");
        std::fs::create_dir_all(source_workspace.join(SESSION_INDEX_DIR)).unwrap();
        std::fs::create_dir_all(canonical_target_session_store(&target_workspace)).unwrap();

        let source_store = SessionStoreConfig::new(source_workspace.join(SESSION_INDEX_DIR));

        let session_id = Uuid::new_v4();
        source_store
            .persist_capture(sample_request(&session_id))
            .unwrap();

        let plan = source_store
            .plan_move_set(&[session_id], &target_workspace)
            .unwrap();
        let outcome = source_store.execute_move_set(&plan).unwrap();
        let journal_id = outcome.journal.id;

        let rolled_back = source_store.rollback_move_set(journal_id).unwrap();
        assert_eq!(rolled_back.journal.id, journal_id);

        let target_store =
            SessionStoreConfig::new(canonical_target_session_store(&target_workspace));
        assert!(source_store.read_session(&session_id.to_string()).is_ok());
        assert!(matches!(
            target_store.read_session(&session_id.to_string()),
            Err(SessionError::NotFound { .. })
        ));
    }
}
