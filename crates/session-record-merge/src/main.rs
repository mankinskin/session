//! Git low-level merge driver for `.session/sessions/**/session.json` and
//! `transcript.json`. These files are independently written by the
//! main-checkout mirror (`session-worktree-inference`, a thin registry stub)
//! and by the session's own worktree branch (the full `copilot-hook`
//! capture), so a textual/diff3 merge on the same session id routinely
//! conflicts even though the two sides are semantically compatible. This
//! driver performs a typed, field-aware merge instead of leaving conflict
//! markers.
//!
//! Invoked by git as: `session-record-merge %O %A %B %P`
//! (base, ours, theirs, original pathname). Git overwrites the file at the
//! `ours` path with our output and expects exit code 0 on success. Any
//! failure leaves `ours` untouched and returns a non-zero exit code so git
//! falls back to its normal conflict-marker behavior.

use std::{
    collections::BTreeMap,
    env,
    fs,
    path::Path,
    process::ExitCode,
};

use session_api::{
    PersistedSessionManifest,
    PersistedSessionTranscript,
    SessionLinks,
    SessionMetadata,
    SessionPinnedEntity,
    SessionRunLineage,
    SessionTurn,
};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let [ours_path, theirs_path, orig_path] = match args.as_slice() {
        [_base, ours, theirs] => [ours.clone(), theirs.clone(), ours.clone()],
        [_base, ours, theirs, orig] => {
            [ours.clone(), theirs.clone(), orig.clone()]
        }
        _ => {
            eprintln!(
                "session-record-merge: expected `%O %A %B [%P]`, got {} args",
                args.len()
            );
            return ExitCode::FAILURE;
        }
    };

    let ours_text = match fs::read_to_string(&ours_path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("session-record-merge: cannot read ours ({ours_path}): {error}");
            return ExitCode::FAILURE;
        }
    };
    let theirs_text = match fs::read_to_string(&theirs_path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("session-record-merge: cannot read theirs ({theirs_path}): {error}");
            return ExitCode::FAILURE;
        }
    };

    let is_transcript = Path::new(&orig_path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "transcript.json");

    let merged = if is_transcript {
        merge_transcript_json(&ours_text, &theirs_text)
    } else {
        merge_manifest_json(&ours_text, &theirs_text)
    };

    match merged {
        Ok(merged_text) => {
            if let Err(error) = fs::write(&ours_path, merged_text) {
                eprintln!(
                    "session-record-merge: cannot write merged result to {ours_path}: {error}"
                );
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(reason) => {
            eprintln!("session-record-merge: {reason}");
            ExitCode::FAILURE
        }
    }
}

fn merge_manifest_json(
    ours_text: &str,
    theirs_text: &str,
) -> Result<String, String> {
    let ours: PersistedSessionManifest = serde_json::from_str(ours_text)
        .map_err(|error| format!("ours is not a valid session.json: {error}"))?;
    let theirs: PersistedSessionManifest = serde_json::from_str(theirs_text)
        .map_err(|error| format!("theirs is not a valid session.json: {error}"))?;
    if ours.session_id != theirs.session_id {
        return Err(format!(
            "refusing to merge mismatched session ids: {} vs {}",
            ours.session_id, theirs.session_id
        ));
    }

    let merged = merge_manifest(ours, theirs);
    serde_json::to_string_pretty(&merged)
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|error| format!("failed to serialize merged session.json: {error}"))
}

fn merge_transcript_json(
    ours_text: &str,
    theirs_text: &str,
) -> Result<String, String> {
    let ours: PersistedSessionTranscript = serde_json::from_str(ours_text)
        .map_err(|error| format!("ours is not a valid transcript.json: {error}"))?;
    let theirs: PersistedSessionTranscript = serde_json::from_str(theirs_text)
        .map_err(|error| format!("theirs is not a valid transcript.json: {error}"))?;
    if ours.session_id != theirs.session_id {
        return Err(format!(
            "refusing to merge mismatched session ids: {} vs {}",
            ours.session_id, theirs.session_id
        ));
    }

    let merged = merge_transcript(ours, theirs);
    serde_json::to_string_pretty(&merged)
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|error| format!("failed to serialize merged transcript.json: {error}"))
}

