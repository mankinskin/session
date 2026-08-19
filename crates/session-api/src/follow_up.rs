//! Backtraceable, verifiable follow-up ticket synthesis from confident
//! structured feedback signals.
//!
//! # Gating decision
//!
//! Synthesis is gated on [`FeedbackSignalKind::ExplicitIngestion`] signals
//! whose live `feedback_ingest` tool call **succeeded**
//! (`tool_success == Some(true)`) and whose recorded rating is
//! `not-helpful` or `mixed` (a `helpful` rating needs no follow-up). This is
//! deliberately the narrowest of the two signal kinds the ring produces:
//!
//! - `ExplicitIngestion` is a deliberate, explicit action (an agent or human
//!   invoked `feedback_ingest` with a specific target/rating/note) — the
//!   highest-confidence signal available.
//! - `FailedToolCall` (even when mapped to a known entity) is not
//!   necessarily "feedback" — most observed failures are transient dev-tool
//!   errors (see the `MAP` ticket's grounding), so synthesizing a ticket
//!   from every failed call would reintroduce the over-triggering failure
//!   mode this hardening effort exists to eliminate.
//!
//! Recovering a *failed* `feedback_ingest` call's `FeedbackEntry` (so the
//! intended feedback is not lost) is a separate concern handled by
//! [`crate::recover_feedback_entry_from_signal`]; that path does not
//! synthesize a ticket either, since the recovered rating/note has not yet
//! been reviewed as a live, confirmed signal in the way a successful call's
//! arguments have.
//!
//! # Idempotent dedupe
//!
//! Re-running synthesis for the same session must not create a duplicate
//! ticket. Rather than maintaining a separate dedupe index, the ticket's id
//! itself *is* the dedupe key: [`follow_up_ticket_id`] derives a
//! deterministic UUIDv5 from a stable string (`session_id` + `tool_call_id`),
//! so the same signal always maps to the same ticket id. [`synthesize_follow_up_ticket`]
//! checks whether a ticket with that id already exists before creating one.

use std::{
    collections::BTreeMap,
    path::Path,
    str::FromStr,
};

use feedback_api::{
    EntityUrn,
    FeedbackRating,
};
use ticket_api::storage::TicketStore;
use uuid::Uuid;

use crate::{
    FeedbackSignalKind,
    StructuredFeedbackSignal,
};

/// A backtraceable, verifiable follow-up ticket draft synthesized from a
/// confident structured feedback signal. Pure data; building a draft
/// performs no store writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUpTicketDraft {
    /// Stable string this draft's ticket id is deterministically derived
    /// from (see [`follow_up_ticket_id`]).
    pub dedupe_key: String,
    pub title: String,
    pub description: String,
    pub component: String,
}

/// Build a follow-up ticket draft from a structured feedback signal, per the
/// gating policy documented on this module. Returns `Ok(None)` when the
/// signal does not meet the gate (wrong kind, unsuccessful call, missing
/// arguments, or a `helpful` rating needing no follow-up) rather than
/// guessing at a draft from incomplete data.
pub fn build_follow_up_ticket_draft(
    signal: &StructuredFeedbackSignal,
    session_id: &str,
) -> Result<Option<FollowUpTicketDraft>, String> {
    if signal.kind != FeedbackSignalKind::ExplicitIngestion {
        return Ok(None);
    }
    if signal.tool_success != Some(true) {
        return Ok(None);
    }
    let Some(ingestion) = signal.ingestion.as_ref() else {
        return Ok(None);
    };
    let (Some(target_raw), Some(rating_raw)) =
        (ingestion.target.as_deref(), ingestion.rating.as_deref())
    else {
        return Ok(None);
    };

    let rating = FeedbackRating::from_str(rating_raw)?;
    if rating == FeedbackRating::Helpful {
        return Ok(None);
    }
    let target = EntityUrn::from_str(target_raw)?;

    let tool_call_id = signal
        .tool_call_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let event_id = signal
        .event_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let dedupe_key = format!("feedback-followup/{session_id}/{tool_call_id}");

    let title = format!(
        "[feedback-followup][{}] Address {rating} feedback on {target}",
        target.store()
    );
    let note = ingestion.note.as_deref().unwrap_or("(no note provided)");
    let description = format!(
        "## Motivation\nExplicit feedback was recorded against `{target}` \
         during session `{session_id}` (tool call `{tool_call_id}`).\n\n\
         ## Feedback\n- Rating: `{rating}`\n- Note: {note}\n\n\
         ## Backtrace\n- Session: `{session_id}`\n- Tool call: `{tool_call_id}`\n\
         - Event id: `{event_id}`\n- Dedupe key: `{dedupe_key}`\n\
         - FeedbackEntry: the live `feedback_ingest` call already persisted \
         its own entry for `{target}`; cross-reference it via \
         `feedback_inbox`/`entries_for(target)` filtered to this session and \
         tool call (today's `feedback_ingest` transport does not yet echo \
         back the created entry's id for direct linking here).\n\n\
         ## Verification\nRecord a validation execution (test-api) confirming \
         the reported issue is addressed before moving this ticket past \
         `in-review`.\n"
    );

    Ok(Some(FollowUpTicketDraft {
        dedupe_key,
        title,
        description,
        component: target.store().to_string(),
    }))
}

