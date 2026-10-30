#[allow(unused_imports)]
use super::*;

impl SessionStoreConfig {
    pub fn ingest_requester_prompt(
        &self,
        session_id: &str,
        ingress: &HostPromptIngress,
    ) -> Result<Option<RequesterDecisionReceipt>, SessionError> {
        self.ingest_requester_prompt_with_fault(session_id, ingress, None)
    }

    pub fn ingest_requester_prompt_with_fault(
        &self,
        session_id: &str,
        ingress: &HostPromptIngress,
        fault: Option<DecisionFault>,
    ) -> Result<Option<RequesterDecisionReceipt>, SessionError> {
        if ingress.hook_event_name() != USER_PROMPT_SUBMIT {
            return Err(rd_err(
                "approval_authority_unavailable",
                format!(
                    "{} is not host UserPromptSubmit ingress",
                    ingress.hook_event_name()
                ),
            ));
        }
        let Some(parsed) = parse_decision_prompt(ingress.raw_prompt()) else {
            return Ok(None);
        };
        self.rd_require_session(session_id)?;
        let challenge = self.read_requester_challenge(session_id, &parsed.challenge_id)?;
        let c = &challenge.request;
        if parsed.mission_id != c.mission_id
            || parsed.revision != c.revision
            || parsed.plan_digest != c.plan_digest
            || parsed.review_snapshot_digest != c.review_snapshot_digest
        {
            return Err(rd_err(
                "approval_stale",
                "decision does not match the challenged revision and digests",
            ));
        }
        let _lock = self.acquire_runtime_lock(session_id)?;
        self.rd_ingest_locked(session_id, &parsed, ingress, fault)
            .map(Some)
    }

    pub(in super::super) fn rd_ingest_locked(
        &self,
        session_id: &str,
        parsed: &ParsedDecision,
        ingress: &HostPromptIngress,
        fault: Option<DecisionFault>,
    ) -> Result<RequesterDecisionReceipt, SessionError> {
        let raw = ingress.raw_prompt();
        let text = String::from_utf8(raw.to_vec())
            .map_err(|_| rd_err("requester_decision_corrupt", "raw prompt is not UTF-8"))?;
        let raw_sha = intake_sha256_hex(raw);
        let cid = parsed.challenge_id.as_str();
        let cdir = self.rd_challenge_dir(session_id, cid)?;
        let ddir = cdir.join("decision");
        ensure_real_directories(&self.root, &ddir)?;
        let conflict = |what: &str| rd_err("requester_decision_conflict", what.to_string());
        let existing_intent: Option<RequesterDecisionIntent> = rd_read(&ddir.join("intent.json"))?;
        if let Some(intent) = &existing_intent {
            if intent.raw_prompt_sha256 != raw_sha || intent.kind != parsed.kind {
                return Err(conflict("challenge already has a different decision"));
            }
            if ingress
                .host_event_id()
                .is_some_and(|supplied| supplied != intent.source_event_id)
            {
                return Err(conflict("host event id differs from the recorded event"));
            }
        } else if let Some(orphan) = read_regular_file_if_exists(&ddir.join("raw.bin"))? {
            if intake_sha256_hex(&orphan) != raw_sha {
                return Err(conflict(
                    "orphan raw bytes differ from the submitted prompt",
                ));
            }
        }
        if read_regular_file_if_exists(&ddir.join("receipt.json"))?.is_some() {
            return self.rd_load_receipt(session_id, cid);
        }
        if self.intake_is_finished(session_id)? {
            return Err(SessionError::WorkspaceFinished {
                session_id: session_id.into(),
            });
        }
        let decision_id = rd_decision_id(session_id, cid);
        let intent = match existing_intent {
            Some(intent) => intent,
            None => {
                let (source_event_id, supplied) = match ingress.host_event_id() {
                    Some(host) => {
                        if host.is_empty() || host.len() > 256 || host.chars().any(char::is_control)
                        {
                            return Err(rd_err(
                                "requester_decision_conflict",
                                "host event id is malformed",
                            ));
                        }
                        (host.to_string(), true)
                    }
                    None => (format!("allocated-{}", Uuid::new_v4()), false),
                };
                self.rd_check_event_id_free(session_id, cid, &source_event_id, &text)?;
                let intent = RequesterDecisionIntent {
                    schema_version: REQUESTER_DECISION_SCHEMA_VERSION,
                    session_id: session_id.into(),
                    challenge_id: cid.into(),
                    decision_id: decision_id.clone(),
                    kind: parsed.kind,
                    mission_id: parsed.mission_id,
                    revision: parsed.revision,
                    plan_digest: parsed.plan_digest.clone(),
                    review_snapshot_digest: parsed.review_snapshot_digest.clone(),
                    raw_prompt_sha256: raw_sha.clone(),
                    source_event_id,
                    host_event_id_supplied: supplied,
                };
                rd_put_same(&self.root, &ddir, "intent.json", &intent)?;
                intent
            }
        };
        rd_fault(fault, DecisionFault::AfterIntent)?;
        if !rd_put_bytes(&self.root, &ddir, "raw.bin", raw)? {
            let stored = read_regular_file_if_exists(&ddir.join("raw.bin"))?.unwrap_or_default();
            if stored != raw {
                return Err(conflict(
                    "stored raw bytes differ from the submitted prompt",
                ));
            }
        }
        rd_fault(fault, DecisionFault::AfterRawPersisted)?;
        let source = RequesterSourceEvent {
            schema_version: REQUESTER_DECISION_SCHEMA_VERSION,
            session_id: session_id.into(),
            source_event_id: intent.source_event_id.clone(),
            host_event_id_supplied: intent.host_event_id_supplied,
            hook_event_name: USER_PROMPT_SUBMIT.into(),
            raw_prompt_sha256: raw_sha.clone(),
            challenge_id: cid.into(),
            decision_id: decision_id.clone(),
        };
        rd_put_same(&self.root, &ddir, "source_event.json", &source)?;
        rd_fault(fault, DecisionFault::AfterSourceEvent)?;
        self.rd_mirror_event(session_id, &intent.source_event_id, &text)?;
        rd_fault(fault, DecisionFault::AfterEventMirror)?;
        let receipt = RequesterDecisionReceipt {
            schema_version: REQUESTER_DECISION_SCHEMA_VERSION,
            reference: DecisionReference {
                session_id: session_id.into(),
                challenge_id: cid.into(),
                decision_id,
                source_event_id: intent.source_event_id.clone(),
            },
            kind: parsed.kind,
            mission_id: parsed.mission_id,
            revision: parsed.revision,
            plan_digest: parsed.plan_digest.clone(),
            review_snapshot_digest: parsed.review_snapshot_digest.clone(),
            raw_prompt_sha256: raw_sha,
            ingress: DecisionIngress {
                hook_event_name: USER_PROMPT_SUBMIT.into(),
                host_event_id_supplied: intent.host_event_id_supplied,
                trusted_local_host: true,
            },
            raw_path: PathBuf::new(),
        };
        let encoded =
            serde_json::to_vec_pretty(&receipt).map_err(|source| SessionError::Serialize {
                path: ddir.join("receipt.json"),
                source,
            })?;
        rd_put_bytes(&self.root, &ddir, "receipt.json", &encoded)?;
        self.rd_load_receipt(session_id, cid)
    }