/// The main-checkout mirror writes a minimal stub record (`source ==
/// "session-worktree-inference"`) purely to register the worktree
/// assignment. Whenever only one side is such a stub, the fully captured
/// side is authoritative for scalar fields; otherwise the newer capture wins.
fn is_registry_stub(record: &PersistedSessionManifest) -> bool {
    record.source == "session-worktree-inference"
}

fn merge_manifest(
    a: PersistedSessionManifest,
    b: PersistedSessionManifest,
) -> PersistedSessionManifest {
    let a_is_stub = is_registry_stub(&a);
    let b_is_stub = is_registry_stub(&b);
    let a_is_primary = match (a_is_stub, b_is_stub) {
        (true, false) => false,
        (false, true) => true,
        _ => a.captured_at >= b.captured_at,
    };
    let (primary, secondary) = if a_is_primary { (a, b) } else { (b, a) };

    let captured_at = primary.captured_at.max(secondary.captured_at);
    let schema_version = primary.schema_version.max(secondary.schema_version);
    let active_run_id = if !primary.active_run_id.is_empty() {
        primary.active_run_id.clone()
    } else {
        secondary.active_run_id.clone()
    };
    let workflow = if !primary.workflow.is_empty() {
        primary.workflow.clone()
    } else {
        secondary.workflow.clone()
    };

    PersistedSessionManifest {
        schema_version,
        session_id: primary.session_id.clone(),
        source: primary.source.clone(),
        started_at: primary.started_at,
        captured_at,
        metadata: merge_metadata(&primary.metadata, &secondary.metadata),
        links: merge_links(&primary.links, &secondary.links),
        track_id: primary.track_id.clone().or(secondary.track_id.clone()),
        anchor_ticket_id: primary
            .anchor_ticket_id
            .clone()
            .or(secondary.anchor_ticket_id.clone()),
        parent_session_id: primary
            .parent_session_id
            .clone()
            .or(secondary.parent_session_id.clone()),
        spawned_session_id: primary
            .spawned_session_id
            .clone()
            .or(secondary.spawned_session_id.clone()),
        emitted_handoff_ids: union_sorted(
            &primary.emitted_handoff_ids,
            &secondary.emitted_handoff_ids,
        ),
        picked_up_handoff_ids: union_sorted(
            &primary.picked_up_handoff_ids,
            &secondary.picked_up_handoff_ids,
        ),
        active_run_id,
        runs: merge_by_key(
            &primary.runs,
            &secondary.runs,
            |run: &SessionRunLineage| run.run_id.clone(),
        ),
        pinned_entities: merge_by_key(
            &primary.pinned_entities,
            &secondary.pinned_entities,
            |pin: &SessionPinnedEntity| pin.urn.clone(),
        ),
        workflow,
    }
}

fn merge_metadata(
    primary: &SessionMetadata,
    secondary: &SessionMetadata,
) -> SessionMetadata {
    SessionMetadata {
        workspace_slug: if !primary.workspace_slug.is_empty() {
            primary.workspace_slug.clone()
        } else {
            secondary.workspace_slug.clone()
        },
        conversation_id: primary
            .conversation_id
            .clone()
            .or(secondary.conversation_id.clone()),
        agent_id: primary.agent_id.clone().or(secondary.agent_id.clone()),
        ticket_id: primary.ticket_id.clone().or(secondary.ticket_id.clone()),
        model: primary.model.clone().or(secondary.model.clone()),
        trigger: primary.trigger.clone().or(secondary.trigger.clone()),
        provisioning: primary
            .provisioning
            .clone()
            .or(secondary.provisioning.clone()),
        producer: primary.producer.clone().or(secondary.producer.clone()),
        copilot_version: primary
            .copilot_version
            .clone()
            .or(secondary.copilot_version.clone()),
        vscode_version: primary
            .vscode_version
            .clone()
            .or(secondary.vscode_version.clone()),
        protocol_version: primary.protocol_version.or(secondary.protocol_version),
        worktree: primary.worktree.clone().or(secondary.worktree.clone()),
    }
}

