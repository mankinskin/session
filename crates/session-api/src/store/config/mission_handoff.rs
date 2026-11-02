use super::*;

impl SessionStoreConfig {
    pub fn check_mission_pin_transfer(
        &self,
        source: &str,
        destination: &str,
        urn: &str,
        operation_id: uuid::Uuid,
    ) -> Result<(), SessionError> {
        let _first = self.begin_runtime_mutation(source.min(destination))?;
        if source == destination {
            return Err(SessionError::InvalidEntityUrn(
                "handoff requires distinct Sessions".into(),
            ));
        }
        let _second = self.begin_runtime_mutation(source.max(destination))?;
        self.mission_transfer_contexts(source, destination, urn, operation_id)?;
        Ok(())
    }

    fn mission_transfer_contexts(
        &self,
        source: &str,
        destination: &str,
        urn: &str,
        operation_id: uuid::Uuid,
    ) -> Result<(SessionRuntimeContext, SessionRuntimeContext), SessionError> {
        let invalid = |message: &str| SessionError::InvalidEntityUrn(message.into());
        if parse_entity_urn_kind(urn)? != SessionPinnedEntityKind::Mission
            || source == destination
            || operation_id.is_nil()
        {
            return Err(invalid(
                "handoff requires Mission URN, distinct Sessions and operation id",
            ));
        }
        self.intake_require_session(source)?;
        self.intake_require_session(destination)?;
        if self.intake_is_finished(source)? || self.intake_is_finished(destination)? {
            return Err(invalid(
                "handoff requires active source and destination Sessions",
            ));
        }
        let from = self.read_runtime_context(source)?;
        let to = self.read_runtime_context(destination)?;
        if from.active_run().is_none() || to.active_run().is_none() {
            return Err(invalid("handoff requires active runtime runs"));
        }
        let marker = format!("mission-handoff/{operation_id}");
        let existing = to.pinned_entities.iter().find(|pin| pin.urn == urn);
        if existing.is_some_and(|pin| pin.relation.as_deref() != Some(&marker)) {
            return Err(invalid("destination already has a conflicting Mission pin"));
        }
        if existing.is_none() && !from.pinned_entities.iter().any(|pin| pin.urn == urn) {
            return Err(invalid("source has no Mission pin"));
        }
        Ok((from, to))
    }

    /// Recoverable, destination-first Mission pin transfer. Both active runtime
    /// locks are held in stable order; no Ticket/worktree assignments move.
    pub fn transfer_mission_pin(
        &self,
        source: &str,
        destination: &str,
        urn: &str,
        operation_id: uuid::Uuid,
    ) -> Result<(), SessionError> {
        parse_entity_urn_kind(urn)?;
        let invalid = |message: &str| SessionError::InvalidEntityUrn(message.into());
        if source == destination || operation_id.is_nil() {
            return Err(invalid(
                "handoff requires distinct active Sessions and an operation id",
            ));
        }
        let (first, second) = if source < destination {
            (source, destination)
        } else {
            (destination, source)
        };
        let _first = self.begin_runtime_mutation(first)?;
        let _second = self.begin_runtime_mutation(second)?;
        let (mut from, mut to) =
            self.mission_transfer_contexts(source, destination, urn, operation_id)?;
        let marker = format!("mission-handoff/{operation_id}");
        let existing = to.pinned_entities.iter().find(|pin| pin.urn == urn);
        if existing.is_some_and(|pin| pin.relation.as_deref() != Some(&marker)) {
            return Err(invalid("destination already has a conflicting Mission pin"));
        }
        let source_pin = from
            .pinned_entities
            .iter()
            .find(|pin| pin.urn == urn)
            .cloned();
        if existing.is_none() {
            let mut pin = source_pin.ok_or_else(|| invalid("source has no Mission pin"))?;
            pin.relation = Some(marker);
            pin.last_used_at = chrono::Utc::now();
            to.pinned_entities.push(pin);
            to.pinned_entities.sort_by(|a, b| a.urn.cmp(&b.urn));
            to.updated_at = chrono::Utc::now();
            self.persist_runtime_state(&to)?;
        }
        if from.remove_pin(urn) {
            from.updated_at = chrono::Utc::now();
            self.persist_runtime_state(&from)?;
        }
        let from = self.read_runtime_context(source)?;
        let to = self.read_runtime_context(destination)?;
        if from.pinned_entities.iter().any(|p| p.urn == urn)
            || !to.pinned_entities.iter().any(|p| p.urn == urn)
        {
            return Err(invalid("Mission pin handoff read-back disagrees"));
        }
        Ok(())
    }
}
