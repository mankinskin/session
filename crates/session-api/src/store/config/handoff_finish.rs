impl SessionStoreConfig {
    pub fn create_handoff_record(
        &self,
        session_id: &str,
        package: Option<SessionHandoffPackage>,
        validation: Vec<SessionValidationGate>,
        resolver: Option<&dyn SessionTicketStateResolver>,
    ) -> Result<SessionHandoffRecord, SessionError> {
        // Validate package completeness when a package is supplied.
        // Missing `objective` is a hard error; missing list fields are a soft warning.
        let mut package = package;
        if let Some(ref mut pkg) = package {
            let missing = pkg.missing_fields();
            if missing.contains(&"objective") {
                return Err(SessionError::HandoffPackageIncomplete {
                    fields: "objective".to_string(),
                });
            }
            let readiness_holds = !pkg.objective.trim().is_empty()
                && pkg.open_escalations.is_empty();
            let missing_upward_context = pkg.missing_upward_context_fields();
            if !missing_upward_context.is_empty() {
                let fields = missing_upward_context.join(", ");
                if readiness_holds {
                    return Err(SessionError::HandoffPackageIncomplete { fields });
                }
                eprintln!(
                    "[session-api] handoff package is missing required upward \
                     context fields ({fields}); the handoff persists but is not \
                     implementation-ready"
                );
            }
            if !missing.is_empty() {
                eprintln!(
                    "[session-api] handoff package is missing required list \
                     fields ({fields}); the handoff persists but is not \
                     implementation-ready",
                    fields = missing.join(", ")
                );
            }

            // AC1/AC2: every `target_files` entry must be a repo-root-relative,
            // forward-slash path that exists on disk, verified at creation
            // time (not left for the consuming Implement Agent to discover).
            // An active session worktree is authoritative because the session
            // may be persisted from an MCP server launched in the main checkout.
            let root = self.handoff_path_validation_root(session_id)?;
            for target in pkg.target_files.iter_mut() {
                let normalized = normalize_repo_relative_path(target);
                if !verify_repo_relative_path_exists(&root, &normalized) {
                    return Err(SessionError::HandoffPathNotFound {
                        path: target.clone(),
                        workspace_root: root,
                    });
                }
                *target = normalized;
            }

            // Path-shaped `context_anchors` (store-qualified physical paths,
            // e.g. `memory-api/.ticket/tickets/<uuid>`) get the same
            // creation-time verification; free-form anchors (URNs, ids,
            // prose) are left untouched.
            for anchor in pkg.context_anchors.iter_mut() {
                if !looks_like_path(anchor) {
                    continue;
                }
                let normalized = normalize_repo_relative_path(anchor);
                if !verify_repo_relative_path_exists(&root, &normalized) {
                    return Err(SessionError::HandoffPathNotFound {
                        path: anchor.clone(),
                        workspace_root: root,
                    });
                }
                *anchor = normalized;
            }
        }

        let context = self.read_runtime_context(session_id)?;
        let workflow =
            self.workflow_snapshot(session_id, resolver)?;
        // Fail before any handoff files are written so a bad graph never leaves a partial folder.
        let structural_issues = validate_workflow_graph(&workflow.workflow);
        if !structural_issues.is_empty() {
            return Err(SessionError::WorkflowGraphInvalid {
                session_id: session_id.to_string(),
                issues: structural_issues
                    .iter()
                    .map(|issue| format!("{}: {}", issue.code, issue.message))
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
        if !workflow.diagnostics.is_empty() {
            return Err(SessionError::WorkflowDiagnosticsUnresolved {
                session_id: session_id.to_string(),
                diagnostics: workflow
                    .diagnostics
                    .iter()
                    .map(|diag| {
                        format!("{} [{}]: {}", diag.node_id, diag.code, diag.message)
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
        let validation =
            self.resolve_validation_gates(&context, validation, false)?;
        let view = self.view_runtime_context(session_id)?;
        let handoff_id = Uuid::new_v4().to_string();
        let resume_command = format!(
            "session-cli resume --session-id {} --predecessor-run-id {}",
            context.session_id, context.active_run_id
        );

        let (
            objective,
            target_tickets,
            higher_level_objective,
            upward_context,
            target_files,
            decisions,
            non_goals,
            context_anchors,
            open_escalations,
            risk_notes,
            predecessor_handoff,
        ) = package
            .map(|pkg| {
                (
                    pkg.objective,
                    pkg.target_tickets,
                    pkg.higher_level_objective,
                    pkg.upward_context,
                    pkg.target_files,
                    pkg.decisions,
                    pkg.non_goals,
                    pkg.context_anchors,
                    pkg.open_escalations,
                    pkg.risk_notes,
                    pkg.predecessor_handoff,
                )
            })
            .unwrap_or_default();

        let record = SessionHandoffRecord {
            handoff_id: handoff_id.clone(),
            session_id: context.session_id.clone(),
            outgoing_run_id: context.active_run_id,
            created_at: chrono::Utc::now(),
            resume_command,
            target_session_id: None,
            pinned_entities: view.pinned_headers,
            workflow,
            validation,
            objective,
            target_tickets: target_tickets.clone(),
            higher_level_objective,
            upward_context,
            target_files,
            decisions,
            non_goals,
            context_anchors,
            open_escalations,
            risk_notes,
            predecessor_handoff,
        };

        let paths = self.runtime_paths_for_workspace(session_id)?;
        fs::create_dir_all(&paths.handoffs_dir).map_err(|source| {
            SessionError::Io {
                path: paths.handoffs_dir.clone(),
                source,
            }
        })?;

        // Create handoff folder and write both JSON and Markdown
        let handoff_folder = paths.handoffs_dir.join(&handoff_id);
        fs::create_dir_all(&handoff_folder).map_err(|source| {
            SessionError::Io {
                path: handoff_folder.clone(),
                source,
            }
        })?;

        let handoff_json_path = handoff_folder.join("handoff.json");
        write_json(&handoff_json_path, &record)?;

        let handoff_md_path = handoff_folder.join("handoff.md");
        let ticket_store = TicketStore::open(&self.ticket_store_root()).ok();
        let markdown_content = render_handoff_record_markdown(&record, ticket_store.as_ref());
        fs::write(&handoff_md_path, markdown_content).map_err(|source| {
            SessionError::Io {
                path: handoff_md_path.clone(),
                source,
            }
        })?;

        // Mirror the handoff onto each target ticket (best-effort; the handoff
        // record is the authoritative source of truth).
        if !target_tickets.is_empty() {
            let _ = self.mirror_handoff_to_tickets(&record, &target_tickets);
        }

        // Record this handoff as emitted by its source session (best-effort:
        // the source session may not have a session.json yet, e.g. a
        // workflow-only workspace session, and the handoff record itself
        // remains the authoritative source of truth).
        if let Ok(mut source_record) =
            self.read_session(&record.session_id)
        {
            if !source_record
                .emitted_handoff_ids
                .iter()
                .any(|id| id == &record.handoff_id)
            {
                source_record
                    .emitted_handoff_ids
                    .push(record.handoff_id.clone());
                let _ = self.persist_record(source_record);
            }
        }

        Ok(record)
    }

    fn handoff_path_validation_root(
        &self,
        session_id: &str,
    ) -> Result<PathBuf, SessionError> {
        let active_worktree = self.worktree_registry_entry(session_id)?.and_then(
            |entry| {
                (entry.assignment.status == SessionWorktreeStatus::Active)
                    .then_some(entry.assignment.path)
            },
        );
        Ok(active_worktree.unwrap_or_else(workspace_root))
    }

    fn mirror_handoff_to_tickets(
        &self,
        record: &SessionHandoffRecord,
        target_tickets: &[crate::SessionHandoffTargetTicket],
    ) -> Result<(), SessionError> {
        let store =
            TicketStore::open_or_init(&self.ticket_store_root()).map_err(
                |error| {
                    SessionError::InvalidHookInput(format!(
                        "ticket store unavailable for handoff mirror: {error}"
                    ))
                },
            )?;

        // A handoff is working-session context, not ticket content: mirror
        // it as a free-form `notes` part addressed by its own stable id
        // rather than injecting an untyped `handoff_package` blob into the
        // ticket's manifest fields (the same untyped whole-field-write bug
        // this ticket fixes for `description`). Each handoff creates a new
        // part, so repeated handoffs accumulate as distinct, addressable
        // notes instead of overwriting one field.
        let mut content = format!(
            "# Handoff {}\n\n**Objective:** {}\n",
            record.handoff_id, record.objective
        );
        if !record.target_tickets.is_empty() {
            content.push_str(&format!(
                "\n**Target tickets:** {}\n",
                record
                    .target_tickets
                    .iter()
                    .map(|ticket| ticket.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !record.target_files.is_empty() {
            content.push_str(&format!(
                "\n**Target files:** {}\n",
                record.target_files.join(", ")
            ));
        }
        if !record.validation.is_empty() {
            let validation_json =
                serde_json::to_string_pretty(&record.validation)
                    .unwrap_or_default();
            content.push_str(&format!(
                "\n**Validation:**\n```json\n{validation_json}\n```\n"
            ));
        }
        if !record.open_escalations.is_empty() {
            content.push_str(&format!(
                "\n**Open escalations:** {}\n",
                record.open_escalations.join(", ")
            ));
        }

        for target_ticket in target_tickets {
            let ticket_id = match Uuid::parse_str(&target_ticket.id) {
                Ok(id) => id,
                Err(_) => continue,
            };
            // Best-effort: ignore individual ticket write errors.
            let _ = store.write_part(
                &ticket_id,
                Uuid::new_v4(),
                "notes",
                &content,
                None,
            );
        }
        Ok(())
    }

    pub fn create_handoff_result(
        &self,
        session_id: &str,
        package: Option<SessionHandoffPackage>,
        validation: Vec<SessionValidationGate>,
        resolver: Option<&dyn SessionTicketStateResolver>,
    ) -> Result<SessionHandoffResult, SessionError> {
        let record = self.create_handoff_record(
            session_id,
            package,
            validation,
            resolver,
        )?;
        let paths = self.runtime_paths_for_workspace(session_id)?;
        let record_path = paths
            .handoffs_dir
            .join(&record.handoff_id);
        Ok(SessionHandoffResult {
            render: render_handoff_record_terminal(&record),
            record,
            record_path: record_path.to_string_lossy().into_owned(),
        })
    }

    pub fn render_handoff_terminal(
        &self,
        session_id: &str,
        package: Option<SessionHandoffPackage>,
        validation: Vec<SessionValidationGate>,
        resolver: Option<&dyn SessionTicketStateResolver>,
    ) -> Result<String, SessionError> {
        let result = self.create_handoff_result(
            session_id,
            package,
            validation,
            resolver,
        )?;
        Ok(result.render)
    }

    pub fn resume_workspace_context(
        &self,
        session_id: &str,
        predecessor_run_id: &str,
    ) -> Result<SessionRuntimeInitResult, SessionError> {
        let init = self.init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(session_id.to_string()),
            predecessor_run_id: Some(predecessor_run_id.to_string()),
            force_new_run: true,
        })?;

        if init.run.run_id == predecessor_run_id {
            return Err(SessionError::InvalidHookInput(
                "resume must produce a new run id".to_string(),
            ));
        }
        Ok(init)
    }

    pub fn finish_workflow(
        &self,
        session_id: &str,
        validation: Vec<SessionValidationGate>,
        deferred_optional_node_ids: Vec<String>,
        resolver: Option<&dyn SessionTicketStateResolver>,
    ) -> Result<SessionFinishResult, SessionError> {
        let paths = self.runtime_paths_for_workspace(session_id)?;
        if let Some(result) = Self::existing_finish_result(&paths.finish_path)?
        {
            return Ok(result);
        }

        // Hold the mutation lock across evaluation and finish-record write so a
        // concurrent workflow mutation cannot interleave with finish.
        let _lock = self.acquire_runtime_lock(session_id)?;
        // Re-check under the lock: another finish may have won the race.
        if let Some(result) = Self::existing_finish_result(&paths.finish_path)?
        {
            return Ok(result);
        }

        let context = self.read_runtime_context(session_id)?;
        let snapshot =
            self.workflow_snapshot(session_id, resolver)?;
        let deferred = deferred_optional_node_ids
            .into_iter()
            .collect::<BTreeSet<_>>();

        Self::evaluate_workflow_completion(&context, &snapshot, &deferred)?;
        let validation =
            self.evaluate_required_validation(&context, validation)?;

        let record = SessionFinishRecord {
            session_id: session_id.to_string(),
            run_id: context.active_run_id,
            finished_at: chrono::Utc::now(),
            deferred_optional_node_ids: deferred.into_iter().collect(),
            validation,
        };

        if let Some(parent) = paths.finish_path.parent() {
            fs::create_dir_all(parent).map_err(|source| SessionError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        write_json(&paths.finish_path, &record)?;
        Ok(SessionFinishResult {
            record,
            already_finished: false,
        })
    }

    /// Load a persisted finish record (if any) as an idempotent finish result.
    ///
    /// Extracted so `finish_workflow` can perform the pre-lock fast path and the
    /// under-lock re-check with a single branch each instead of duplicating the
    /// read-and-wrap logic inline.
    fn existing_finish_result(
        finish_path: &Path
    ) -> Result<Option<SessionFinishResult>, SessionError> {
        Ok(
            read_json_if_exists::<SessionFinishRecord>(finish_path)?.map(
                |record| SessionFinishResult {
                    record,
                    already_finished: true,
                },
            ),
        )
    }

    /// Pure completion predicate for finish: verify every required workflow node
    /// is done and every optional node is done or explicitly deferred with a
    /// reason. Ticket nodes whose live state could not be resolved fail closed.
    ///
    /// Extracted from `finish_workflow` so the completion invariant can be unit
    /// tested in isolation and so the locked finish path reads as a linear
    /// sequence rather than an oversized branching function.
    fn evaluate_workflow_completion(
        context: &SessionRuntimeContext,
        snapshot: &SessionWorkflowSnapshot,
        deferred: &BTreeSet<String>,
    ) -> Result<(), SessionError> {
        let live_states = snapshot
            .resolutions
            .iter()
            .map(|item| (item.node_id.clone(), item.live_ticket_state.clone()))
            .collect::<BTreeMap<_, _>>();
        // Ticket-state diagnostics (unavailable / misrouted / not-found) must be
        // able to block finish; they cannot be silently ignored.
        let diagnostics_by_node = snapshot
            .diagnostics
            .iter()
            .map(|diag| (diag.node_id.clone(), diag.message.clone()))
            .collect::<BTreeMap<_, _>>();

        for node in &context.workflow.nodes {
            // A required ticket- or spec-backed node whose live state could not
            // be resolved (missing, misrouted, or otherwise unavailable) fails
            // closed with an explicit unavailable reason instead of a generic
            // "not done". Spec gating is symmetric to ticket gating.
            if node.requirement
                == crate::SessionWorkflowNodeRequirement::Required
                && matches!(
                    node.kind,
                    crate::SessionWorkflowNodeKind::Ticket
                        | crate::SessionWorkflowNodeKind::Spec
                )
            {
                if let Some(message) = diagnostics_by_node.get(&node.node_id) {
                    let backing = match node.kind {
                        crate::SessionWorkflowNodeKind::Spec => "spec",
                        _ => "ticket",
                    };
                    return Err(SessionError::FinishBlocked {
                        reason: format!(
                            "required {} node {} has unavailable live state: {}",
                            backing, node.node_id, message
                        ),
                    });
                }
            }

            let is_done =
                node_is_effectively_done(node, live_states.get(&node.node_id));
            if node.requirement
                == crate::SessionWorkflowNodeRequirement::Required
                && !is_done
            {
                return Err(SessionError::FinishBlocked {
                    reason: format!(
                        "required node {} is not done",
                        node.node_id
                    ),
                });
            }

            if node.requirement
                == crate::SessionWorkflowNodeRequirement::Optional
                && !is_done
            {
                let valid_defer = node.status
                    == SessionWorkflowNodeStatus::Deferred
                    && node.deferred_reason.is_some()
                    && deferred.contains(&node.node_id);
                if !valid_defer {
                    return Err(SessionError::FinishBlocked {
                        reason: format!(
                            "optional node {} must be deferred with a reason",
                            node.node_id
                        ),
                    });
                }
            }
        }

        Ok(())
    }

    /// Resolve required validation gates from the authoritative test store and
    /// verify each required gate passed, returning the merged gate list to
    /// persist. Extracted from `finish_workflow` to keep authoritative-gate
    /// evaluation testable independently of the locked finish sequence.
    fn evaluate_required_validation(
        &self,
        context: &SessionRuntimeContext,
        validation: Vec<SessionValidationGate>,
    ) -> Result<Vec<SessionValidationGate>, SessionError> {
        let validation =
            self.resolve_validation_gates(context, validation, true)?;
        for gate in &validation {
            if gate.required && gate.outcome.as_deref() != Some("passed") {
                return Err(SessionError::FinishBlocked {
                    reason: format!(
                        "required validation {} is not passed",
                        gate.validation_spec_id
                    ),
                });
            }
        }
        Ok(validation)
    }

}
