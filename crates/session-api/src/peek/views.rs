use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    SessionRecord,
    SessionRole,
    SessionTurn,
};

use super::preview_line;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTurnRange {
    pub session_id: String,
    pub total_turns: usize,
    pub start: usize,
    pub end: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<SessionTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSkeletonEntry {
    pub sequence: usize,
    pub role: SessionRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub preview: String,
    pub content_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSkeleton {
    pub session_id: String,
    pub total_turns: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<SessionSkeletonEntry>,
}

pub fn peek_turn_range(
    record: &SessionRecord,
    start: usize,
    end: Option<usize>,
) -> SessionTurnRange {
    let total = record.turns.len();
    let start = start.min(total);
    let end = end.unwrap_or(total).clamp(start, total);

    SessionTurnRange {
        session_id: record.session_id.clone(),
        total_turns: total,
        start,
        end,
        turns: record.turns[start..end].to_vec(),
    }
}

pub fn peek_skeleton(
    record: &SessionRecord,
    preview_chars: usize,
) -> SessionSkeleton {
    let entries = record
        .turns
        .iter()
        .map(|turn| SessionSkeletonEntry {
            sequence: turn.sequence,
            role: turn.role.clone(),
            tool_name: turn.tool_name.clone(),
            preview: preview_line(&turn.content, preview_chars),
            content_len: turn.content.chars().count(),
        })
        .collect();

    SessionSkeleton {
        session_id: record.session_id.clone(),
        total_turns: record.turns.len(),
        entries,
    }
}