fn merge_links(
    primary: &SessionLinks,
    secondary: &SessionLinks,
) -> SessionLinks {
    SessionLinks {
        ticket_ids: union_sorted(&primary.ticket_ids, &secondary.ticket_ids),
        spec_ids: union_sorted(&primary.spec_ids, &secondary.spec_ids),
        doc_evidence_ids: union_sorted(
            &primary.doc_evidence_ids,
            &secondary.doc_evidence_ids,
        ),
        log_ids: union_sorted(&primary.log_ids, &secondary.log_ids),
        runtime_session_id: primary
            .runtime_session_id
            .clone()
            .or(secondary.runtime_session_id.clone()),
        runtime_run_id: primary
            .runtime_run_id
            .clone()
            .or(secondary.runtime_run_id.clone()),
    }
}

fn merge_transcript(
    a: PersistedSessionTranscript,
    b: PersistedSessionTranscript,
) -> PersistedSessionTranscript {
    let captured_at = a.captured_at.max(b.captured_at);
    let schema_version = a.schema_version.max(b.schema_version);

    let mut by_sequence: BTreeMap<usize, SessionTurn> = BTreeMap::new();
    for turn in a.turns.into_iter().chain(b.turns) {
        by_sequence
            .entry(turn.sequence)
            .and_modify(|existing: &mut SessionTurn| {
                if turn.captured_at > existing.captured_at {
                    *existing = turn.clone();
                }
            })
            .or_insert(turn);
    }

    PersistedSessionTranscript {
        schema_version,
        session_id: a.session_id,
        captured_at,
        turns: by_sequence.into_values().collect(),
    }
}

fn union_sorted(
    a: &[String],
    b: &[String],
) -> Vec<String> {
    let mut merged: Vec<String> = a.iter().chain(b).cloned().collect();
    merged.sort();
    merged.dedup();
    merged
}

