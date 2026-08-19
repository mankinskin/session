impl SessionStoreConfig {
    /// Determine the strongest relation signal between `record` and
    /// `ticket_id`, without exceeding the requested `strength` ceiling.
    /// Cumulative/widening: strict is checked first, then linked, then
    /// mentioned (which alone requires reading the session's handoff
    /// packages from disk). Never scans `transcript.json` text.
    fn ticket_relation_signal(
        &self,
        record: &SessionRecord,
        session_dir: &Path,
        ticket_id: &str,
        strength: RelationStrength,
    ) -> Result<Option<RelationStrength>, SessionError> {
        let registry_entry = self.worktree_registry_entry(&record.session_id)?;
        if registry_entry
            .as_ref()
            .is_some_and(|entry| entry.ticket_id == ticket_id)
            || record.metadata.ticket_id.as_deref() == Some(ticket_id)
        {
            return Ok(Some(RelationStrength::Strict));
        }
        if strength == RelationStrength::Strict {
            return Ok(None);
        }

        if record.links.links_to_ticket(ticket_id) {
            return Ok(Some(RelationStrength::Linked));
        }
        if strength == RelationStrength::Linked {
            return Ok(None);
        }

        if self.session_handoffs_mention_ticket(session_dir, ticket_id)? {
            return Ok(Some(RelationStrength::Mentioned));
        }
        Ok(None)
    }

    /// Scan a session's `handoffs/*/handoff.json` records for `ticket_id` in
    /// `target_tickets`. Handoff packages are structured data, not
    /// transcript text, so this satisfies the "no transcript scanning" rule.
    fn session_handoffs_mention_ticket(
        &self,
        session_dir: &Path,
        ticket_id: &str,
    ) -> Result<bool, SessionError> {
        let handoffs_dir = session_dir.join("handoffs");
        if !handoffs_dir.exists() {
            return Ok(false);
        }

        for entry in
            fs::read_dir(&handoffs_dir).map_err(|source| SessionError::Io {
                path: handoffs_dir.clone(),
                source,
            })?
        {
            let entry = entry.map_err(|source| SessionError::Io {
                path: handoffs_dir.clone(),
                source,
            })?;
            let handoff_json_path = entry.path().join("handoff.json");
            if let Some(record) = read_json_if_exists::<SessionHandoffRecord>(
                &handoff_json_path,
            )? {
                if record
                    .target_tickets
                    .iter()
                    .any(|target_ticket| target_ticket.id == ticket_id)
                {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Query sessions related to `ticket_id` at or below the requested
    /// [`RelationStrength`] tier. Separate from [`SessionQuery`]/
    /// [`Self::query_sessions`], which this leaves unchanged.
    pub fn sessions_for_ticket(
        &self,
        ticket_id: &str,
        strength: RelationStrength,
    ) -> Result<Vec<TicketSessionMatch>, SessionError> {
        let mut matches = Vec::new();
        for entry in self.federated_sessions()? {
            let record = match entry.store.read_session(&entry.session_id) {
                Ok(record) => record,
                Err(error) => {
                    // A single corrupt/malformed session entry must never
                    // abort the whole scan; skip it but keep the failure
                    // visible.
                    eprintln!(
                        "[session-api] skipping unreadable session \
                         {} in sessions_for_ticket scan at {}: {error}",
                        entry.session_id,
                        entry.source_path.display(),
                    );
                    continue;
                }
            };
            let Some(matched_strength) = entry.store.ticket_relation_signal(
                &record,
                &entry.session_dir,
                ticket_id,
                strength,
            )?
            else {
                continue;
            };

            let registry_entry = entry.store.worktree_registry_entry(&record.session_id)?;
            let worktree = registry_entry
                .as_ref()
                .map(|entry| &entry.assignment)
                .or(record.metadata.worktree.as_ref());
            matches.push(TicketSessionMatch {
                session_id: record.session_id,
                agent_id: registry_entry
                    .as_ref()
                    .map(|entry| entry.agent_id.clone())
                    .or(record.metadata.agent_id),
                started_at: record.started_at,
                ended_at: record.captured_at,
                branch: worktree.map(|assignment| assignment.branch.clone()),
                worktree_path: worktree.map(|assignment| assignment.path.clone()),
                matched_strength,
            });
        }

        matches.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });

        Ok(matches)
    }
}
