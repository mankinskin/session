#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn workflow_finish_enforces_gates_and_is_idempotent() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"));
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("required-node".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "must finish".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();
    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("optional-node".to_string()),
                kind: SessionWorkflowNodeKind::Task,
                requirement: SessionWorkflowNodeRequirement::Optional,
                title: "may defer".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: None,
            },
        )
        .unwrap();

    let blocked = config.finish_workflow(
        &workspace_id,
        vec![crate::SessionValidationGate {
            validation_spec_id: "val-session-workflow-finish".to_string(),
            required: true,
            outcome: Some("passed".to_string()),
            command: None,
        }],
        vec![],
        None,
    );
    assert!(matches!(
        blocked,
        Err(crate::SessionError::FinishBlocked { .. })
    ));

    config
        .workflow_update_node_status(
            &workspace_id,
            "required-node",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();
    config
        .workflow_update_node_status(
            &workspace_id,
            "optional-node",
            SessionWorkflowNodeStatus::Deferred,
            Some("not needed for this handoff".to_string()),
        )
        .unwrap();

    let blocked_validation = config.finish_workflow(
        &workspace_id,
        vec![crate::SessionValidationGate {
            validation_spec_id: "val-session-workflow-finish".to_string(),
            required: true,
            outcome: Some("failed".to_string()),
            command: None,
        }],
        vec!["optional-node".to_string()],
        None,
    );
    assert!(matches!(
        blocked_validation,
        Err(crate::SessionError::FinishBlocked { .. })
    ));

    let finished = config
        .finish_workflow(
            &workspace_id,
            vec![crate::SessionValidationGate {
                validation_spec_id: "val-session-workflow-finish".to_string(),
                required: true,
                outcome: Some("passed".to_string()),
                command: None,
            }],
            vec!["optional-node".to_string()],
            None,
        )
        .unwrap();
    assert!(!finished.already_finished);

    let finished_again = config
        .finish_workflow(
            &workspace_id,
            vec![crate::SessionValidationGate {
                validation_spec_id: "val-session-workflow-finish".to_string(),
                required: true,
                outcome: Some("passed".to_string()),
                command: None,
            }],
            vec!["optional-node".to_string()],
            None,
        )
        .unwrap();
    assert!(finished_again.already_finished);
    assert_eq!(finished_again.record.run_id, finished.record.run_id);
}

#[test]
pub(super) fn workflow_finish_blocks_when_required_validation_guard_is_missing() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    // The spec must exist to be a legal validation_spec_id (AC5), but no
    // execution is ever recorded for it, so finish still blocks on missing
    // authoritative evidence rather than an unresolvable spec id.
    let test_store = test_store_for(&store_root);
    seed_validation_spec(&test_store, "val-session-workflow-finish");

    config
        .workflow_add_node(
            &workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("required-validation".to_string()),
                kind: SessionWorkflowNodeKind::Validation,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "must pass".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: Some("val-session-workflow-finish".to_string()),
            },
        )
        .unwrap();
    config
        .workflow_update_node_status(
            &workspace_id,
            "required-validation",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();

    let error = config
        .finish_workflow(&workspace_id, vec![], vec![], None)
        .unwrap_err();
    assert!(matches!(error, SessionError::FinishBlocked { .. }));
}

// ── Remediation regression coverage ─────────────────────────────────────────

/// A resolver returning a caller-controlled state for a specific URN.
pub(super) struct FixedStateResolver {
    pub(super) urn: String,
    pub(super) state: Option<String>,
}

impl SessionTicketStateResolver for FixedStateResolver {
    fn resolve_ticket_state(&self, ticket_urn: &str) -> Result<Option<String>, String> {
        if ticket_urn == self.urn {
            Ok(self.state.clone())
        } else {
            Err(format!("unexpected urn: {ticket_urn}"))
        }
    }
}

pub(super) struct BlockingTerminalResolver {
    pub(super) urn: String,
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl SessionTicketStateResolver for BlockingTerminalResolver {
    fn resolve_ticket_state(&self, ticket_urn: &str) -> Result<Option<String>, String> {
        if ticket_urn != self.urn {
            return Err(format!("unexpected urn: {ticket_urn}"));
        }
        self.entered.send(()).map_err(|error| error.to_string())?;
        self.release
            .lock()
            .map_err(|error| error.to_string())?
            .recv()
            .map_err(|error| error.to_string())?;
        Ok(Some("done".to_string()))
    }
}

pub(super) fn test_store_for(store_root: &std::path::Path) -> test_api::TestStoreConfig {
    test_api::TestStoreConfig::new(store_root.join(".test"))
}

pub(super) fn seed_validation_spec(store: &test_api::TestStoreConfig, spec_id: &str) {
    store
        .record_spec(&test_api::ValidationSpec::new(spec_id, spec_id))
        .unwrap();
}

pub(super) fn seed_execution(
    store: &test_api::TestStoreConfig,
    exec_id: &str,
    spec_id: &str,
    outcome: test_api::ValidationOutcome,
) {
    let mut execution =
        test_api::ValidationExecution::new(exec_id, spec_id, outcome, chrono::Utc::now());
    execution.provenance.domain = Some("session-api".to_string());
    execution.provenance.operation = Some("workflow-finish".to_string());
    execution.provenance.run_id = Some("remediation-test-run".to_string());
    execution.links.spec_ids = vec![spec_id.to_string()];
    store.record_execution(&execution).unwrap();
}

pub(super) fn add_required_validation_node(
    config: &SessionStoreConfig,
    workspace_id: &str,
    spec_id: &str,
) {
    config
        .workflow_add_node(
            workspace_id,
            SessionWorkflowNodeDraft {
                node_id: Some("required-validation".to_string()),
                kind: SessionWorkflowNodeKind::Validation,
                requirement: SessionWorkflowNodeRequirement::Required,
                title: "authoritative gate".to_string(),
                ticket_urn: None,
                spec_urn: None,
                anchor_urn: None,
                category: None,
                cached_ticket_title: None,
                validation_spec_id: Some(spec_id.to_string()),
            },
        )
        .unwrap();
    config
        .workflow_update_node_status(
            workspace_id,
            "required-validation",
            SessionWorkflowNodeStatus::Done,
            None,
        )
        .unwrap();
}

/// Critical: a caller submitting `passed` cannot override an authoritative
/// `failed` execution recorded in test-api.
#[test]
pub(super) fn workflow_finish_rejects_caller_passed_when_authoritative_failed() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone());
    let init = config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        })
        .unwrap();
    let workspace_id = init.context.session_id;

    let spec_id = "val-remediation-authority";
    let test_store = test_store_for(&store_root);
    seed_validation_spec(&test_store, spec_id);
    seed_execution(
        &test_store,
        "exec-authority-failed",
        spec_id,
        test_api::ValidationOutcome::Failed,
    );

    add_required_validation_node(&config, &workspace_id, spec_id);

    let error = config
        .finish_workflow(
            &workspace_id,
            vec![crate::SessionValidationGate {
                validation_spec_id: spec_id.to_string(),
                required: true,
                outcome: Some("passed".to_string()),
                command: None,
            }],
            vec![],
            None,
        )
        .unwrap_err();
    assert!(matches!(error, SessionError::FinishBlocked { .. }));
}

#[path = "validation_authority/node_repair.rs"]
mod node_repair;
#[allow(unused_imports)]
pub use node_repair::*;