/// Deterministically derive a follow-up ticket's id from its dedupe key.
/// The same dedupe key always yields the same id, which is what makes
/// synthesis idempotent across re-runs of the same session.
pub fn follow_up_ticket_id(dedupe_key: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, dedupe_key.as_bytes())
}

/// Outcome of attempting to synthesize a follow-up ticket. Explicit rather
/// than a boolean so callers (and logs) can distinguish "created a new
/// ticket" from "this session was already processed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUpSynthesisOutcome {
    Created(Uuid),
    AlreadyExists(Uuid),
}

/// Synthesize a follow-up ticket for `draft`, idempotently. If a ticket with
/// the deterministic id derived from `draft.dedupe_key` already exists, no
/// new ticket is created and `AlreadyExists` is returned.
pub fn synthesize_follow_up_ticket(
    ticket_store: &TicketStore,
    draft: &FollowUpTicketDraft,
    target_root: Option<&Path>,
) -> Result<FollowUpSynthesisOutcome, String> {
    let id = follow_up_ticket_id(&draft.dedupe_key);

    if ticket_store
        .get_indexed(&id)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Ok(FollowUpSynthesisOutcome::AlreadyExists(id));
    }

    let mut extra = BTreeMap::new();
    extra.insert(
        "component".to_string(),
        serde_json::Value::String(draft.component.clone()),
    );
    extra.insert(
        "priority".to_string(),
        serde_json::Value::String("medium".to_string()),
    );

    ticket_store
        .create(
            Some(id),
            "tracker-improvement",
            Some(&draft.title),
            Some("open"),
            extra,
            target_root,
            Some(&draft.description),
        )
        .map_err(|error| error.to_string())?;

    Ok(FollowUpSynthesisOutcome::Created(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ExplicitIngestionArgs,
        StructuredFeedbackSignal,
    };

    fn ingestion_signal(
        tool_success: Option<bool>,
        target: &str,
        rating: &str,
        note: Option<&str>,
    ) -> StructuredFeedbackSignal {
        StructuredFeedbackSignal {
            kind: FeedbackSignalKind::ExplicitIngestion,
            sequence: None,
            tool_name: Some("mcp_rmcp5_feedback_ingest".to_string()),
            tool_call_id: Some("call-1".to_string()),
            event_id: Some("evt-1".to_string()),
            tool_success,
            ingestion: Some(ExplicitIngestionArgs {
                target: Some(target.to_string()),
                source: Some("agent".to_string()),
                rating: Some(rating.to_string()),
                note: note.map(str::to_string),
                note_kind: Some("note".to_string()),
                session_id: Some("session-1".to_string()),
                author: Some("copilot".to_string()),
            }),
            mapping: None,
        }
    }

    #[test]
    fn builds_draft_for_successful_not_helpful_ingestion() {
        let signal = ingestion_signal(
            Some(true),
            "ce://memory-api/rule/r1",
            "not-helpful",
            Some("confusing wording"),
        );

        let draft = build_follow_up_ticket_draft(&signal, "session-1")
            .unwrap()
            .expect("draft");

        assert_eq!(draft.dedupe_key, "feedback-followup/session-1/call-1");
        assert!(draft.title.contains("rule"));
        assert!(draft.description.contains("confusing wording"));
        assert_eq!(draft.component, "rule");
    }

    #[test]
    fn skips_helpful_rating() {
        let signal = ingestion_signal(
            Some(true),
            "ce://memory-api/rule/r1",
            "helpful",
            None,
        );

        assert!(
            build_follow_up_ticket_draft(&signal, "session-1")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn skips_failed_live_call() {
        // The live call did not persist; recovery (a separate concern) may
        // still record the FeedbackEntry, but synthesis does not fire here.
        let signal = ingestion_signal(
            Some(false),
            "ce://memory-api/rule/r1",
            "not-helpful",
            None,
        );

        assert!(
            build_follow_up_ticket_draft(&signal, "session-1")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn skips_non_ingestion_signal_kind() {
        let mut signal = ingestion_signal(
            Some(true),
            "ce://memory-api/rule/r1",
            "not-helpful",
            None,
        );
        signal.kind = FeedbackSignalKind::FailedToolCall;

        assert!(
            build_follow_up_ticket_draft(&signal, "session-1")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn synthesis_is_idempotent_across_reruns() {
        let dir = tempfile::tempdir().unwrap();
        let ticket_store = TicketStore::open_or_init(dir.path()).unwrap();

        let signal = ingestion_signal(
            Some(true),
            "ce://memory-api/rule/r1",
            "not-helpful",
            Some("confusing wording"),
        );
        let draft = build_follow_up_ticket_draft(&signal, "session-1")
            .unwrap()
            .unwrap();

        let first =
            synthesize_follow_up_ticket(&ticket_store, &draft, None).unwrap();
        let FollowUpSynthesisOutcome::Created(first_id) = first else {
            panic!("expected Created on first synthesis, got {first:?}");
        };

        // Re-running against the same signal/session must not duplicate.
        let second =
            synthesize_follow_up_ticket(&ticket_store, &draft, None).unwrap();
        assert_eq!(second, FollowUpSynthesisOutcome::AlreadyExists(first_id));

        let manifest = ticket_store.get(&first_id).unwrap();
        let title = manifest
            .extra
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        assert!(title.contains("rule"));
    }
}
