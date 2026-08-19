impl SessionStoreConfig {
    /// Backfill ticket linkage for historical sessions using ONLY
    /// structured signals: `branch` shape, `worktree_path` shape, and
    /// handoff-package `target_tickets`, and ticket-tool transcript signals.
    /// Idempotent: an already-populated `metadata.ticket_id` is never
    /// overwritten, and a ticket id already present in `links.ticket_ids` is
    /// never duplicated. When `write` is `false` this only computes the
    /// report; no session file is touched.
    pub fn backfill_ticket_links(
        &self,
        write: bool,
    ) -> Result<SessionTicketBackfillReport, SessionError> {
        let mut report = SessionTicketBackfillReport::default();

        let ticket_store_root = self.ticket_store_root();
        let ticket_store = if ticket_store_root.exists() {
            Some(
                TicketStore::open(&ticket_store_root).map_err(|error| {
                    SessionError::InvalidHookInput(format!(
                        "ticket store unavailable at {}: {error}",
                        ticket_store_root.display()
                    ))
                })?,
            )
        } else {
            None
        };
        let known_ticket_ids = ticket_store
            .as_ref()
            .map(|store| {
                store
                    .list(None, None, None)
                    .map(|tickets| {
                        tickets
                            .into_iter()
                            .map(|ticket| ticket.id)
                            .collect::<BTreeSet<_>>()
                    })
                    .map_err(|error| {
                        SessionError::InvalidHookInput(format!(
                            "ticket store unavailable at {}: {error}",
                            ticket_store_root.display()
                        ))
                    })
            })
            .transpose()?
            .unwrap_or_default();
        let known_ticket_prefixes = known_ticket_ids
            .iter()
            .map(|ticket_id| ticket_id.simple().to_string()[..8].to_string())
            .collect::<BTreeSet<_>>();
        let mut transcript_ticket_resolution_cache = BTreeMap::new();

        for entry in self.federated_sessions()? {
            report.total_sessions += 1;
            let mut record = match entry.store.read_session(&entry.session_id) {
                Ok(record) => record,
                Err(_) => {
                    report.skipped_corrupt += 1;
                    continue;
                },
            };

            let mut changed = false;

            if record.metadata.ticket_id.is_some()
                || entry
                    .store
                    .worktree_registry_entry(&entry.session_id)?
                    .is_some()
            {
                report.already_linked_untouched += 1;
            } else if let Some(worktree) = record.metadata.worktree.clone() {
                let short_id = parse_agent_branch_short_id(&worktree.branch)
                    .or_else(|| parse_worktree_path_short_id(&worktree.path));
                if let Some(short_id) = short_id {
                    match resolve_ticket_prefix(
                        ticket_store.as_ref(),
                        &short_id,
                    ) {
                        Some(full_id) => {
                            let via_branch = parse_agent_branch_short_id(
                                &worktree.branch,
                            )
                            .is_some();
                            record.metadata.ticket_id = Some(full_id);
                            if via_branch {
                                report.linked_via_branch += 1;
                            } else {
                                report.linked_via_worktree_path += 1;
                            }
                            changed = true;
                        },
                        None => {
                            report.skipped_unresolvable_shortid += 1;
                        },
                    }
                }
            }

            for ticket_id in extract_transcript_ticket_ids(&record.turns) {
                if record.links.links_to_ticket(&ticket_id) {
                    continue;
                }
                let resolved_ticket_id = transcript_ticket_resolution_cache
                    .entry(ticket_id.clone())
                    .or_insert_with(|| {
                        if let Ok(ticket_id) = uuid::Uuid::parse_str(&ticket_id) {
                            known_ticket_ids
                                .contains(&ticket_id)
                                .then(|| ticket_id.to_string())
                        } else if known_ticket_prefixes.contains(&ticket_id) {
                            resolve_ticket_prefix(
                                ticket_store.as_ref(),
                                &ticket_id,
                            )
                        } else {
                            None
                        }
                    })
                    .clone();
                match resolved_ticket_id {
                    Some(full_id) => {
                        if !record.links.links_to_ticket(&full_id) {
                            record.links.ticket_ids.push(full_id);
                            changed = true;
                        }
                    },
                    None => {
                        report.skipped_unresolvable_shortid += 1;
                    },
                }
            }

            let handoff_targets =
                entry.store.session_handoff_target_tickets(&entry.session_dir)?;
            if !handoff_targets.is_empty() {
                report.handoff_already_at_mentioned = true;
            }
            for target in handoff_targets {
                if record.links.links_to_ticket(&target) {
                    report.already_linked_untouched += 1;
                    continue;
                }
                match resolve_ticket_prefix(ticket_store.as_ref(), &target) {
                    Some(full_id) => {
                        if !record.links.links_to_ticket(&full_id) {
                            record.links.ticket_ids.push(full_id);
                            report.linked_via_handoff += 1;
                            changed = true;
                        }
                    },
                    None => {
                        report.skipped_unresolvable_shortid += 1;
                    },
                }
            }

            record.links.ticket_ids.sort();
            record.links.ticket_ids.dedup();

            if changed {
                report.total_would_link += 1;
                if write {
                    entry.store.persist_record(record)?;
                }
            }
        }

        Ok(report)
    }

    /// Collect the deduplicated union of `target_tickets` across every
    /// `handoffs/*/handoff.json` on disk for one session. Structured-data
    /// read only, same source `sessions_for_ticket`'s mentioned tier already
    /// scans; never touches `transcript.json`.
    fn session_handoff_target_tickets(
        &self,
        session_dir: &Path,
    ) -> Result<BTreeSet<String>, SessionError> {
        let handoffs_dir = session_dir.join("handoffs");
        let mut targets = BTreeSet::new();
        if !handoffs_dir.exists() {
            return Ok(targets);
        }

        for entry in
            fs::read_dir(&handoffs_dir).map_err(|source| SessionError::Io {
                path: handoffs_dir.clone(),
                source,
            })?
        {
            let entry = entry.map_err(|source| SessionError::Io {
                path: handoffs_dir.clone(),
                source,
            })?;
            let handoff_json_path = entry.path().join("handoff.json");
            if let Some(record) = read_json_if_exists::<SessionHandoffRecord>(
                &handoff_json_path,
            )? {
                targets.extend(record.target_tickets.into_iter().map(|ticket| ticket.id));
            }
        }

        Ok(targets)
    }
}

