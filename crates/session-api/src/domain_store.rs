use std::path::Path;

use memory_kernel::{
    domain_store::{
        CreateEntity, DomainStore, DomainStoreResolution, ListEntities, ReadEntity, StoreAccessMode,
    },
    model::domain::DomainId,
    workspace::StoreRootDiagnostic,
};

use crate::{
    CopilotHookEvent, SessionCaptureRequest, SessionError, SessionQuery, SessionQueryResult,
    SessionRecord, SessionStoreConfig, SessionStorePlan,
};

/// The supported Session-domain persistence inputs.
///
/// Session records and hook-only events deliberately retain their existing
/// lifecycle representations rather than being forced into a shared schema.
#[derive(Debug, Clone)]
pub enum SessionCreateInput {
    Capture(SessionCaptureRequest),
    HookEvent {
        session_id: String,
        event: CopilotHookEvent,
    },
}

/// Result of a supported Session-domain write.
#[derive(Debug, Clone)]
pub enum SessionCreateResult {
    Capture(SessionStorePlan),
    HookEvent,
}

/// Typed capabilities over existing Session persistence.
#[derive(Debug, Clone)]
pub struct SessionDomainStore {
    config: SessionStoreConfig,
}

impl SessionDomainStore {
    pub fn new(config: SessionStoreConfig) -> Self {
        Self { config }
    }
}

impl DomainStore for SessionStoreConfig {
    fn domain_id() -> DomainId {
        DomainId::new("session").expect("session is a valid domain id")
    }

    fn store_dir_name() -> &'static str {
        ".session"
    }
}

impl SessionStoreConfig {
    /// Resolve a selected workspace through the shared DomainStore contract.
    ///
    /// Read-only access retains a legacy `.session` store when it is the only
    /// layout. Writes always initialize and select `.workflow-tools/session`.
    pub fn resolve_workspace_store(
        local_workspace: &Path,
        access_mode: StoreAccessMode,
    ) -> Result<DomainStoreResolution, SessionError> {
        if access_mode == StoreAccessMode::CreateOrOpen {
            memory_kernel::workspace::validate_explicit_store_root_for_write(
                local_workspace,
                ".session",
            )?;
        }
        let mut resolution = <Self as DomainStore>::resolve_store(local_workspace, access_mode)?;
        let legacy_root = resolution.local_workspace.join(".session");
        let canonical_root =
            memory_kernel::workspace::canonical_store_root(&resolution.local_workspace, ".session");
        let legacy_exists = legacy_root.is_dir();
        let canonical_exists = canonical_root.is_dir();

        if access_mode == StoreAccessMode::ReadOnly && legacy_exists && !canonical_exists {
            resolution.store_root = legacy_root.clone();
        }
        if legacy_exists {
            let diagnostic = if canonical_exists {
                StoreRootDiagnostic::BothLayoutsPresent {
                    domain: "session".to_string(),
                    legacy_path: legacy_root,
                    canonical_path: canonical_root,
                }
            } else {
                StoreRootDiagnostic::LegacyStore {
                    domain: "session".to_string(),
                    legacy_path: legacy_root,
                    canonical_path: canonical_root,
                }
            };
            if !resolution.diagnostics.contains(&diagnostic) {
                resolution.diagnostics.push(diagnostic);
            }
        }
        Ok(resolution)
    }
}

impl CreateEntity for SessionDomainStore {
    type Entity = SessionRecord;
    type CreateInput = SessionCreateInput;
    type CreateResult = SessionCreateResult;
    type Error = SessionError;

    fn create_entity(&self, input: Self::CreateInput) -> Result<Self::CreateResult, Self::Error> {
        match input {
            SessionCreateInput::Capture(request) => self
                .config
                .persist_capture(request)
                .map(SessionCreateResult::Capture),
            SessionCreateInput::HookEvent { session_id, event } => {
                self.config.persist_hook_event(&session_id, event)?;
                Ok(SessionCreateResult::HookEvent)
            }
        }
    }
}

impl ReadEntity for SessionDomainStore {
    type EntityId = String;
    type Entity = SessionRecord;
    type ReadResult = SessionRecord;
    type Error = SessionError;

    fn read_entity(&self, id: Self::EntityId) -> Result<Self::ReadResult, Self::Error> {
        self.config.read_session(&id)
    }
}

impl ListEntities for SessionDomainStore {
    type Query = SessionQuery;
    type Entity = SessionRecord;
    type ListResult = SessionQueryResult;
    type Error = SessionError;

    fn list_entities(&self, query: Self::Query) -> Result<Self::ListResult, Self::Error> {
        self.config.query_sessions(&query)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use memory_kernel::{
        domain_store::{CreateEntity, ReadEntity, StoreAccessMode},
        workspace::canonical_store_root,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::{CopilotHookMessage, CopilotHookPayload, SessionRole};

    #[test]
    fn selected_workspace_capture_and_hook_event_read_back_from_its_canonical_store() {
        let parent = tempdir().unwrap();
        let selected = parent.path().join("selected");
        let sibling = parent.path().join("sibling");
        std::fs::create_dir_all(&selected).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();

        let resolution =
            SessionStoreConfig::resolve_workspace_store(&selected, StoreAccessMode::CreateOrOpen)
                .unwrap();
        let store = SessionDomainStore::new(SessionStoreConfig::new(resolution.store_root));
        let session_id = "11111111-1111-4111-8111-111111111111";
        let captured_at = chrono::Utc
            .with_ymd_and_hms(2026, 10, 7, 12, 0, 0)
            .single()
            .unwrap();
        let payload = CopilotHookPayload {
            session_id: session_id.to_string(),
            workspace_path: selected.to_string_lossy().into_owned(),
            captured_at,
            conversation_id: None,
            agent_id: None,
            model: None,
            trigger: Some("SessionStart".to_string()),
            provisioning: None,
            messages: vec![CopilotHookMessage {
                role: SessionRole::User,
                content: "Capture in the selected workspace.".to_string(),
                tool_name: None,
                captured_at: Some(captured_at),
                event_meta: None,
            }],
            events: Vec::new(),
            runtime: None,
        };

        store
            .create_entity(SessionCreateInput::Capture(SessionCaptureRequest::copilot(
                payload,
            )))
            .unwrap();
        store
            .create_entity(SessionCreateInput::HookEvent {
                session_id: session_id.to_string(),
                event: CopilotHookEvent {
                    event_id: None,
                    parent_event_id: None,
                    event_type: Some("UserPromptSubmit".to_string()),
                    captured_at: Some(captured_at),
                    turn_id: None,
                    message_id: None,
                    tool_call_id: None,
                    tool_name: None,
                    tool_success: None,
                    reasoning_text: None,
                    tool_requests_json: None,
                    tool_arguments_json: None,
                    data_json: Some(serde_json::json!({"prompt": "selected only"})),
                    raw_event_json: None,
                },
            })
            .unwrap();

        let record = store.read_entity(session_id.to_string()).unwrap();
        assert_eq!(record.session_id, session_id);
        assert_eq!(record.metadata.workspace_path, selected.to_string_lossy());
        let selected_store = canonical_store_root(&selected, ".session");
        assert!(selected_store.join("sessions").join(session_id).join("session.json").is_file());
        assert!(selected_store.join("sessions").join(session_id).join("events.json").is_file());
        assert!(!canonical_store_root(parent.path(), ".session").exists());
        assert!(!canonical_store_root(&sibling, ".session").exists());
    }
}