/// Union two lists keyed by `key_fn`, preferring the `primary` list's entry
/// when both sides carry the same key, ordered by key for determinism.
fn merge_by_key<T: Clone, K: Ord + Clone>(
    primary: &[T],
    secondary: &[T],
    key_fn: impl Fn(&T) -> K,
) -> Vec<T> {
    let mut by_key: BTreeMap<K, T> = BTreeMap::new();
    for item in secondary {
        by_key.insert(key_fn(item), item.clone());
    }
    for item in primary {
        by_key.insert(key_fn(item), item.clone());
    }
    by_key.into_values().collect()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use session_api::{
        SessionWorktreeAllocationMode,
        SessionWorktreeAssignment,
        SessionWorktreeStatus,
    };

    use super::*;

    fn time(hour: u32, minute: u32, second: u32) -> DateTimeUtc {
        chrono::Utc
            .with_ymd_and_hms(2026, 8, 19, hour, minute, second)
            .single()
            .unwrap()
    }

    type DateTimeUtc = chrono::DateTime<chrono::Utc>;

    fn stub_manifest(captured_at: DateTimeUtc) -> PersistedSessionManifest {
        PersistedSessionManifest {
            schema_version: 1,
            session_id: "s-1".to_string(),
            source: "session-worktree-inference".to_string(),
            started_at: captured_at,
            captured_at,
            metadata: SessionMetadata {
                workspace_slug: "default".to_string(),
                conversation_id: None,
                agent_id: None,
                ticket_id: None,
                model: None,
                trigger: Some("session-worktree-inference".to_string()),
                provisioning: None,
                producer: None,
                copilot_version: None,
                vscode_version: None,
                protocol_version: None,
                worktree: Some(SessionWorktreeAssignment {
                    path: "worktrees/s-1".into(),
                    branch: "agent/s-1/session".to_string(),
                    allocation_mode: SessionWorktreeAllocationMode::New,
                    status: SessionWorktreeStatus::Active,
                    predecessor_session_id: None,
                    predecessor_path: None,
                }),
            },
            links: SessionLinks::default(),
            track_id: None,
            anchor_ticket_id: None,
            parent_session_id: None,
            spawned_session_id: None,
            emitted_handoff_ids: Vec::new(),
            picked_up_handoff_ids: Vec::new(),
            active_run_id: String::new(),
            runs: Vec::new(),
            pinned_entities: Vec::new(),
            workflow: Default::default(),
        }
    }

    fn full_manifest(started_at: DateTimeUtc, captured_at: DateTimeUtc) -> PersistedSessionManifest {
        let mut record = stub_manifest(captured_at);
        record.source = "copilot-hook".to_string();
        record.started_at = started_at;
        record.metadata.agent_id = Some("copilot-agent".to_string());
        record.metadata.trigger = Some("Stop".to_string());
        record.metadata.copilot_version = Some("0.61.0".to_string());
        record.links.ticket_ids = vec!["ticket-a".to_string()];
        record
    }

    #[test]
    fn stub_side_defers_to_fully_captured_side() {
        let stub = stub_manifest(time(1, 35, 14));
        let full = full_manifest(time(0, 12, 23), time(1, 20, 34));

        let merged = merge_manifest(stub, full);

        assert_eq!(merged.source, "copilot-hook");
        assert_eq!(merged.metadata.copilot_version.as_deref(), Some("0.61.0"));
        // Newer captured_at wins even though it came from the stub side.
        assert_eq!(merged.captured_at, time(1, 35, 14));
        // Worktree assignment present on both sides is preserved.
        assert!(merged.metadata.worktree.is_some());
        assert_eq!(merged.links.ticket_ids, vec!["ticket-a".to_string()]);
    }

    #[test]
    fn merge_is_symmetric_regardless_of_argument_order() {
        let stub = stub_manifest(time(1, 35, 14));
        let full = full_manifest(time(0, 12, 23), time(1, 20, 34));

        let a = merge_manifest(stub.clone(), full.clone());
        let b = merge_manifest(full, stub);

        assert_eq!(a.source, b.source);
        assert_eq!(a.captured_at, b.captured_at);
        assert_eq!(a.metadata.worktree, b.metadata.worktree);
    }

    #[test]
    fn transcript_turns_union_by_sequence_preferring_newer_capture() {
        let turn = |sequence, content: &str, captured_at| SessionTurn {
            sequence,
            role: session_api::SessionRole::User,
            content: content.to_string(),
            captured_at,
            tool_name: None,
            model: None,
            event_meta: None,
        };

        let a = PersistedSessionTranscript {
            schema_version: 1,
            session_id: "s-1".to_string(),
            captured_at: time(1, 20, 0),
            turns: vec![turn(0, "hello", time(1, 19, 0))],
        };
        let b = PersistedSessionTranscript {
            schema_version: 1,
            session_id: "s-1".to_string(),
            captured_at: time(1, 35, 0),
            turns: vec![
                turn(0, "hello (revised)", time(1, 20, 0)),
                turn(1, "follow-up", time(1, 35, 0)),
            ],
        };

        let merged = merge_transcript(a, b);

        assert_eq!(merged.turns.len(), 2);
        assert_eq!(merged.turns[0].content, "hello (revised)");
        assert_eq!(merged.turns[1].content, "follow-up");
        assert_eq!(merged.captured_at, time(1, 35, 0));
    }

    #[test]
    fn manifest_json_roundtrip_rejects_mismatched_session_ids() {
        let ours = serde_json::to_string(&stub_manifest(time(1, 0, 0))).unwrap();
        let mut theirs_record = stub_manifest(time(1, 0, 0));
        theirs_record.session_id = "s-2".to_string();
        let theirs = serde_json::to_string(&theirs_record).unwrap();

        let result = merge_manifest_json(&ours, &theirs);
        assert!(result.is_err());
    }
}
