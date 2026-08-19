use std::collections::HashMap;

use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    SessionRecord,
    SessionRole,
    SessionTurn,
};

use super::{
    DEFAULT_SKELETON_PREVIEW_CHARS,
    preview_line,
};

/// Default content length after which turns are summarized.
pub const DEFAULT_PROMPT_SUMMARIZE_THRESHOLD_CHARS: usize = 600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PromptInclusion {
    Retain,
    Summarize,
    ReferenceOnly,
    DropFromPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPromptPackEntry {
    pub sequence: usize,
    pub role: SessionRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub inclusion: PromptInclusion,
    pub reason: String,
    pub preview: String,
    pub content_len: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_pointer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPromptPack {
    pub session_id: String,
    pub total_turns: usize,
    pub retained_turns: usize,
    pub summarized_turns: usize,
    pub reference_only_turns: usize,
    pub dropped_turns: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<SessionPromptPackEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptPackOptions {
    pub preview_chars: usize,
    pub summarize_threshold_chars: usize,
}

impl Default for PromptPackOptions {
    fn default() -> Self {
        Self {
            preview_chars: DEFAULT_SKELETON_PREVIEW_CHARS,
            summarize_threshold_chars: DEFAULT_PROMPT_SUMMARIZE_THRESHOLD_CHARS,
        }
    }
}

pub fn peek_prompt_pack(
    record: &SessionRecord,
    options: PromptPackOptions,
) -> SessionPromptPack {
    let mut entries = Vec::new();
    let mut retain = 0;
    let mut summarize = 0;
    let mut reference_only = 0;
    let mut dropped = 0;
    let mut seen_signatures = HashMap::<String, usize>::new();

    for turn in &record.turns {
        let content_len = turn.content.chars().count();
        let normalized = normalize_for_signature(&turn.content);
        let signature = format!(
            "{:?}|{}|{}",
            turn.role,
            turn.tool_name.as_deref().unwrap_or(""),
            normalized
        );

        if content_len == 0 || is_routine_retry_narration(turn, &normalized) {
            dropped += 1;
            continue;
        }
        if seen_signatures.contains_key(&signature)
            && (is_repeated_state_check(turn) || turn.role == SessionRole::Tool)
        {
            dropped += 1;
            continue;
        }
        seen_signatures.insert(signature, turn.sequence);

        let reference_pointer = extract_reference_pointer(&turn.content);
        let (inclusion, reason) = if reference_pointer.is_some() {
            (PromptInclusion::ReferenceOnly, "artifact-pointer-detected")
        } else if content_len > options.summarize_threshold_chars {
            (PromptInclusion::Summarize, "oversized-content")
        } else {
            (PromptInclusion::Retain, "durable-content")
        };
        match inclusion {
            PromptInclusion::Retain => retain += 1,
            PromptInclusion::Summarize => summarize += 1,
            PromptInclusion::ReferenceOnly => reference_only += 1,
            PromptInclusion::DropFromPrompt => dropped += 1,
        }
        entries.push(SessionPromptPackEntry {
            sequence: turn.sequence,
            role: turn.role.clone(),
            tool_name: turn.tool_name.clone(),
            inclusion,
            reason: reason.to_string(),
            preview: preview_line(&turn.content, options.preview_chars),
            content_len,
            reference_pointer,
        });
    }

    SessionPromptPack {
        session_id: record.session_id.clone(),
        total_turns: record.turns.len(),
        retained_turns: retain,
        summarized_turns: summarize,
        reference_only_turns: reference_only,
        dropped_turns: dropped,
        entries,
    }
}

fn normalize_for_signature(content: &str) -> String {
    let mut normalized = String::with_capacity(content.len().min(512));
    let mut saw_space = false;
    for character in content.chars() {
        if character.is_whitespace() {
            if !saw_space {
                normalized.push(' ');
                saw_space = true;
            }
            continue;
        }
        saw_space = false;
        normalized.push(character.to_ascii_lowercase());
        if normalized.len() >= 512 {
            break;
        }
    }
    normalized.trim().to_string()
}

fn is_repeated_state_check(turn: &SessionTurn) -> bool {
    matches!(
        turn.tool_name.as_deref(),
        Some(
            "run_in_terminal"
                | "get_terminal_output"
                | "terminal_last_command"
                | "file_search"
                | "grep_search"
                | "list_dir"
                | "get_changed_files"
                | "get_errors"
        )
    )
}

fn is_routine_retry_narration(
    turn: &SessionTurn,
    normalized_content: &str,
) -> bool {
    turn.role == SessionRole::Assistant
        && normalized_content.len() <= 220
        && [
            "retry",
            "re-run",
            "rerun",
            "try again",
            "run again",
            "checking again",
        ]
        .iter()
        .any(|marker| normalized_content.contains(marker))
}

fn extract_reference_pointer(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        for marker in ["saved to:", "content at:"] {
            if let Some((_, right)) = trimmed.split_once(marker) {
                let pointer = right.trim();
                if !pointer.is_empty() {
                    return Some(pointer.to_string());
                }
            }
        }
    }
    if content.contains("chat-session-resources") {
        return Some("chat-session-resource-pointer".to_string());
    }
    content
        .contains(".session/sessions/")
        .then(|| "session-store-pointer".to_string())
}
