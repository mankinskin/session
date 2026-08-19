use std::collections::BTreeMap;

use serde::{
    Deserialize,
    Serialize,
};
use serde_json::Value;

use crate::{
    PersistedSessionEvents,
    SessionRecord,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionAuditSelector {
    SessionId(String),
    Latest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAuditMetrics {
    pub turn_count: usize,
    pub assistant_turn_count: usize,
    pub empty_assistant_turn_count: usize,
    pub event_count: usize,
    pub assistant_tool_plan_count: usize,
    pub tool_execution_result_count: usize,
    pub ambiguous_sync_terminal_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAuditToolCount {
    pub tool_name: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAuditSeverity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAuditFinding {
    pub code: String,
    pub severity: SessionAuditSeverity,
    pub summary: String,
    pub evidence: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAuditReport {
    pub session_id: String,
    pub schema_version: u32,
    pub source: String,
    pub captured_at: chrono::DateTime<chrono::Utc>,
    pub workspace_slug: String,
    pub metrics: SessionAuditMetrics,
    pub top_tools: Vec<SessionAuditToolCount>,
    pub findings: Vec<SessionAuditFinding>,
}

pub fn build_session_audit_report(
    record: &SessionRecord,
    events: Option<&PersistedSessionEvents>,
) -> SessionAuditReport {
    let assistant_turn_count = record
        .turns
        .iter()
        .filter(|turn| turn.role == crate::SessionRole::Assistant)
        .count();
    let empty_assistant_turn_count = record
        .turns
        .iter()
        .filter(|turn| {
            turn.role == crate::SessionRole::Assistant
                && turn.content.trim().is_empty()
        })
        .count();

    let mut assistant_tool_plan_count = 0usize;
    let mut tool_execution_result_count = 0usize;
    let mut ambiguous_sync_terminal_count = 0usize;
    let mut tool_counts = BTreeMap::<String, usize>::new();
    let mut complete_tool_calls = std::collections::BTreeSet::<String>::new();

    if let Some(events) = events {
        for event in &events.events {
            if is_tool_execution_complete(event.event_type.as_deref()) {
                if let Some(tool_call_id) = event.tool_call_id.as_ref() {
                    complete_tool_calls.insert(tool_call_id.clone());
                }
            }
        }
    }

    if let Some(events) = events {
        for event in &events.events {
            if event.event_type.as_deref() == Some("assistant.tool_plan") {
                assistant_tool_plan_count += 1;
            }
            if is_tool_execution_outcome(event.event_type.as_deref()) {
                if is_tool_execution_result(event.event_type.as_deref())
                    && event
                        .tool_call_id
                        .as_ref()
                        .is_some_and(|id| complete_tool_calls.contains(id))
                {
                    continue;
                }
                tool_execution_result_count += 1;
                if event
                    .data_json
                    .as_ref()
                    .and_then(|value| value.get("blocker"))
                    .and_then(|value| value.as_str())
                    == Some("sync-terminal-state-ambiguous")
                {
                    ambiguous_sync_terminal_count += 1;
                }
            }

            if let Some(name) = event.tool_name.as_deref() {
                *tool_counts.entry(name.to_string()).or_insert(0) += 1;
            }
        }
    }

    let mut top_tools = tool_counts
        .into_iter()
        .map(|(tool_name, count)| SessionAuditToolCount { tool_name, count })
        .collect::<Vec<_>>();
    top_tools.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.tool_name.cmp(&right.tool_name))
    });
    top_tools.truncate(10);

    let mut findings = Vec::new();
    if empty_assistant_turn_count > 0 {
        findings.push(SessionAuditFinding {
            code: "empty-assistant-turns".to_string(),
            severity: SessionAuditSeverity::Medium,
            summary: "Persisted transcript includes empty assistant turns that add low-signal context volume.".to_string(),
            evidence: serde_json::json!({
                "empty_assistant_turn_count": empty_assistant_turn_count,
            }),
        });
    }
    if ambiguous_sync_terminal_count > 0 {
        findings.push(SessionAuditFinding {
            code: "sync-terminal-ambiguous-lifecycle".to_string(),
            severity: SessionAuditSeverity::High,
            summary: "Session events include ambiguous sync terminal lifecycle markers.".to_string(),
            evidence: serde_json::json!({
                "ambiguous_sync_terminal_count": ambiguous_sync_terminal_count,
                "expected_blocker": "sync-terminal-state-ambiguous",
            }),
        });
    }

    SessionAuditReport {
        session_id: record.session_id.clone(),
        schema_version: record.schema_version,
        source: record.source.clone(),
        captured_at: record.captured_at,
        workspace_slug: record.metadata.workspace_slug.clone(),
        metrics: SessionAuditMetrics {
            turn_count: record.turns.len(),
            assistant_turn_count,
            empty_assistant_turn_count,
            event_count: events
                .map(|value| value.events.len())
                .unwrap_or_default(),
            assistant_tool_plan_count,
            tool_execution_result_count,
            ambiguous_sync_terminal_count,
        },
        top_tools,
        findings,
    }
}

fn is_tool_execution_outcome(event_type: Option<&str>) -> bool {
    matches!(
        event_type,
        Some("tool.execution_result")
            | Some("tool_execution_result")
            | Some("tool.execution_complete")
            | Some("tool_execution_complete")
    )
}

fn is_tool_execution_complete(event_type: Option<&str>) -> bool {
    matches!(
        event_type,
        Some("tool.execution_complete") | Some("tool_execution_complete")
    )
}

fn is_tool_execution_result(event_type: Option<&str>) -> bool {
    matches!(
        event_type,
        Some("tool.execution_result") | Some("tool_execution_result")
    )
}
