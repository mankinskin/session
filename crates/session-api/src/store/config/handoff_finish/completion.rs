#[allow(unused_imports)]
use super::*;

impl SessionStoreConfig {
    pub fn finish_workflow(
        &self,
        session_id: &str,
        validation: Vec<SessionValidationGate>,
        deferred_optional_node_ids: Vec<String>,
        resolver: Option<&dyn SessionTicketStateResolver>,
    ) -> Result<SessionFinishResult, SessionError> {
        let paths = self.runtime_paths_for_workspace(session_id)?;
        if let Some(result) = Self::existing_finish_result(&paths.finish_path)? {
            return Ok(result);
        }

        // Hold the mutation lock across evaluation and finish-record write so a
        // concurrent workflow mutation cannot interleave with finish.
        let _lock = self.acquire_runtime_lock(session_id)?;
        // Re-check under the lock: another finish may have won the race.
        if let Some(result) = Self::existing_finish_result(&paths.finish_path)? {
            return Ok(result);
        }

        let context = self.read_runtime_context(session_id)?;
        let snapshot = self.workflow_snapshot(session_id, resolver)?;
        let deferred = deferred_optional_node_ids
            .into_iter()
            .collect::<BTreeSet<_>>();

        Self::evaluate_workflow_completion(&context, &snapshot, &deferred)?;
        let validation = self.evaluate_required_validation(&context, validation)?;

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
    pub(in super::super) fn existing_finish_result(
        finish_path: &Path,
    ) -> Result<Option<SessionFinishResult>, SessionError> {
        Ok(
            read_json_if_exists::<SessionFinishRecord>(finish_path)?.map(|record| {
                SessionFinishResult {
                    record,
                    already_finished: true,
                }
            }),
        )
    }

    /// Pure completion predicate for finish: verify every required workflow node
    /// is done and every optional node is done or explicitly deferred with a
    /// reason. Ticket nodes whose live state could not be resolved fail closed.
    ///
    /// Extracted from `finish_workflow` so the completion invariant can be unit
    /// tested in isolation and so the locked finish path reads as a linear
    /// sequence rather than an oversized branching function.
    pub(in super::super) fn evaluate_workflow_completion(
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
            if node.requirement == crate::SessionWorkflowNodeRequirement::Required
                && matches!(
                    node.kind,
                    crate::SessionWorkflowNodeKind::Ticket | crate::SessionWorkflowNodeKind::Spec
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

            let is_done = node_is_effectively_done(node, live_states.get(&node.node_id));
            if node.requirement == crate::SessionWorkflowNodeRequirement::Required && !is_done {
                return Err(SessionError::FinishBlocked {
                    reason: format!("required node {} is not done", node.node_id),
                });
            }

            if node.requirement == crate::SessionWorkflowNodeRequirement::Optional && !is_done {
                let valid_defer = node.status == SessionWorkflowNodeStatus::Deferred
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
    pub(in super::super) fn evaluate_required_validation(
        &self,
        context: &SessionRuntimeContext,
        validation: Vec<SessionValidationGate>,
    ) -> Result<Vec<SessionValidationGate>, SessionError> {
        let validation = self.resolve_validation_gates(context, validation, true)?;
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
