#[derive(serde::Deserialize)]
struct LegacyRuntimeContext {
    #[serde(default)]
    active_run_id: String,
    #[serde(default)]
    runs: Vec<SessionRunLineage>,
    #[serde(default)]
    pinned_entities: Vec<SessionPinnedEntity>,
    #[serde(default)]
    workflow: SessionWorkflowGraph,
    #[serde(default)]
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl SessionStoreConfig {
    fn resolve_validation_gates(
        &self,
        context: &SessionRuntimeContext,
        validation: Vec<SessionValidationGate>,
        strict_required: bool,
    ) -> Result<Vec<SessionValidationGate>, SessionError> {
        let mut by_id = BTreeMap::<String, SessionValidationGate>::new();
        for gate in validation {
            by_id.insert(gate.validation_spec_id.clone(), gate);
        }

        let required_specs = context
            .workflow
            .nodes
            .iter()
            .filter(|node| {
                node.kind == crate::SessionWorkflowNodeKind::Validation
                    && node.requirement
                        == crate::SessionWorkflowNodeRequirement::Required
            })
            .map(|node| {
                node.validation_spec_id.clone().ok_or_else(|| {
                    SessionError::FinishBlocked {
                        reason: format!(
                            "required validation node {} is missing validation_spec_id; \
                             repair it with workflow_update_node (MCP: session_workflow_update_node) \
                             to set validation_spec_id, or remove the node with \
                             workflow_remove_node (MCP: session_workflow_remove_node)",
                            node.node_id
                        ),
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        if required_specs.is_empty() {
            return Ok(by_id.into_values().collect());
        }

        let test_store = self.test_store_config();
        for spec_id in required_specs {
            // Fail closed when a required guard references an unknown validation spec.
            test_store.get_spec(&spec_id).map_err(|error| {
                SessionError::FinishBlocked {
                    reason: format!(
                        "required validation {} is unavailable: {}",
                        spec_id, error
                    ),
                }
            })?;

            // Required outcomes are ALWAYS derived from the authoritative test-api
            // execution record. Caller-provided outcomes are never accepted as
            // completion authority; they may only identify or display a gate.
            let latest = test_store
                .list_executions(&ExecutionQuery {
                    validation_spec_id: Some(spec_id.clone()),
                    limit: Some(1),
                    ..ExecutionQuery::default()
                })
                .map_err(|error| SessionError::FinishBlocked {
                    reason: format!(
                        "required validation {} could not be queried: {}",
                        spec_id, error
                    ),
                })?;
            let outcome = latest
                .into_iter()
                .next()
                .map(|execution| validation_outcome_label(execution.outcome));

            // Fail closed for absent executions, failed executions, and blocked
            // executions when finish requires strict authoritative evidence.
            if strict_required && outcome.as_deref() != Some("passed") {
                return Err(SessionError::FinishBlocked {
                    reason: format!(
                        "required validation {} is not passed (authoritative outcome: {})",
                        spec_id,
                        outcome.as_deref().unwrap_or("no execution record")
                    ),
                });
            }

            by_id.insert(
                spec_id.clone(),
                SessionValidationGate {
                    validation_spec_id: spec_id,
                    required: true,
                    outcome,
                    command: None,
                },
            );
        }

        Ok(by_id.into_values().collect())
    }

    pub(crate) fn paths_for_session_id(
        &self,
        session_id: &str,
    ) -> Result<SessionStorePaths, SessionError> {
        if self.root.as_os_str().is_empty() {
            return Err(SessionError::EmptyStoreRoot);
        }
        validate_segment(session_id, false)?;

        let session_dir = self.root.join("sessions").join(session_id);
        let manifest_path = session_dir.join("session.json");
        let transcript_path = session_dir.join("transcript.json");
        let events_path = session_dir.join("events.json");

        if manifest_path.parent().is_none()
            || transcript_path.parent().is_none()
            || events_path.parent().is_none()
        {
            return Err(SessionError::InvalidStorePath(session_dir));
        }

        Ok(SessionStorePaths {
            session_dir,
            manifest_path,
            transcript_path,
            events_path,
        })
    }

    fn sessions_root(&self) -> Result<PathBuf, SessionError> {
        if self.root.as_os_str().is_empty() {
            return Err(SessionError::EmptyStoreRoot);
        }
        Ok(self.root.join("sessions"))
    }

    pub(super) fn runtime_paths_for_workspace(
        &self,
        session_id: &str,
    ) -> Result<SessionRuntimePaths, SessionError> {
        validate_session_id(session_id)?;
        let workspace_dir = self
            .sessions_root()?
            .join(session_id);
        let handoffs_dir = workspace_dir.join("handoffs");
        let finish_path = workspace_dir.join("finish.json");
        Ok(SessionRuntimePaths {
            workspace_dir,
            handoffs_dir,
            finish_path,
        })
    }

    fn persist_runtime_state(
        &self,
        context: &SessionRuntimeContext,
    ) -> Result<(), SessionError> {
        let paths = self.runtime_paths_for_workspace(&context.session_id)?;
        ensure_local_gitignore(&self.root)?;
        fs::create_dir_all(&paths.workspace_dir).map_err(|source| {
            SessionError::Io {
                path: paths.workspace_dir.clone(),
                source,
            }
        })?;
        let session_paths = self.paths_for_session_id(&context.session_id)?;
        let mut manifest = read_json_if_exists(&session_paths.manifest_path)?
            .unwrap_or_else(|| PersistedSessionManifest {
                schema_version: SESSION_SCHEMA_VERSION,
                session_id: context.session_id.clone(),
                source: "session-runtime-init".to_string(),
                started_at: context.created_at,
                captured_at: context.updated_at,
                metadata: SessionMetadata {
                    workspace_slug: self.workspace_slug.clone(),
                    conversation_id: None,
                    agent_id: None,
                    ticket_id: None,
                    model: None,
                    trigger: Some("session-runtime-init".to_string()),
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
            });
        manifest.captured_at = context.updated_at;
        manifest.active_run_id = context.active_run_id.clone();
        manifest.runs = context.runs.clone();
        manifest.pinned_entities = context.pinned_entities.clone();
        manifest.workflow = context.workflow.clone();
        write_json(&session_paths.manifest_path, &manifest)
    }

    fn ticket_store_root(&self) -> PathBuf {
        sibling_store_root(&self.root, ".ticket")
    }

    fn test_store_root(&self) -> PathBuf {
        sibling_store_root(&self.root, ".test")
    }

    fn test_store_config(&self) -> TestStoreConfig {
        TestStoreConfig::new(
            self.test_store_root(),
            self.workspace_slug.clone(),
        )
    }

    fn default_ticket_state_resolver(&self) -> DefaultTicketStateResolver {
        DefaultTicketStateResolver {
            session_store_root: self.root.clone(),
            workspace_slug: self.workspace_slug.clone(),
            ticket_stores: std::sync::Mutex::new(BTreeMap::new()),
            spec_stores: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    fn resolve_session_id(
        &self,
        requested: Option<String>,
    ) -> Result<String, SessionError> {
        let requested = requested.map(|id| {
            validate_session_id(&id)?;
            Ok(id)
        }).transpose()?;

        if let Some(provisioned) = self.provisioned_worktree_session_id() {
            if let Some(requested) = requested {
                if requested != provisioned {
                    return Err(SessionError::SessionIdentityMismatch {
                        requested,
                        provisioned,
                    });
                }
            }
            return Ok(provisioned);
        }

        if let Some(requested) = requested {
            return Ok(requested);
        }

        Ok(Uuid::new_v4().to_string())
    }

    fn provisioned_worktree_session_id(&self) -> Option<String> {
        let mut path = self.root.canonicalize().unwrap_or_else(|_| {
            if self.root.is_absolute() {
                self.root.clone()
            } else {
                std::env::current_dir()
                    .map(|current_dir| current_dir.join(&self.root))
                    .unwrap_or_else(|_| self.root.clone())
            }
        });
        while let Some(parent) = path.parent() {
            if parent.file_name().is_some_and(|name| name == ".worktrees") {
                let value = path.file_name()?.to_str()?;
                return Uuid::parse_str(value).ok().map(|id| id.to_string());
            }
            path = parent.to_path_buf();
        }
        None
    }

    fn read_legacy_runtime_context(
        &self,
        session_id: &str,
    ) -> Result<Option<LegacyRuntimeContext>, SessionError> {
        let paths = self.runtime_paths_for_workspace(session_id)?;
        read_json_if_exists(&paths.workspace_dir.join("context.json"))
    }

    pub fn plan_capture(
        &self,
        request: SessionCaptureRequest,
    ) -> Result<SessionStorePlan, SessionError> {
        let (mut record, events) = request.into_record_and_events()?;
        
        // Compute cost_usd for turns with token attribution (ticket 6549b6a7)
        let price_table = crate::price_loader::load_price_table(&self.root).ok();
        if let Some(table) = &price_table {
            for turn in &mut record.turns {
                if let Some(meta) = &mut turn.event_meta {
                    if let (Some(model_id), Some(input), Some(output)) =
                        (&meta.model_id, meta.input_tokens, meta.output_tokens)
                    {
                        meta.cost_usd = crate::price_loader::compute_cost_usd(
                            model_id,
                            input,
                            output,
                            meta.cache_read_tokens.unwrap_or(0),
                            meta.cache_write_tokens.unwrap_or(0),
                            table,
                        );
                    }
                }
            }
        }
        
        let paths = self.paths_for(&record)?;
        let events = if events.is_empty() {
            None
        } else {
            Some(PersistedSessionEvents {
                schema_version: record.schema_version,
                session_id: record.session_id.clone(),
                captured_at: record.captured_at,
                events,
            })
        };
        Ok(SessionStorePlan {
            record,
            paths,
            events,
        })
    }

    pub fn persist_capture(
        &self,
        request: SessionCaptureRequest,
    ) -> Result<SessionStorePlan, SessionError> {
        let plan = self.plan_capture(request)?;
        plan.persist()?;
        Ok(plan)
    }

    fn persist_record(
        &self,
        record: SessionRecord,
    ) -> Result<SessionStorePlan, SessionError> {
        let paths = self.paths_for(&record)?;
        let plan = SessionStorePlan {
            record,
            paths,
            events: None,
        };
        plan.persist()?;
        Ok(plan)
    }

}
