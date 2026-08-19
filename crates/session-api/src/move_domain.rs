//! Session-domain adapter onto the domain-neutral move kernel.
//!
//! Sessions are persisted as one folder per session id under
//! `sessions/<session_id>`. The shared kernel is UUID-based,
//! so this adapter supports sessions whose ids are UUID strings and leaves
//! non-UUID legacy/session-provider ids to their existing read/query paths.

use std::path::{
    Path,
    PathBuf,
};

use memory_kernel::storage::move_kernel::{
    self,
    MoveDomain,
    MoveError,
    MoveOutcome,
    MovePlan,
    MoveReferences,
    MoveResult,
};
use uuid::Uuid;

use crate::{
    SessionError,
    SessionStoreConfig,
};

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

    fn store_at(
        &self,
        root: &Path,
    ) -> SessionStoreConfig {
        SessionStoreConfig::new(
            root.to_path_buf(),
            self.store.workspace_slug.clone(),
        )
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

    fn source_entity_path(
        &self,
        entity_id: &Uuid,
    ) -> MoveResult<Option<PathBuf>> {
        let session_id = entity_id.to_string();
        let paths = self
            .store
            .paths_for_session_id(&session_id)
            .map_err(to_move_error)?;
        Ok(paths.session_dir.exists().then_some(paths.session_dir))
    }

    fn related_entities(
        &self,
        _entity_id: &Uuid,
    ) -> MoveResult<MoveReferences> {
        Ok(MoveReferences::default())
    }

    fn target_store_present(
        &self,
        target_store_root: &Path,
    ) -> MoveResult<bool> {
        Ok(target_store_root.is_dir())
    }

    fn entity_indexed_in(
        &self,
        store_root: &Path,
        entity_id: &Uuid,
    ) -> MoveResult<bool> {
        let store = self.store_at(store_root);
        match store.read_session(&entity_id.to_string()) {
            Ok(_) => Ok(true),
            Err(SessionError::NotFound { .. }) => Ok(false),
            Err(error) => Err(to_move_error(error)),
        }
    }

    fn scan_store(
        &self,
        _store_root: &Path,
    ) -> MoveResult<()> {
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
        move_kernel::plan_move(&domain, session_id, target_workspace_root)
            .map_err(from_move_error)
    }

    /// Execute a supported session move with a fresh journal.
    pub fn execute_move_with_journal(
        &self,
        plan: &MovePlan,
    ) -> Result<MoveOutcome, SessionError> {
        let domain = SessionMoveDomain::new(self);
        move_kernel::execute_move(&domain, plan).map_err(from_move_error)
    }

    /// Resume an interrupted session move from its journal id.
    pub fn resume_move_with_journal(
        &self,
        journal_id: Uuid,
    ) -> Result<MoveOutcome, SessionError> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_kernel::storage::move_kernel::MoveExecutionPhase;
    use std::process::Command;
    use tempfile::tempdir;

    use crate::{
        CopilotHookMessage,
        CopilotHookPayload,
        SessionCaptureRequest,
        SessionRole,
    };

    fn run_git(
        repo_root: &Path,
        args: &[&str],
    ) {
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
            workspace_slug: "context-engine".to_string(),
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

    #[test]
    fn session_store_reuses_move_kernel_between_stores() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init"]);

        let source_workspace = repo.join("source");
        let target_workspace = repo.join("target");
        std::fs::create_dir_all(source_workspace.join(SESSION_INDEX_DIR))
            .unwrap();
        std::fs::create_dir_all(target_workspace.join(SESSION_INDEX_DIR))
            .unwrap();

        let session_id = Uuid::new_v4();
        let source_store = SessionStoreConfig::new(
            source_workspace.join(SESSION_INDEX_DIR),
            "context-engine",
        );
        source_store
            .persist_capture(sample_request(&session_id))
            .unwrap();

        let plan = source_store
            .plan_move_preflight(&session_id, &target_workspace)
            .unwrap();
        assert!(plan.supported(), "unexpected blockers: {:?}", plan.blockers);

        let outcome = source_store.execute_move_with_journal(&plan).unwrap();
        assert_eq!(outcome.journal.phase, MoveExecutionPhase::Validated);

        let target_store = SessionStoreConfig::new(
            target_workspace.join(SESSION_INDEX_DIR),
            "context-engine",
        );
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
}
