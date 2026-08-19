impl SessionStoreConfig {
    pub fn workflow_update_node_status(
        &self,
        session_id: &str,
        node_id: &str,
        status: SessionWorkflowNodeStatus,
        deferred_reason: Option<String>,
    ) -> Result<SessionRuntimeContext, SessionError> {
        let _lock = self.begin_runtime_mutation(session_id)?;
        let mut context = self.read_runtime_context(session_id)?;
        let node = context
            .workflow
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
            .ok_or_else(|| {
                SessionError::InvalidHookInput(format!(
                    "unknown workflow node id: {node_id}"
                ))
            })?;

        node.status = status;
        node.deferred_reason = if status == SessionWorkflowNodeStatus::Deferred
        {
            deferred_reason
        } else {
            None
        };
        node.updated_at = chrono::Utc::now();
        context.updated_at = node.updated_at;
        self.persist_runtime_state(&context)?;
        Ok(context)
    }

    /// Repair surface for a node that is already wedged in a persisted
    /// graph (for example a `validation` node with a missing
    /// `validation_spec_id`, or a `ticket`/`spec` node with a missing URN).
    ///
    /// Every field in `patch` is optional: `None` leaves the current node
    /// value unchanged, `Some(value)` overwrites it. The merged node is
    /// re-validated with the same rules enforced at node creation
    /// (`validate_workflow_node_draft`) before anything is persisted, so a
    /// patch that would introduce a new wedge is rejected instead of
    /// silently replacing one wedge with another.
    ///
    /// Passing the same `node_id` that is already present has no special
    /// case here — every call re-validates and re-persists the merged node,
    /// so repeated identical patches are idempotent.
    pub fn workflow_update_node(
        &self,
        session_id: &str,
        node_id: &str,
        patch: crate::SessionWorkflowNodePatch,
    ) -> Result<SessionRuntimeContext, SessionError> {
        let _lock = self.begin_runtime_mutation(session_id)?;
        let mut context = self.read_runtime_context(session_id)?;
        let index = context
            .workflow
            .nodes
            .iter()
            .position(|node| node.node_id == node_id)
            .ok_or_else(|| {
                SessionError::InvalidHookInput(format!(
                    "unknown workflow node id: {node_id}"
                ))
            })?;

        let mut updated = context.workflow.nodes[index].clone();
        if let Some(kind) = patch.kind {
            updated.kind = kind;
        }
        if let Some(requirement) = patch.requirement {
            updated.requirement = requirement;
        }
        if let Some(title) = patch.title {
            updated.title = title;
        }
        if let Some(ticket_urn) = patch.ticket_urn {
            updated.ticket_urn = Some(ticket_urn);
        }
        if let Some(spec_urn) = patch.spec_urn {
            updated.spec_urn = Some(spec_urn);
        }
        if let Some(anchor_urn) = patch.anchor_urn {
            updated.anchor_urn = Some(anchor_urn);
        }
        if let Some(category) = patch.category {
            updated.category = Some(category);
        }
        if let Some(cached_ticket_title) = patch.cached_ticket_title {
            updated.cached_ticket_title = Some(cached_ticket_title);
        }
        if let Some(validation_spec_id) = patch.validation_spec_id {
            updated.validation_spec_id = Some(validation_spec_id);
        }

        let draft = crate::SessionWorkflowNodeDraft {
            node_id: Some(updated.node_id.clone()),
            kind: updated.kind,
            requirement: updated.requirement,
            title: updated.title.clone(),
            ticket_urn: updated.ticket_urn.clone(),
            spec_urn: updated.spec_urn.clone(),
            anchor_urn: updated.anchor_urn.clone(),
            category: updated.category.clone(),
            cached_ticket_title: updated.cached_ticket_title.clone(),
            validation_spec_id: updated.validation_spec_id.clone(),
        };
        self.validate_workflow_node_draft(&draft)?;

        updated.updated_at = chrono::Utc::now();
        context.workflow.nodes[index] = updated;
        sort_workflow_graph(&mut context.workflow);
        context.updated_at = chrono::Utc::now();
        self.persist_runtime_state(&context)?;
        Ok(context)
    }

    /// Delete a workflow node and any edges that reference it.
    ///
    /// This is the other half of the repair surface for a wedged node: when
    /// a node cannot be repaired in place (or should never have been added),
    /// it can be removed outright instead of permanently blocking
    /// `session_finish`/`session_handoff`. Edges naming the removed node are
    /// pruned so the graph never carries a dangling reference.
    pub fn workflow_remove_node(
        &self,
        session_id: &str,
        node_id: &str,
    ) -> Result<SessionRuntimeContext, SessionError> {
        let _lock = self.begin_runtime_mutation(session_id)?;
        let mut context = self.read_runtime_context(session_id)?;
        let existed = context
            .workflow
            .nodes
            .iter()
            .any(|node| node.node_id == node_id);
        if !existed {
            return Err(SessionError::InvalidHookInput(format!(
                "unknown workflow node id: {node_id}"
            )));
        }

        context.workflow.nodes.retain(|node| node.node_id != node_id);
        context
            .workflow
            .edges
            .retain(|edge| edge.from != node_id && edge.to != node_id);
        context.updated_at = chrono::Utc::now();
        self.persist_runtime_state(&context)?;
        Ok(context)
    }

    pub fn workflow_add_edge(
        &self,
        session_id: &str,
        from: &str,
        to: &str,
        kind: SessionWorkflowEdgeKind,
    ) -> Result<SessionRuntimeContext, SessionError> {
        self.workflow_add_edges(
            session_id,
            vec![SessionWorkflowEdge {
                from: from.to_string(),
                to: to.to_string(),
                kind,
            }],
        )
    }

    pub fn workflow_add_edges(
        &self,
        session_id: &str,
        edges: Vec<SessionWorkflowEdge>,
    ) -> Result<SessionRuntimeContext, SessionError> {
        let _lock = self.begin_runtime_mutation(session_id)?;
        let mut context = self.read_runtime_context(session_id)?;
        let known = context
            .workflow
            .nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for (index, edge) in edges.iter().enumerate() {
            if !known.contains(edge.from.as_str())
                || !known.contains(edge.to.as_str())
            {
                return Err(indexed_workflow_error(
                    "edges",
                    index,
                    SessionError::InvalidHookInput(format!(
                        "cannot link unknown workflow nodes: {} -> {}; add both nodes first",
                        edge.from, edge.to
                    )),
                ));
            }
        }

        let mut changed = false;
        for edge in edges {
            if context
                .workflow
                .edges
                .iter()
                .any(|existing| existing == &edge)
            {
                continue;
            }
            context.workflow.edges.push(edge);
            changed = true;
        }
        if changed {
            sort_workflow_graph(&mut context.workflow);
            context.updated_at = chrono::Utc::now();
            self.persist_runtime_state(&context)?;
        }
        Ok(context)
    }

    pub fn workflow_promote_node_to_ticket(
        &self,
        session_id: &str,
        node_id: &str,
        ticket_urn: &str,
        cached_ticket_title: Option<String>,
    ) -> Result<SessionRuntimeContext, SessionError> {
        let parsed = parse_entity_urn(ticket_urn)?;
        if parsed.kind != SessionPinnedEntityKind::Ticket {
            return Err(SessionError::InvalidHookInput(format!(
                "promotion requires a ticket URN, got {ticket_urn}"
            )));
        }
        let _lock = self.begin_runtime_mutation(session_id)?;
        let mut context = self.read_runtime_context(session_id)?;
        let node = context
            .workflow
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
            .ok_or_else(|| {
                SessionError::InvalidHookInput(format!(
                    "unknown workflow node id: {node_id}"
                ))
            })?;

        node.kind = crate::SessionWorkflowNodeKind::Ticket;
        node.ticket_urn = Some(ticket_urn.to_string());
        if cached_ticket_title.is_some() {
            node.cached_ticket_title = cached_ticket_title;
        }
        node.updated_at = chrono::Utc::now();
        context.updated_at = node.updated_at;
        self.persist_runtime_state(&context)?;
        Ok(context)
    }

    pub fn workflow_snapshot(
        &self,
        session_id: &str,
        resolver: Option<&dyn SessionTicketStateResolver>,
    ) -> Result<SessionWorkflowSnapshot, SessionError> {
        let context = self.read_runtime_context(session_id)?;
        let mut resolutions = Vec::new();
        let mut diagnostics = Vec::new();
        let owned_resolver = resolver.is_none().then(|| self.default_ticket_state_resolver());
        let resolver = resolver.or(owned_resolver
            .as_ref()
            .map(|item| item as &dyn SessionTicketStateResolver));

        if let Some(resolver) = resolver {
            for node in &context.workflow.nodes {
                // Ticket-backed nodes resolve authoritative live ticket state.
                if let Some(ticket_urn) = node.ticket_urn.as_deref() {
                    match resolver.resolve_ticket_state(ticket_urn) {
                        Ok(state) =>
                            resolutions.push(SessionWorkflowNodeResolution {
                                node_id: node.node_id.clone(),
                                live_ticket_state: state,
                            }),
                        Err(message) =>
                            diagnostics.push(SessionWorkflowDiagnostic {
                                node_id: node.node_id.clone(),
                                code: "ticket-state-unavailable".to_string(),
                                message,
                            }),
                    }
                }

                // Spec-backed nodes resolve authoritative live spec state,
                // symmetric to ticket resolution. The `live_ticket_state` slot
                // carries the live entity state regardless of backing kind.
                if let Some(spec_urn) = node.spec_urn.as_deref() {
                    match resolver.resolve_spec_state(spec_urn) {
                        Ok(state) =>
                            resolutions.push(SessionWorkflowNodeResolution {
                                node_id: node.node_id.clone(),
                                live_ticket_state: state,
                            }),
                        Err(message) =>
                            diagnostics.push(SessionWorkflowDiagnostic {
                                node_id: node.node_id.clone(),
                                code: "spec-state-unavailable".to_string(),
                                message,
                            }),
                    }
                }
            }
        }

        Ok(SessionWorkflowSnapshot {
            workflow: context.workflow,
            resolutions,
            diagnostics,
        })
    }

    pub fn workflow_render_terminal(
        &self,
        session_id: &str,
        resolver: Option<&dyn SessionTicketStateResolver>,
    ) -> Result<String, SessionError> {
        let snapshot =
            self.workflow_snapshot(session_id, resolver)?;
        let mut lines = Vec::new();
        lines.push(format!("workflow {}", session_id));

        let live_states = snapshot
            .resolutions
            .iter()
            .map(|item| (item.node_id.clone(), item.live_ticket_state.clone()))
            .collect::<BTreeMap<_, _>>();

        let mut blockers = BTreeMap::<String, Vec<String>>::new();
        for edge in &snapshot.workflow.edges {
            if edge.kind != SessionWorkflowEdgeKind::DependsOn {
                continue;
            }
            if let Some(dependency) = snapshot
                .workflow
                .nodes
                .iter()
                .find(|node| node.node_id == edge.to)
            {
                if !node_is_effectively_done(
                    dependency,
                    live_states.get(&dependency.node_id),
                ) {
                    blockers
                        .entry(edge.from.clone())
                        .or_default()
                        .push(edge.to.clone());
                }
            }
        }

        for node in &snapshot.workflow.nodes {
            let requirement = match node.requirement {
                crate::SessionWorkflowNodeRequirement::Required => "required",
                crate::SessionWorkflowNodeRequirement::Optional => "optional",
            };
            let live_state = live_states
                .get(&node.node_id)
                .and_then(|state| state.as_deref())
                .unwrap_or("-");
            let blockers_for_node = blockers
                .get(&node.node_id)
                .cloned()
                .unwrap_or_default()
                .join(",");
            let blocker_view = if blockers_for_node.is_empty() {
                "-".to_string()
            } else {
                blockers_for_node
            };

            lines.push(format!(
                "- {} [{} {}] ticket_state={} blockers={} {}",
                node.node_id,
                requirement,
                workflow_status_label(node.status),
                live_state,
                blocker_view,
                node.title
            ));
        }

        for diag in &snapshot.diagnostics {
            lines.push(format!(
                "! {} {} {}",
                diag.node_id, diag.code, diag.message
            ));
        }

        Ok(lines.join("\n"))
    }

    pub fn workflow_render_mermaid(
        &self,
        session_id: &str,
        resolver: Option<&dyn SessionTicketStateResolver>,
    ) -> Result<String, SessionError> {
        let snapshot =
            self.workflow_snapshot(session_id, resolver)?;
        Ok(render_workflow_mermaid(
            &snapshot.workflow,
            &snapshot.resolutions,
        ))
    }
}
