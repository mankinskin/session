use crate::{
    SessionError,
    ToolMetricsReport,
    ToolMetricsWindow,
    aggregate_with_cost,
    compute_session_summary_with_events,
    write_rollup,
    tool_metrics::{CharsPerTokenEstimator, GradedCostCalibration, SessionToolMetricsSummary},
};
use std::path::PathBuf;

impl SessionStoreConfig {
    /// Get aggregate tool metrics for sessions in this store, respecting the window.
    pub fn tool_metrics(
        &self,
        window: ToolMetricsWindow,
    ) -> Result<ToolMetricsReport, SessionError> {
        let mut summaries = Vec::new();
        for entry in self.federated_sessions()? {
            let summary = match entry
                .store
                .load_or_compute_tool_metrics_summary(&entry.session_id)
            {
                Ok(summary) => summary,
                Err(error) => {
                    eprintln!(
                        "[session-api] skipping unreadable session {} in tool metrics scan at {}: {error}",
                        entry.session_id,
                        entry.source_path.display()
                    );
                    continue;
                },
            };
            summaries.push(summary);
        }

        let estimator = CharsPerTokenEstimator::default();
        let cal = GradedCostCalibration::default();
        Ok(aggregate_with_cost(summaries, window, &estimator, Some(cal)))
    }

    /// Write tool metrics rollup to the canonical location.
    pub fn write_tool_metrics_rollup(
        &self,
        window: ToolMetricsWindow,
    ) -> Result<(), SessionError> {
        let report = self.tool_metrics(window)?;
        let rollup_path = self.root.join("tool-metrics-rollup.json");
        write_rollup(&rollup_path, report)
    }

    fn load_or_compute_tool_metrics_summary(
        &self,
        session_id: &str,
    ) -> Result<SessionToolMetricsSummary, SessionError> {
        let tool_metrics_path = self.tool_metrics_path(session_id)?;

        // Reuse a cached summary only when it actually holds tool data. An
        // empty cached summary predates event-based capture, so recompute it.
        if let Some(summary) =
            read_json_if_exists::<SessionToolMetricsSummary>(&tool_metrics_path)?
        {
            if !summary.is_empty() {
                return Ok(summary);
            }
        }

        // Compute from the transcript plus the captured event stream, which is
        // where tool call telemetry actually lives.
        let record = self.read_session(session_id)?;
        let paths = self.paths_for_session_id(session_id)?;
        let events: Option<crate::PersistedSessionEvents> =
            read_json_if_exists(&paths.events_path)?;
        let estimator = CharsPerTokenEstimator::default();
        let summary = compute_session_summary_with_events(
            &record,
            events
                .as_ref()
                .map(|events| events.events.as_slice())
                .unwrap_or_default(),
            &estimator,
        );

        // Persist lazily: never leave an empty sidecar behind.
        if summary.is_empty() {
            remove_file_if_exists(&tool_metrics_path)?;
        } else {
            write_json(&tool_metrics_path, &summary)?;
        }

        Ok(summary)
    }

    fn tool_metrics_path(&self, session_id: &str) -> Result<PathBuf, SessionError> {
        let paths = self.paths_for_session_id(session_id)?;
        Ok(paths.session_dir.join("tool-metrics.json"))
    }
}
