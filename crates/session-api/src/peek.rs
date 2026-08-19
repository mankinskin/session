/// Default number of preview characters retained per turn in a skeleton view.
pub const DEFAULT_SKELETON_PREVIEW_CHARS: usize = 120;

mod prompt_pack;

pub use prompt_pack::{
    DEFAULT_PROMPT_SUMMARIZE_THRESHOLD_CHARS,
    PromptInclusion,
    PromptPackOptions,
    SessionPromptPack,
    SessionPromptPackEntry,
    peek_prompt_pack,
};
pub use views::{
    SessionSkeleton,
    SessionSkeletonEntry,
    SessionTurnRange,
    peek_skeleton,
    peek_turn_range,
};

mod views;

/// Build a single-line, character-bounded preview of `content`.
pub(super) fn preview_line(
    content: &str,
    preview_chars: usize,
) -> String {
    let first_line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");

    let mut preview: String = first_line.chars().take(preview_chars).collect();
    if first_line.chars().count() > preview_chars {
        preview.push('\u{2026}');
    }
    preview
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::{
        SessionRecord,
        SessionRole,
        SessionTurn,
    };

    fn turn(
        sequence: usize,
        role: SessionRole,
        content: &str,
    ) -> SessionTurn {
        SessionTurn {
            sequence,
            role,
            content: content.to_string(),
            captured_at: Utc::now(),
            tool_name: None,
            model: None,
            event_meta: None,
        }
    }

    fn record_with(turns: Vec<SessionTurn>) -> SessionRecord {
        SessionRecord {
            schema_version: crate::SESSION_SCHEMA_VERSION,
            session_id: "sess-1".to_string(),
            source: "test".to_string(),
            started_at: Utc::now(),
            captured_at: Utc::now(),
            metadata: crate::SessionMetadata {
                workspace_slug: "default".to_string(),
                conversation_id: None,
                agent_id: None,
                ticket_id: None,
                model: None,
                trigger: None,
                provisioning: None,
                producer: None,
                copilot_version: None,
                vscode_version: None,
                protocol_version: None,
                worktree: None,
            },
            turns,
            links: crate::SessionLinks::default(),
            track_id: None,
            anchor_ticket_id: None,
            parent_session_id: None,
            spawned_session_id: None,
            emitted_handoff_ids: Vec::new(),
            picked_up_handoff_ids: Vec::new(),
        }
    }

    #[test]
    fn peek_range_returns_full_window_by_default() {
        let record = record_with(vec![
            turn(0, SessionRole::User, "hello"),
            turn(1, SessionRole::Assistant, "world"),
        ]);

        let range = peek_turn_range(&record, 0, None);

        assert_eq!(range.total_turns, 2);
        assert_eq!(range.start, 0);
        assert_eq!(range.end, 2);
        assert_eq!(range.turns.len(), 2);
    }

    #[test]
    fn peek_range_clamps_out_of_bounds_indices() {
        let record = record_with(vec![
            turn(0, SessionRole::User, "a"),
            turn(1, SessionRole::Assistant, "b"),
            turn(2, SessionRole::User, "c"),
        ]);

        let range = peek_turn_range(&record, 5, Some(99));

        assert_eq!(range.start, 3);
        assert_eq!(range.end, 3);
        assert!(range.turns.is_empty());
    }

    #[test]
    fn peek_range_returns_inner_slice() {
        let record = record_with(vec![
            turn(0, SessionRole::User, "a"),
            turn(1, SessionRole::Assistant, "b"),
            turn(2, SessionRole::User, "c"),
        ]);

        let range = peek_turn_range(&record, 1, Some(2));

        assert_eq!(range.start, 1);
        assert_eq!(range.end, 2);
        assert_eq!(range.turns.len(), 1);
        assert_eq!(range.turns[0].content, "b");
    }

    #[test]
    fn peek_skeleton_strips_bodies_to_first_line() {
        let record = record_with(vec![turn(
            0,
            SessionRole::Assistant,
            "\n   first meaningful line\nsecond line\n",
        )]);

        let skeleton = peek_skeleton(&record, DEFAULT_SKELETON_PREVIEW_CHARS);

        assert_eq!(skeleton.total_turns, 1);
        assert_eq!(skeleton.entries[0].preview, "first meaningful line");
    }

    #[test]
    fn peek_skeleton_truncates_long_previews() {
        let long = "x".repeat(50);
        let record = record_with(vec![turn(0, SessionRole::User, &long)]);

        let skeleton = peek_skeleton(&record, 10);

        assert_eq!(skeleton.entries[0].content_len, 50);
        assert_eq!(skeleton.entries[0].preview.chars().count(), 11); // 10 + ellipsis
        assert!(skeleton.entries[0].preview.ends_with('\u{2026}'));
    }

    #[test]
    fn prompt_pack_drops_repeated_state_checks() {
        let mut first = turn(0, SessionRole::Tool, "status output");
        first.tool_name = Some("run_in_terminal".to_string());
        let mut second = turn(1, SessionRole::Tool, "status output");
        second.tool_name = Some("run_in_terminal".to_string());
        let record = record_with(vec![first, second]);

        let pack = peek_prompt_pack(&record, PromptPackOptions::default());

        assert_eq!(pack.total_turns, 2);
        assert_eq!(pack.entries.len(), 1);
        assert_eq!(pack.dropped_turns, 1);
        assert_eq!(pack.retained_turns, 1);
    }

    #[test]
    fn prompt_pack_marks_spill_paths_as_reference_only() {
        let mut tool = turn(
            0,
            SessionRole::Tool,
            "Large tool result written to file. Use the read_file tool to access the content at: /tmp/output.txt",
        );
        tool.tool_name = Some("run_in_terminal".to_string());
        let record = record_with(vec![tool]);

        let pack = peek_prompt_pack(&record, PromptPackOptions::default());

        assert_eq!(pack.reference_only_turns, 1);
        assert_eq!(pack.entries[0].inclusion, PromptInclusion::ReferenceOnly);
        assert_eq!(
            pack.entries[0].reference_pointer.as_deref(),
            Some("/tmp/output.txt")
        );
    }

    #[test]
    fn prompt_pack_summarizes_oversized_content() {
        let record = record_with(vec![turn(
            0,
            SessionRole::Assistant,
            &"x".repeat(800),
        )]);

        let pack = peek_prompt_pack(
            &record,
            PromptPackOptions {
                preview_chars: 40,
                summarize_threshold_chars: 120,
            },
        );

        assert_eq!(pack.summarized_turns, 1);
        assert_eq!(pack.entries[0].inclusion, PromptInclusion::Summarize);
    }

    #[test]
    fn prompt_pack_drops_routine_retry_narration() {
        let record = record_with(vec![turn(
            0,
            SessionRole::Assistant,
            "I will retry the same command and check again.",
        )]);

        let pack = peek_prompt_pack(&record, PromptPackOptions::default());

        assert_eq!(pack.entries.len(), 0);
        assert_eq!(pack.dropped_turns, 1);
    }

    #[test]
    fn prompt_pack_keeps_inline_blob_as_summarize_not_reference_only() {
        let record = record_with(vec![turn(
            0,
            SessionRole::Tool,
            &format!("inline payload: {}", "x".repeat(800)),
        )]);

        let pack = peek_prompt_pack(
            &record,
            PromptPackOptions {
                preview_chars: 40,
                summarize_threshold_chars: 120,
            },
        );

        assert_eq!(pack.summarized_turns, 1);
        assert_eq!(pack.reference_only_turns, 0);
        assert_eq!(pack.entries[0].inclusion, PromptInclusion::Summarize);
    }

    #[test]
    fn prompt_pack_drops_repeated_state_check_with_normalized_variants() {
        let mut first = turn(0, SessionRole::Tool, "Status   output\n");
        first.tool_name = Some("run_in_terminal".to_string());
        let mut second = turn(1, SessionRole::Tool, " status output ");
        second.tool_name = Some("run_in_terminal".to_string());
        let record = record_with(vec![first, second]);

        let pack = peek_prompt_pack(&record, PromptPackOptions::default());

        assert_eq!(pack.entries.len(), 1);
        assert_eq!(pack.dropped_turns, 1);
    }

    #[test]
    fn prompt_pack_retains_short_progress_preamble_variants() {
        let record = record_with(vec![
            turn(
                0,
                SessionRole::Assistant,
                "I will gather context and verify ticket drift status.",
            ),
            turn(
                1,
                SessionRole::Assistant,
                "Now I am checking spec and validation anchors.",
            ),
            turn(
                2,
                SessionRole::Assistant,
                "Next I will run tests after these edits.",
            ),
            turn(
                3,
                SessionRole::Assistant,
                "Durable finding: ambiguity markers should require explicit signals.",
            ),
        ]);

        let pack = peek_prompt_pack(&record, PromptPackOptions::default());

        assert_eq!(pack.total_turns, 4);
        assert_eq!(pack.dropped_turns, 0);
        assert_eq!(pack.entries.len(), 4);
    }

    #[test]
    fn prompt_pack_enforces_measurable_compactness_ratio_for_tool_output_noise()
    {
        let record = record_with(vec![
            turn(
                0,
                SessionRole::User,
                "Harden sync ambiguity labeling and add regression coverage.",
            ),
            turn(1, SessionRole::Tool, "status output"),
            turn(2, SessionRole::Tool, "status output"),
            turn(3, SessionRole::Tool, "status output"),
            turn(
                4,
                SessionRole::Tool,
                "Large tool result written to file. Use the read_file tool to access the content at: /tmp/trace.txt",
            ),
            turn(
                5,
                SessionRole::Tool,
                &format!("inline payload: {}", "x".repeat(700)),
            ),
            turn(
                6,
                SessionRole::Assistant,
                "Durable finding: sync completions need explicit ambiguity signals.",
            ),
        ]);

        let mut record = record;
        record.turns[1].tool_name = Some("run_in_terminal".to_string());
        record.turns[2].tool_name = Some("run_in_terminal".to_string());
        record.turns[3].tool_name = Some("run_in_terminal".to_string());
        record.turns[4].tool_name = Some("run_in_terminal".to_string());
        record.turns[5].tool_name = Some("run_in_terminal".to_string());

        let pack = peek_prompt_pack(
            &record,
            PromptPackOptions {
                preview_chars: 80,
                summarize_threshold_chars: 120,
            },
        );

        let included = pack.entries.len();
        assert_eq!(pack.total_turns, 7);
        assert!(pack.dropped_turns >= 2);
        assert!(included <= 5);
        assert!(pack.reference_only_turns >= 1);
        assert!(pack.summarized_turns >= 1);
    }
}
