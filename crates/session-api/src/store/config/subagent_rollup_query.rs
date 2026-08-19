use std::collections::HashMap;

use crate::{
    PersistedSessionEvents,
    SubAgentRollup,
    compute_subagent_rollups_with_events,
};

impl SessionStoreConfig {
    /// Get subagent rollups for a specific workspace session.
    /// Returns a map keyed by run_id with per-sub-agent token and cost rollups.
    pub fn subagent_rollups(
        &self,
        session_id: &str,
    ) -> Result<HashMap<String, SubAgentRollup>, SessionError> {
        // Read the session record
        let record = self.read_session(session_id)?;

        // Try to load the runtime context (may not exist for non-runtime sessions)
        let context = match self.read_runtime_context(session_id) {
            Ok(ctx) => Some(ctx),
            Err(SessionError::RuntimeContextNotFound { .. }) => None,
            Err(err) => return Err(err),
        };
        let paths = self.paths_for_session_id(session_id)?;
        let events: Option<PersistedSessionEvents> =
            read_json_if_exists(&paths.events_path)?;

        // Compute rollups from transcript spans and captured lifecycle hooks.
        let rollups = compute_subagent_rollups_with_events(
            &record,
            context.as_ref(),
            events.as_ref(),
        );

        Ok(rollups)
    }
}
