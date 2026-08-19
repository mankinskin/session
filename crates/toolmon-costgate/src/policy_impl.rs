//! `CostGatePolicy`: wraps [`crate::gate::Gate`] behind the transport-agnostic
//! `toolmon_policy_api::Policy` trait, with no behavior change from the
//! pre-split direct `gate::` calls.

use serde_json::Value;
use toolmon_policy_api::{
    Decision,
    Policy,
    inject_caller_model_schema,
};

use crate::gate::Gate;

/// Wraps the cost-gate price/budget logic (`gate::Gate`) behind [`Policy`].
pub struct CostGatePolicy {
    gate: Gate,
}

impl CostGatePolicy {
    pub fn new(gate: Gate) -> Self {
        Self { gate }
    }
}

impl Policy for CostGatePolicy {
    fn on_tools_list(
        &self,
        tool: &mut Value,
    ) {
        inject_caller_model_schema(tool);
    }

    fn resolves(
        &self,
        caller_model: &str,
    ) -> bool {
        self.gate.resolves(caller_model)
    }

    fn evaluate(
        &self,
        caller_model: &str,
        tool: &str,
        grant_id: Option<&str>,
    ) -> Decision {
        self.gate.evaluate(caller_model, tool, grant_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_gate() -> Gate {
        use std::sync::atomic::{
            AtomicU64,
            Ordering,
        };
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "mcpcg-policy-fixture-{}-{}.json",
            std::process::id(),
            n
        ));
        std::fs::write(
            &path,
            r#"{"models":[{"provider_id":"anthropic","model_id":"claude-opus-4-1","output_mtok":75.0}]}"#,
        )
        .unwrap();
        let g = Gate::load(
            Path::new(&path),
            crate::gate::ModelBudgetCalibration::default(),
            None,
            None,
        )
        .unwrap();
        let _ = std::fs::remove_file(&path);
        g
    }

    /// AC: `CostGatePolicy`'s methods are reached only through `Policy`
    /// (`Box<dyn Policy>` here), not via direct `gate::` calls from callers.
    #[test]
    fn policy_trait_dispatch() {
        let policy: Box<dyn Policy> =
            Box::new(CostGatePolicy::new(test_gate()));
        assert!(policy.resolves("claude-opus-4-1"));
        assert!(!policy.resolves("totally-unknown-model"));
        match policy.evaluate("claude-opus-4-1", "read_file", None) {
            Decision::Allow
            | Decision::Delegate { .. }
            | Decision::Reject { .. } => {},
        }
        let mut tool = serde_json::json!({ "name": "x" });
        policy.on_tools_list(&mut tool);
        assert_eq!(
            tool["inputSchema"]["required"][0],
            serde_json::json!(toolmon_policy_api::CALLER_MODEL_ARG)
        );
    }
}
