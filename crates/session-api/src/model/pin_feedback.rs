pub trait SessionPinFeedbackSink {
    fn record_pin_usage(
        &self,
        session_id: &str,
        run_id: &str,
        entity_urn: &str,
    ) -> Result<(), String>;
}
