impl SessionStoreConfig {
    /// Reject workflow/pin mutations once the workspace has a persisted finish
    /// record. Finished workspaces are immutable: this guarantees a finished
    /// workspace cannot silently drift into an incomplete state while still
    /// returning a stale success from `finish_workflow`.
    fn ensure_workspace_not_finished(
        &self,
        session_id: &str,
    ) -> Result<(), SessionError> {
        let paths = self.runtime_paths_for_workspace(session_id)?;
        if paths.finish_path.exists() {
            return Err(SessionError::WorkspaceFinished {
                session_id: session_id.to_string(),
            });
        }
        Ok(())
    }

    /// Begin a runtime mutation: acquire the exclusive lock and *then* verify the
    /// workspace is not finished.
    ///
    /// Ordering is load-bearing. The finished-check must run under the lock so it
    /// cannot race with `finish_workflow`. Consider two threads: if the check ran
    /// before the lock, a mutation could observe "not finished", block on the
    /// lock while finish commits its record and releases the lock, then acquire
    /// the lock and mutate a workspace that is now finished — silently drifting a
    /// finished workspace into an incomplete state. Checking after the lock is
    /// held closes that window: any mutation that wins the lock after finish
    /// observes the finish record and is rejected with [`SessionError::WorkspaceFinished`].
    fn begin_runtime_mutation(
        &self,
        session_id: &str,
    ) -> Result<RuntimeMutationLock, SessionError> {
        let lock = self.acquire_runtime_lock(session_id)?;
        self.ensure_workspace_not_finished(session_id)?;
        Ok(lock)
    }

    /// Acquire an exclusive lock over a workspace runtime context for the duration
    /// of a read-modify-write mutation. This prevents two concurrent mutations from
    /// both reading the same context and silently clobbering each other's write.
    /// The lock is an OS-held exclusive file lock. The lock file remains in place
    /// between owners so releasing one file handle cannot unlink the inode used by
    /// a successor. The operating system releases the lock if the process exits.
    pub(super) fn acquire_runtime_lock(
        &self,
        session_id: &str,
    ) -> Result<RuntimeMutationLock, SessionError> {
        let paths = self.runtime_paths_for_workspace(session_id)?;
        fs::create_dir_all(&paths.workspace_dir).map_err(|source| {
            SessionError::Io {
                path: paths.workspace_dir.clone(),
                source,
            }
        })?;
        let lock_path = paths.workspace_dir.join(".context.lock");

        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)
            .map_err(|source| SessionError::Io {
                path: lock_path.clone(),
                source,
            })?;