/// Parses the 8-hex-char short id out of an `agent/<short-id>-<slug>`
/// branch name. Returns `None` for any other shape.
fn parse_agent_branch_short_id(branch: &str) -> Option<String> {
    let rest = branch.strip_prefix("agent/")?;
    short_id_prefix(rest)
}

/// Parses the 8-hex-char short id out of a `.worktrees/<short-id>-<slug>`
/// path component. Returns `None` when no `.worktrees` component is present
/// or the following component does not match the shape.
fn parse_worktree_path_short_id(path: &Path) -> Option<String> {
    let mut components = path.components();
    while let Some(component) = components.next() {
        if component.as_os_str() == ".worktrees" {
            let next = components.next()?;
            let name = next.as_os_str().to_str()?;
            return short_id_prefix(name);
        }
    }
    None
}

/// Shared `<8-hex-chars>-<rest>` shape check used by both the branch and
/// worktree-path parsers.
fn short_id_prefix(candidate: &str) -> Option<String> {
    let bytes = candidate.as_bytes();
    if bytes.len() < 9 || bytes[8] != b'-' {
        return None;
    }
    let prefix = &candidate[..8];
    if prefix.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(prefix.to_ascii_lowercase())
    } else {
        None
    }
}

/// Resolves and verifies `prefix` (short id or full ticket id) against the
/// real ticket store. Returns `None` (never writes a guess) when the store
/// is unavailable, the prefix does not resolve, or it is ambiguous.
fn resolve_ticket_prefix(
    store: Option<&TicketStore>,
    prefix: &str,
) -> Option<String> {
    let store = store?;
    if let Ok(ticket_id) = uuid::Uuid::parse_str(prefix) {
        return store
            .list(None, None, None)
            .ok()?
            .into_iter()
            .any(|ticket| ticket.id == ticket_id)
            .then(|| ticket_id.to_string());
    }
    resolve_uuid_with_prefix(store, prefix)
        .ok()
        .map(|ticket_id| ticket_id.to_string())
}

fn extract_transcript_ticket_ids(turns: &[SessionTurn]) -> BTreeSet<String> {
    let mut ticket_ids = BTreeSet::new();
    for turn in turns {
        let Some(event_meta) = turn.event_meta.as_ref() else {
            continue;
        };
        let Some(requests) = event_meta
            .tool_requests_json
            .as_ref()
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for request in requests {
            let Some(arguments) = request
                .as_object()
                .and_then(|request| request.get("arguments"))
            else {
                continue;
            };
            collect_ticket_id_candidates(arguments, &mut ticket_ids);
        }
    }

    ticket_ids
}

fn collect_ticket_id_candidates(
    value: &serde_json::Value,
    ticket_ids: &mut BTreeSet<String>,
) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                collect_ticket_id_candidates(value, ticket_ids);
            }
        },
        serde_json::Value::Object(object) => {
            for value in object.values() {
                collect_ticket_id_candidates(value, ticket_ids);
            }
        },
        serde_json::Value::String(value) => {
            if uuid::Uuid::parse_str(value).is_ok() {
                ticket_ids.insert(value.to_ascii_lowercase());
            } else {
                collect_short_id_candidates(value, ticket_ids);
            }
            if matches!(value.as_bytes().first(), Some(b'{' | b'['))
                && let Ok(parsed) = serde_json::from_str(value)
            {
                collect_ticket_id_candidates(&parsed, ticket_ids);
            }
        },
        _ => {},
    }
}

fn collect_short_id_candidates(value: &str, ticket_ids: &mut BTreeSet<String>) {
    let bytes = value.as_bytes();
    for start in 0..bytes.len().saturating_sub(7) {
        let end = start + 8;
        if bytes[start..end].iter().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            && is_non_word_byte(bytes.get(start.wrapping_sub(1)))
            && is_non_word_byte(bytes.get(end))
        {
            ticket_ids.insert(value[start..end].to_string());
        }
    }
}

fn is_non_word_byte(byte: Option<&u8>) -> bool {
    byte.is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
}

