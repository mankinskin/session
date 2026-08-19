//! Quality gate evaluation for delegated sessions.
//!
//! Defines pre- and post-delegation quality gates that record structured checks
//! with pass/fail/blocked outcomes. Quality gate outcomes are recorded as
//! test-api validation executions linked to the delegated session id.

use serde::{
    Deserialize,
    Serialize,
};

/// When a quality gate is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QualityGatePhase {
    /// Evaluated before delegation (precondition validation).
    PreDelegation,
    /// Evaluated after delegation completes (acceptance criteria check).
    PostDelegation,
}

/// The outcome of a quality gate evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityGateOutcome {
    /// The gate check passed.
    Passed,
    /// The gate check failed.
    Failed,
    /// The gate check was blocked (e.g., missing preconditions).
    Blocked,
}

/// A quality gate definition.
///
/// Quality gates are recorded as test-api validation executions linked to
/// the delegated session id via the `session_id` field, and to the owning
/// session id via the `ticket_ids` field (or a dedicated `parent_session_id`
/// field if added to test-api in the future).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityGate {
    /// When the gate is evaluated.
    pub phase: QualityGatePhase,
    /// The type of check being performed.
    pub check_type: String,
    /// The outcome of the gate evaluation.
    pub outcome: QualityGateOutcome,
    /// The delegated session id this gate is evaluating.
    pub delegated_session_id: String,
    /// The owning session id that spawned the delegated session.
    pub parent_session_id: String,
    /// Optional validation spec id this gate is an instance of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_spec_id: Option<String>,
    /// Optional detail explaining the outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl QualityGate {
    /// Create a new quality gate.
    pub fn new(
        phase: QualityGatePhase,
        check_type: impl Into<String>,
        outcome: QualityGateOutcome,
        delegated_session_id: impl Into<String>,
        parent_session_id: impl Into<String>,
    ) -> Self {
        Self {
            phase,
            check_type: check_type.into(),
            outcome,
            delegated_session_id: delegated_session_id.into(),
            parent_session_id: parent_session_id.into(),
            validation_spec_id: None,
            detail: None,
        }
    }

    /// Set the validation spec id for this gate.
    pub fn with_validation_spec_id(
        mut self,
        spec_id: impl Into<String>,
    ) -> Self {
        self.validation_spec_id = Some(spec_id.into());
        self
    }

    /// Set the detail for this gate.
    pub fn with_detail(
        mut self,
        detail: impl Into<String>,
    ) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Helper to construct a pre-delegation quality gate.
pub fn pre_delegation_gate(
    check_type: impl Into<String>,
    outcome: QualityGateOutcome,
    delegated_session_id: impl Into<String>,
    parent_session_id: impl Into<String>,
) -> QualityGate {
    QualityGate::new(
        QualityGatePhase::PreDelegation,
        check_type,
        outcome,
        delegated_session_id,
        parent_session_id,
    )
}

/// Helper to construct a post-delegation quality gate.
pub fn post_delegation_gate(
    check_type: impl Into<String>,
    outcome: QualityGateOutcome,
    delegated_session_id: impl Into<String>,
    parent_session_id: impl Into<String>,
) -> QualityGate {
    QualityGate::new(
        QualityGatePhase::PostDelegation,
        check_type,
        outcome,
        delegated_session_id,
        parent_session_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_gate_roundtrip_json() {
        let gate = QualityGate::new(
            QualityGatePhase::PreDelegation,
            "prompt-clarity",
            QualityGateOutcome::Passed,
            "delegated-sess-123",
            "parent-sess-456",
        )
        .with_validation_spec_id("val-pre-gate-clarity")
        .with_detail("Prompt is clear and testable");

        let json = serde_json::to_string(&gate).expect("serialize");
        let parsed: QualityGate =
            serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed, gate);
        assert_eq!(parsed.phase, QualityGatePhase::PreDelegation);
        assert_eq!(parsed.outcome, QualityGateOutcome::Passed);
        assert_eq!(parsed.delegated_session_id, "delegated-sess-123");
        assert_eq!(parsed.parent_session_id, "parent-sess-456");
    }

    #[test]
    fn pre_delegation_gate_helper() {
        let gate = pre_delegation_gate(
            "context-present",
            QualityGateOutcome::Blocked,
            "sess-abc",
            "sess-parent",
        );

        assert_eq!(gate.phase, QualityGatePhase::PreDelegation);
        assert_eq!(gate.check_type, "context-present");
        assert_eq!(gate.outcome, QualityGateOutcome::Blocked);
    }

    #[test]
    fn post_delegation_gate_helper() {
        let gate = post_delegation_gate(
            "acceptance-criteria",
            QualityGateOutcome::Failed,
            "sess-def",
            "sess-owner",
        );

        assert_eq!(gate.phase, QualityGatePhase::PostDelegation);
        assert_eq!(gate.check_type, "acceptance-criteria");
        assert_eq!(gate.outcome, QualityGateOutcome::Failed);
    }
}