        match file.try_lock() {
            Ok(()) => Ok(RuntimeMutationLock { file }),
            Err(fs::TryLockError::WouldBlock) =>
                Err(SessionError::RuntimeMutationConflict {
                    session_id: session_id.to_string(),
                }),
            Err(fs::TryLockError::Error(source)) => Err(SessionError::Io {
                path: lock_path,
                source,
            }),
        }
    }

    pub fn pin_runtime_entity(
        &self,
        session_id: &str,
        entity_urn: &str,
        relation: Option<String>,
        reason: Option<String>,
    ) -> Result<SessionRuntimeContext, SessionError> {
        self.pin_runtime_entity_with_sink(
            session_id,
            entity_urn,
            relation,
            reason,
            None,
        )
    }

    pub fn pin_runtime_entity_with_sink(
        &self,
        session_id: &str,
        entity_urn: &str,
        relation: Option<String>,
        reason: Option<String>,
        feedback_sink: Option<&dyn SessionPinFeedbackSink>,
    ) -> Result<SessionRuntimeContext, SessionError> {
        let _lock = self.begin_runtime_mutation(session_id)?;
        let mut context = self.read_runtime_context(session_id)?;
        let now = chrono::Utc::now();
        let kind = parse_entity_urn_kind(entity_urn)?;

        if let Some(existing) = context.find_pin_mut(entity_urn) {
            existing.last_used_at = now;
            if relation.is_some() {
                existing.relation = relation.clone();
            }
            if reason.is_some() {
                existing.reason = reason.clone();
            }
        } else {
            context.pinned_entities.push(SessionPinnedEntity {
                urn: entity_urn.to_string(),
                kind,
                relation,
                reason,
                pinned_at: now,
                last_used_at: now,
            });
            context
                .pinned_entities
                .sort_by(|left, right| left.urn.cmp(&right.urn));
        }

        context.updated_at = now;
        self.persist_runtime_state(&context)?;

        if let Some(sink) = feedback_sink {
            let _ = sink.record_pin_usage(
                &context.session_id,
                &context.active_run_id,
                entity_urn,
            );
        }

        Ok(context)
    }

    pub fn unpin_runtime_entity(
        &self,
        session_id: &str,
        entity_urn: &str,
    ) -> Result<SessionRuntimeContext, SessionError> {
        parse_entity_urn_kind(entity_urn)?;
        let _lock = self.begin_runtime_mutation(session_id)?;
        let mut context = self.read_runtime_context(session_id)?;
        let changed = context.remove_pin(entity_urn);
        if changed {
            context.updated_at = chrono::Utc::now();
            self.persist_runtime_state(&context)?;
        }
        Ok(context)
    }

    pub fn view_runtime_context(
        &self,
        session_id: &str,
    ) -> Result<SessionRuntimeView, SessionError> {
        let context = self.read_runtime_context(session_id)?;
        let mut pinned_headers = context
            .pinned_entities
            .iter()
            .map(|pin| SessionPinnedEntityHeader {
                urn: pin.urn.clone(),
                kind: pin.kind,
                relation: pin.relation.clone(),
                reason: pin.reason.clone(),
            })
            .collect::<Vec<_>>();
        pinned_headers.sort_by(|left, right| left.urn.cmp(&right.urn));

        Ok(SessionRuntimeView {
            session_id: context.session_id,
            active_run_id: context.active_run_id,
            pinned_count: pinned_headers.len(),
            pinned_headers,
        })
    }

    pub fn render_pinned_rule_instructions(
        &self,
        session_id: &str,
    ) -> Result<String, SessionError> {
        let context = self.read_runtime_context(session_id)?;
        // Missing/unopenable rule store degrades to no rule pins rather than failing the render.
        let rule_store = match RuleStore::open(&sibling_store_root(&self.root, ".rule")) {
            Ok(store) => Some(store),
            Err(error) => {
                eprintln!(
                    "[session-api] rule store unavailable ({error}); \
                     skipping pinned rule resolution"
                );
                None
            }
        };
        let mut rules = Vec::new();

        if let Some(rule_store) = rule_store.as_ref() {
            for pin in context
                .pinned_entities
                .iter()
                .filter(|pin| pin.kind == SessionPinnedEntityKind::Rule)
            {
                let parsed = parse_entity_urn(&pin.urn)?;
                if parsed.workspace_slug != self.workspace_slug {
                    return Err(SessionError::InvalidHookInput(format!(
                        "unsupported cross-workspace rule routing: URN workspace `{}` does not match session workspace `{}` ({})",
                        parsed.workspace_slug, self.workspace_slug, pin.urn
                    )));
                }
                match rule_store.get(&parsed.entity_id) {
                    Ok(rule) => rules.push(rule),
                    Err(error) => {
                        eprintln!(
                            "[session-api] pinned rule {} could not be \
                             resolved ({error}); skipping",
                            pin.urn
                        );
                    }
                }
            }
        }

        rules.sort_by_key(|rule| {
            (
                rule.order_key().unwrap_or_default(),
                rule.slug().unwrap_or("").to_string(),
            )
        });
        Ok(rule_api::render_markdown_file(&rules))
    }

    pub fn workflow_add_node(
        &self,
        session_id: &str,
        draft: SessionWorkflowNodeDraft,
    ) -> Result<SessionRuntimeContext, SessionError> {
        self.workflow_add_nodes(session_id, vec![draft])
    }

    pub fn workflow_add_nodes(
        &self,
        session_id: &str,
        drafts: Vec<SessionWorkflowNodeDraft>,
    ) -> Result<SessionRuntimeContext, SessionError> {
        for (index, draft) in drafts.iter().enumerate() {
            self.validate_workflow_node_draft(draft).map_err(|error| {
                indexed_workflow_error("nodes", index, error)
            })?;
        }

        let _lock = self.begin_runtime_mutation(session_id)?;
        let mut context = self.read_runtime_context(session_id)?;
        let now = chrono::Utc::now();
        let mut changed = false;

        for draft in drafts {
            let node_id = draft
                .node_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            if context
                .workflow
                .nodes
                .iter()
                .any(|node| node.node_id == node_id)
            {
                continue;
            }

            context.workflow.nodes.push(SessionWorkflowNode {
                node_id,
                kind: draft.kind,
                requirement: draft.requirement,
                status: SessionWorkflowNodeStatus::Pending,
                title: draft.title,
                created_at: now,
                updated_at: now,
                ticket_urn: draft.ticket_urn,
                spec_urn: draft.spec_urn,
                anchor_urn: draft.anchor_urn,
                category: draft.category,
                cached_ticket_title: draft.cached_ticket_title,
                deferred_reason: None,
                validation_spec_id: draft.validation_spec_id,
            });
            changed = true;
        }

        if changed {
            sort_workflow_graph(&mut context.workflow);
            context.updated_at = now;
            self.persist_runtime_state(&context)?;
        }
        Ok(context)
    }
}

