#[allow(unused_imports)]
use super::*;

/// Detect structured feedback signals in the just-captured session and log a
/// compact summary for observability.
///
/// This intentionally does **not** create tickets or write feedback entries.
/// The previous implementation mined free-text with a keyword / confusion-marker
/// heuristic and auto-created tracker tickets, which produced large volumes of
/// false positives (over a hundred spurious tickets in a single run). Auto
/// synthesis is paused until (1) signals are derived only from structured
/// metadata and (2) a backtraceable, verifiable ticket format is defined.
///
/// Two failed-tool-call miners run: the turn-based
/// [`mine_structured_feedback_signals`] (kept for forward compatibility, in
/// case a future capture path populates `role: tool` turns) and the
/// event-based [`mine_failed_tool_call_signals`], which is what actually
/// fires against real captured transcripts — every committed session has
/// zero `role: tool` turns, so tool call/result metadata lives only in the
/// captured events list. The event-based miner also resolves each failure's
/// [`session_api::FailedToolCallMapping`] per the grounded policy.
///
/// `ExplicitIngestion` signals (captured `feedback_ingest` tool calls) are
/// also summarized here for observability, but are never auto-recorded: a
/// successful live call already persisted its own `FeedbackEntry`, and a
/// failed one is left for a dedicated recovery entry point
/// (`recover_feedback_entry_from_signal`) to avoid silently double-writing
/// or partially-writing feedback from a stop-hook code path.
pub(super) fn report_structured_feedback_signals(plan: &SessionStorePlan) {
    let turn_signals = if plan.record.turns.is_empty() {
        Vec::new()
    } else {
        mine_structured_feedback_signals(&plan.record.turns)
    };
    let workspace_path = plan.record.metadata.workspace_path.as_str();
    let event_failed_tool_calls = plan
        .events
        .as_ref()
        .map(|events| mine_failed_tool_call_signals(&events.events, workspace_path))
        .unwrap_or_default();
    let event_ingestions = plan
        .events
        .as_ref()
        .map(|events| mine_explicit_ingestion_signals(&events.events))
        .unwrap_or_default();

    if turn_signals.is_empty() && event_failed_tool_calls.is_empty() && event_ingestions.is_empty()
    {
        return;
    }

    let failed_tool_calls = turn_signals
        .iter()
        .chain(event_failed_tool_calls.iter())
        .filter(|signal| matches!(signal.kind, FeedbackSignalKind::FailedToolCall))
        .count();
    let explicit_ingestions = event_ingestions
        .iter()
        .filter(|signal| matches!(signal.kind, FeedbackSignalKind::ExplicitIngestion))
        .count();

    let signals: Vec<_> = turn_signals
        .iter()
        .chain(event_failed_tool_calls.iter())
        .chain(event_ingestions.iter())
        .collect();

    match serde_json::to_string(&signals) {
        Ok(json) => eprintln!(
            "[session-capture-hook] structured feedback signals for session {}: {} total ({} failed tool calls, {} explicit ingestions) {}",
            plan.record.session_id,
            signals.len(),
            failed_tool_calls,
            explicit_ingestions,
            json
        ),
        Err(error) => eprintln!(
            "[session-capture-hook] structured feedback signals for session {}: {} total ({} failed tool calls, {} explicit ingestions); summary serialization failed: {error}",
            plan.record.session_id,
            signals.len(),
            failed_tool_calls,
            explicit_ingestions
        ),
    }
}

/// Re-enable backtraceable, verifiable follow-up ticket synthesis, gated on
/// confident `ExplicitIngestion` signals only (see `session_api::follow_up`
/// module docs for the gating rationale and the idempotent-dedupe design).
/// Ticket-store errors are logged and skipped rather than failing the hook:
/// session capture must still succeed even if the ticket store is
/// unavailable.
pub(super) fn synthesize_follow_up_tickets(plan: &SessionStorePlan, cwd: Option<&Path>) {
    let Some(events) = plan.events.as_ref() else {
        return;
    };
    let ingestion_signals = mine_explicit_ingestion_signals(&events.events);
    if ingestion_signals.is_empty() {
        return;
    }

    let ticket_root = match cwd {
        Some(cwd) => memory_kernel::workspace::resolve_local_root_from(cwd, ".ticket"),
        None => PathBuf::from(".ticket"),
    };
    let ticket_store = match TicketStore::open_or_init(&ticket_root) {
        Ok(store) => store,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] follow-up synthesis skipped: failed to open ticket store at {}: {error}",
                ticket_root.display()
            );
            return;
        }
    };

    for signal in &ingestion_signals {
        let draft = match build_follow_up_ticket_draft(signal, &plan.record.session_id) {
            Ok(Some(draft)) => draft,
            Ok(None) => continue,
            Err(error) => {
                eprintln!(
                    "[session-capture-hook] follow-up draft build failed for session {}: {error}",
                    plan.record.session_id
                );
                continue;
            }
        };

        match synthesize_follow_up_ticket(&ticket_store, &draft, None) {
            Ok(FollowUpSynthesisOutcome::Created(id)) => eprintln!(
                "[session-capture-hook] synthesized follow-up ticket {id} ({})",
                draft.dedupe_key
            ),
            Ok(FollowUpSynthesisOutcome::AlreadyExists(id)) => eprintln!(
                "[session-capture-hook] follow-up ticket {id} already exists for {} (no duplicate created)",
                draft.dedupe_key
            ),
            Err(error) => eprintln!(
                "[session-capture-hook] follow-up ticket synthesis failed for {}: {error}",
                draft.dedupe_key
            ),
        }
    }
}

/// Refresh the tool metrics rollup for the store after a successful capture.
/// Best-effort: rollup write failures do NOT fail the capture.
pub(super) fn refresh_tool_metrics_rollup(config: &SessionStoreConfig) {
    let window = ToolMetricsWindow::default();
    if let Err(error) = config.write_tool_metrics_rollup(window) {
        eprintln!("[session-capture-hook] tool metrics rollup refresh failed (non-fatal): {error}");
    }
}
