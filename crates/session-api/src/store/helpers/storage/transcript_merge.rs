#[allow(unused_imports)]
use super::*;

pub(in super::super::super) fn merge_events(
    existing: Option<PersistedSessionEvents>,
    incoming: Option<PersistedSessionEvents>,
    session_id: String,
    captured_at: chrono::DateTime<chrono::Utc>,
) -> Result<Option<PersistedSessionEvents>, SessionError> {
    match (existing, incoming) {
        (None, None) => Ok(None),
        (Some(existing), None) => Ok(Some(existing)),
        (None, Some(mut incoming)) => {
            incoming.events = canonicalize_captured_events(incoming.events);
            Ok(Some(incoming))
        }
        (Some(mut existing), Some(incoming)) => {
            if existing.session_id != incoming.session_id {
                return Err(SessionError::TranscriptConflict {
                    session_id: incoming.session_id,
                    existing_turns: existing.events.len(),
                    incoming_turns: incoming.events.len(),
                });
            }

            existing.events = canonicalize_captured_events(existing.events);
            let incoming_events = canonicalize_captured_events(incoming.events);

            let mut known = std::collections::BTreeSet::new();
            for event in &existing.events {
                known.insert(captured_event_key(event));
            }
            for event in incoming_events {
                let key = captured_event_key(&event);
                if known.insert(key) {
                    existing.events.push(event);
                }
            }

            existing.session_id = session_id;
            if captured_at > existing.captured_at {
                existing.captured_at = captured_at;
            }

            Ok(Some(existing))
        }
    }
}

pub(in super::super::super) fn merge_transcript(
    existing: Option<PersistedSessionTranscript>,
    incoming: PersistedSessionTranscript,
) -> Result<PersistedSessionTranscript, SessionError> {
    match existing {
        None => Ok(incoming),
        Some(mut existing) => {
            if existing.session_id != incoming.session_id {
                return Err(SessionError::TranscriptConflict {
                    session_id: incoming.session_id,
                    existing_turns: existing.turns.len(),
                    incoming_turns: incoming.turns.len(),
                });
            }

            let shared_prefix_len = existing
                .turns
                .iter()
                .zip(&incoming.turns)
                .take_while(|(left, right)| turns_match(left, right))
                .count();

            if shared_prefix_len < existing.turns.len() && shared_prefix_len < incoming.turns.len()
            {
                // Hook captures are periodic snapshots; when histories diverge,
                // keep the newest complete snapshot instead of rejecting sync.
                if incoming.turns.len() >= existing.turns.len() {
                    return Ok(incoming);
                }
                return Ok(existing);
            }

            if incoming.turns.len() > existing.turns.len() {
                existing
                    .turns
                    .extend(incoming.turns.into_iter().skip(existing.turns.len()));
            }

            if incoming.captured_at > existing.captured_at {
                existing.captured_at = incoming.captured_at;
            }

            Ok(existing)
        }
    }
}

pub(in super::super::super) fn turns_match(left: &SessionTurn, right: &SessionTurn) -> bool {
    left.sequence == right.sequence
        && left.role == right.role
        && left.content == right.content
        && left.tool_name == right.tool_name
        && left.event_meta == right.event_meta
}