impl SessionStoreConfig {
    /// Validate a workflow node draft before it is persisted.
    ///
    /// Every behavioral kind that gates finish (`ticket`, `spec`,
    /// `validation`) requires its matching side-data field to be present,
    /// non-empty, and (for `validation`) resolvable against the sibling
    /// `.test` store. Rejecting these at creation time is the primary fix for
    /// the wedge where a `validation` node with a null/empty
    /// `validation_spec_id` could be added and would then permanently block
    /// `session_finish`/`session_handoff` with no way to identify the cause
    /// beyond a generic "missing field" message. Any node that is already
    /// wedged in a persisted graph can be repaired with
    /// `workflow_update_node`/`workflow_remove_node`
    /// (`session_workflow_update_node`/`session_workflow_remove_node` at the
    /// MCP layer) instead of being stuck forever or "fixed" by piling on a
    /// second required node.
    fn validate_workflow_node_draft(
        &self,
        draft: &SessionWorkflowNodeDraft,
    ) -> Result<(), SessionError> {
        if draft.title.trim().is_empty() {
            return Err(SessionError::InvalidHookInput(
                "workflow node title cannot be empty".to_string(),
            ));
        }

        if draft.kind == crate::SessionWorkflowNodeKind::Ticket {
            let ticket_urn = draft
                .ticket_urn
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    SessionError::InvalidHookInput(
                        "ticket workflow node requires a non-empty ticket_urn"
                            .to_string(),
                    )
                })?;
            let parsed = parse_entity_urn(ticket_urn)?;
            if parsed.kind != SessionPinnedEntityKind::Ticket {
                return Err(SessionError::InvalidHookInput(format!(
                    "ticket workflow node requires a ticket URN, got {}",
                    ticket_urn
                )));
            }
        } else if draft.ticket_urn.is_some() {
            return Err(SessionError::InvalidHookInput(
                "only ticket workflow nodes may set ticket_urn; for a non-gating reference, use anchor_urn or pin the ticket"
                    .to_string(),
            ));
        }

        if draft.kind == crate::SessionWorkflowNodeKind::Spec {
            let spec_urn = draft
                .spec_urn
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    SessionError::InvalidHookInput(
                        "spec workflow node requires a non-empty spec_urn"
                            .to_string(),
                    )
                })?;
            let parsed = parse_entity_urn(spec_urn)?;
            if parsed.kind != SessionPinnedEntityKind::Spec {
                return Err(SessionError::InvalidHookInput(format!(
                    "spec workflow node requires a spec URN, got {}",
                    spec_urn
                )));
            }
        } else if draft.spec_urn.is_some() {
            return Err(SessionError::InvalidHookInput(
                "only spec workflow nodes may set spec_urn; for a non-gating reference, use anchor_urn or pin the spec"
                    .to_string(),
            ));
        }

        if draft.kind == crate::SessionWorkflowNodeKind::Validation {
            let spec_id = draft
                .validation_spec_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    SessionError::InvalidHookInput(
                        "validation workflow node requires a non-empty validation_spec_id"
                            .to_string(),
                    )
                })?;
            // Resolve against the authoritative test-api store so a node UUID
            // (or any other non-spec id) cannot be passed as a validation
            // spec id and silently wedge finish later.
            self.test_store_config().get_spec(spec_id).map_err(|error| {
                SessionError::InvalidHookInput(format!(
                    "validation workflow node validation_spec_id `{spec_id}` does not resolve to a spec in the .test store: {error}"
                ))
            })?;
        } else if draft.validation_spec_id.is_some() {
            return Err(SessionError::InvalidHookInput(
                "only validation workflow nodes may set validation_spec_id"
                    .to_string(),
            ));
        }

        if let Some(anchor_urn) = draft.anchor_urn.as_deref() {
            let parsed = parse_entity_urn(anchor_urn)?;
            if !matches!(
                parsed.kind,
                SessionPinnedEntityKind::Ticket | SessionPinnedEntityKind::Spec
            ) {
                return Err(SessionError::InvalidHookInput(format!(
                    "anchor_urn requires a ticket or spec URN, got {anchor_urn}"
                )));
            }
        }

        Ok(())
    }
}

pub(super) fn indexed_workflow_error(
    collection: &str,
    index: usize,
    error: SessionError,
) -> SessionError {
    let message = match error {
        SessionError::InvalidHookInput(message) => message,
        other => other.to_string(),
    };
    SessionError::InvalidHookInput(format!("{collection}[{index}]: {message}"))
}