    pub(in super::super) fn rd_canonical_event_state(
        &self,
        session_id: &str,
        event_id: &str,
        text: &str,
    ) -> Result<bool, SessionError> {
        let paths = self.paths_for_session_id(session_id)?;
        let Some(events) = read_json_if_exists::<PersistedSessionEvents>(&paths.events_path)?
        else {
            return Ok(false);
        };
        let Some(found) = events
            .events
            .iter()
            .find(|event| event.event_id.as_deref() == Some(event_id))
        else {
            return Ok(false);
        };
        let same = found
            .event_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case(USER_PROMPT_SUBMIT))
            && found
                .data_json
                .as_ref()
                .and_then(|data| data.get("prompt"))
                .and_then(|prompt| prompt.as_str())
                == Some(text);
        if same {
            Ok(true)
        } else {
            Err(rd_err(
                "requester_decision_conflict",
                format!("event id {event_id} already names a different canonical event"),
            ))
        }
    }

    pub(in super::super) fn rd_check_event_id_free(
        &self,
        session_id: &str,
        challenge_id: &str,
        event_id: &str,
        text: &str,
    ) -> Result<(), SessionError> {
        let root = self.rd_root_dir(session_id)?;
        match fs::read_dir(&root) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|source| SessionError::Io {
                        path: root.clone(),
                        source,
                    })?;
                    if entry.file_name() == challenge_id {
                        continue;
                    }
                    ensure_real_directories(&self.root, &entry.path())?;
                    ensure_real_directories(&self.root, &entry.path().join("decision"))?;
                    let path = entry.path().join("decision").join("intent.json");
                    if let Some(other) = rd_read::<RequesterDecisionIntent>(&path)? {
                        if other.source_event_id == event_id {
                            return Err(rd_err(
                                "requester_decision_conflict",
                                format!("event id {event_id} already belongs to another decision"),
                            ));
                        }
                    }
                }
            }
            Err(source) if source.kind() == ErrorKind::NotFound => {}
            Err(source) => return Err(SessionError::Io { path: root, source }),
        }
        self.rd_canonical_event_state(session_id, event_id, text)
            .map(|_| ())
    }

    pub(in super::super) fn rd_mirror_event(
        &self,
        session_id: &str,
        event_id: &str,
        text: &str,
    ) -> Result<(), SessionError> {
        if self.rd_canonical_event_state(session_id, event_id, text)? {
            return Ok(());
        }
        let event = CopilotHookEvent {
            event_id: Some(event_id.into()),
            parent_event_id: None,
            event_type: Some(USER_PROMPT_SUBMIT.into()),
            captured_at: None,
            turn_id: None,
            message_id: None,
            tool_call_id: None,
            tool_name: None,
            tool_success: None,
            reasoning_text: None,
            tool_requests_json: None,
            tool_arguments_json: None,
            data_json: Some(serde_json::json!({ "prompt": text })),
            raw_event_json: None,
        };
        self.persist_hook_event(session_id, event)?;
        if self.rd_canonical_event_state(session_id, event_id, text)? {
            Ok(())
        } else {
            Err(rd_err(
                "requester_decision_pending",
                "canonical event projection is not visible after persistence",
            ))
        }
    }
}
