use crate::requester_decision::{
    DecisionFault, DecisionIngress, DecisionKind, DecisionReference, HostPromptIngress,
    ParsedDecision, PendingRequesterDecision, REQUESTER_DECISION_SCHEMA_VERSION,
    RequesterChallenge, RequesterChallengeRequest, RequesterConsumption, RequesterDecisionIntent,
    RequesterDecisionReceipt, RequesterSourceEvent, USER_PROMPT_SUBMIT, parse_decision_prompt,
    requester_lower_hex64, requester_valid_id,
};

const REQUESTER_DIR: &str = "requester";
fn rd_err(code: &'static str, message: impl Into<String>) -> SessionError {
    SessionError::RequesterDecision {
        code,
        message: message.into(),
    }
}
fn rd_read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, SessionError> {
    match read_regular_file_if_exists(path)? {
        None => Ok(None),
        Some(bytes) => {
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|source| SessionError::Deserialize {
                    path: path.to_path_buf(),
                    source,
                })
        }
    }
}
fn rd_put_bytes(root: &Path, dir: &Path, name: &str, bytes: &[u8]) -> Result<bool, SessionError> {
    ensure_real_directories(root, dir)?;
    fs::create_dir_all(dir).map_err(|source| SessionError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    ensure_real_directories(root, dir)?;
    write_new_synced(&dir.join(name), bytes)
}
fn rd_put_same<T: Serialize + DeserializeOwned + PartialEq>(
    root: &Path,
    dir: &Path,
    name: &str,
    value: &T,
) -> Result<(), SessionError> {
    let path = dir.join(name);
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| SessionError::Serialize {
        path: path.clone(),
        source,
    })?;
    if rd_put_bytes(root, dir, name, &bytes)? {
        return Ok(());
    }
    match rd_read::<T>(&path)? {
        Some(existing) if &existing == value => Ok(()),
        _ => Err(rd_err(
            "requester_decision_conflict",
            format!("{} already holds a different record", path.display()),
        )),
    }
}
fn rd_fault(configured: Option<DecisionFault>, point: DecisionFault) -> Result<(), SessionError> {
    if configured == Some(point) {
        return Err(rd_err(
            "requester_decision_pending",
            format!("interrupted after {point:?}; redeliver the prompt to reconcile"),
        ));
    }
    Ok(())
}
fn rd_verify_records(
    ddir: &Path,
    session_id: &str,
    challenge_id: &str,
    decision_id: &str,
    challenge: &RequesterChallenge,
    receipt: &RequesterDecisionReceipt,
) -> Result<(), SessionError> {
    let corrupt = |what: &str| rd_err("requester_decision_corrupt", what.to_string());
    let source: RequesterSourceEvent = rd_read(&ddir.join("source_event.json"))?
        .ok_or_else(|| corrupt("persisted source event is missing"))?;
    let intent: RequesterDecisionIntent =
        rd_read(&ddir.join("intent.json"))?.ok_or_else(|| corrupt("decision intent is missing"))?;
    if source.schema_version != REQUESTER_DECISION_SCHEMA_VERSION
        || intent.schema_version != REQUESTER_DECISION_SCHEMA_VERSION
    {
        return Err(corrupt("record schema version is unsupported"));
    }
    let c = &challenge.request;
    if intent.session_id != session_id
        || intent.challenge_id != challenge_id
        || intent.decision_id != decision_id
        || intent.kind != receipt.kind
        || intent.mission_id != c.mission_id
        || intent.revision != c.revision
        || intent.plan_digest != c.plan_digest
        || intent.review_snapshot_digest != c.review_snapshot_digest
        || intent.mission_id != receipt.mission_id
        || intent.revision != receipt.revision
        || intent.plan_digest != receipt.plan_digest
        || intent.review_snapshot_digest != receipt.review_snapshot_digest
        || intent.raw_prompt_sha256 != receipt.raw_prompt_sha256
        || intent.source_event_id != receipt.reference.source_event_id
        || intent.host_event_id_supplied != receipt.ingress.host_event_id_supplied
        || intent.host_event_id_supplied != source.host_event_id_supplied
    {
        return Err(corrupt(
            "intent record disagrees with the challenge, receipt or source event",
        ));
    }
    Ok(())
}

fn rd_decision_id(session_id: &str, challenge_id: &str) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("requester-decision|{session_id}|{challenge_id}").as_bytes(),
    )
    .to_string()
}

impl SessionStoreConfig {
    /// Finished Sessions retain historical decisions but cannot grant new entry.
    pub fn requester_session_finished(&self, session_id: &str) -> Result<bool, SessionError> {
        self.intake_is_finished(session_id)
    }
    fn rd_root_dir(&self, session_id: &str) -> Result<PathBuf, SessionError> {
        let dir = self
            .runtime_paths_for_workspace(session_id)?
            .workspace_dir
            .join(REQUESTER_DIR);
        ensure_real_directories(&self.root, &dir)?;
        Ok(dir)
    }
    fn rd_challenge_dir(
        &self,
        session_id: &str,
        challenge_id: &str,
    ) -> Result<PathBuf, SessionError> {
        if !requester_valid_id(challenge_id) {
            return Err(rd_err(
                "requester_challenge_invalid",
                format!("invalid challenge id {challenge_id:?}"),
            ));
        }
        let dir = self.rd_root_dir(session_id)?.join(challenge_id);
        ensure_real_directories(&self.root, &dir)?;
        Ok(dir)
    }
    fn rd_require_session(&self, session_id: &str) -> Result<(), SessionError> {
        match self.intake_require_session(session_id) {
            Err(SessionError::IntakeSessionNotFound { .. }) => Err(rd_err(
                "approval_authority_unavailable",
                format!("session {session_id} is not initialized"),
            )),
            other => other,
        }
    }
    pub fn register_requester_challenge(
        &self,
        request: &RequesterChallengeRequest,
    ) -> Result<RequesterChallenge, SessionError> {
        if !requester_valid_id(&request.challenge_id)
            || !requester_lower_hex64(&request.plan_digest)
            || !requester_lower_hex64(&request.review_snapshot_digest)
            || !request.mission_root.is_absolute()
        {
            return Err(rd_err(
                "requester_challenge_invalid",
                "challenge id, digests or mission root are malformed",
            ));
        }
        self.rd_require_session(&request.session_id)?;
        let _lock = self.acquire_runtime_lock(&request.session_id)?;
        let dir = self.rd_challenge_dir(&request.session_id, &request.challenge_id)?;
        let record = RequesterChallenge {
            schema_version: REQUESTER_DECISION_SCHEMA_VERSION,
            session_root: self.root.clone(),
            request: request.clone(),
        };
        if let Some(existing) = rd_read::<RequesterChallenge>(&dir.join("challenge.json"))? {
            return if existing == record {
                Ok(existing)
            } else {
                Err(rd_err(
                    "requester_challenge_conflict",
                    format!(
                        "challenge {} already bound differently",
                        request.challenge_id
                    ),
                ))
            };
        }
        if self.intake_is_finished(&request.session_id)? {
            return Err(SessionError::WorkspaceFinished {
                session_id: request.session_id.clone(),
            });
        }
        rd_put_same(&self.root, &dir, "challenge.json", &record)?;
        Ok(record)
    }
    pub fn read_requester_challenge(
        &self,
        session_id: &str,
        challenge_id: &str,
    ) -> Result<RequesterChallenge, SessionError> {
        let dir = self.rd_challenge_dir(session_id, challenge_id)?;
        let Some(challenge) = rd_read::<RequesterChallenge>(&dir.join("challenge.json"))? else {
            return Err(rd_err(
                "approval_authority_unavailable",
                format!("no persisted challenge {challenge_id} in session {session_id}"),
            ));
        };
        if challenge.schema_version != REQUESTER_DECISION_SCHEMA_VERSION
            || challenge.request.session_id != session_id
            || challenge.request.challenge_id != challenge_id
            || challenge.session_root != self.root
        {
            return Err(rd_err(
                "requester_decision_corrupt",
                "challenge record identity does not match its location",
            ));
        }
        Ok(challenge)
    }
    pub fn render_decision_prompt(
        &self,
        challenge: &RequesterChallenge,
        kind: DecisionKind,
    ) -> String {
        let c = &challenge.request;
        format!(
            "mission-decision {} {} {} {} {} {}",
            kind.as_str(),
            c.mission_id,
            c.revision,
            c.plan_digest,
            c.review_snapshot_digest,
            c.challenge_id
        )
    }
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
    fn rd_ingest_locked(
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
    fn rd_canonical_event_state(
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
    fn rd_check_event_id_free(
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
    fn rd_mirror_event(
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
    fn rd_load_receipt(
        &self,
        session_id: &str,
        challenge_id: &str,
    ) -> Result<RequesterDecisionReceipt, SessionError> {
        let corrupt = |what: &str| rd_err("requester_decision_corrupt", what.to_string());
        let challenge = self.read_requester_challenge(session_id, challenge_id)?;
        let ddir = self
            .rd_challenge_dir(session_id, challenge_id)?
            .join("decision");
        ensure_real_directories(&self.root, &ddir)?;
        let Some(mut receipt) = rd_read::<RequesterDecisionReceipt>(&ddir.join("receipt.json"))?
        else {
            return Err(rd_err(
                "requester_decision_pending",
                format!("no completed receipt for challenge {challenge_id}"),
            ));
        };
        let decision_id = rd_decision_id(session_id, challenge_id);
        let r = &receipt.reference;
        if receipt.schema_version != REQUESTER_DECISION_SCHEMA_VERSION
            || r.session_id != session_id
            || r.challenge_id != challenge_id
            || r.decision_id != decision_id
        {
            return Err(corrupt("receipt identity does not match its location"));
        }
        let raw = read_regular_file_if_exists(&ddir.join("raw.bin"))?
            .ok_or_else(|| corrupt("raw prompt bytes are missing"))?;
        if intake_sha256_hex(&raw) != receipt.raw_prompt_sha256 {
            return Err(corrupt("raw prompt bytes do not match the receipt hash"));
        }
        let parsed = parse_decision_prompt(&raw)
            .ok_or_else(|| corrupt("raw prompt is not a complete decision line"))?;
        let c = &challenge.request;
        if parsed.kind != receipt.kind
            || parsed.mission_id != receipt.mission_id
            || parsed.revision != receipt.revision
            || parsed.plan_digest != receipt.plan_digest
            || parsed.review_snapshot_digest != receipt.review_snapshot_digest
            || parsed.challenge_id != challenge_id
            || parsed.mission_id != c.mission_id
            || parsed.revision != c.revision
            || parsed.plan_digest != c.plan_digest
            || parsed.review_snapshot_digest != c.review_snapshot_digest
        {
            return Err(corrupt(
                "receipt binding differs from raw bytes or challenge",
            ));
        }
        let source = rd_read::<RequesterSourceEvent>(&ddir.join("source_event.json"))?
            .ok_or_else(|| corrupt("persisted source event is missing"))?;
        if source.session_id != session_id
            || source.source_event_id != receipt.reference.source_event_id
            || source.raw_prompt_sha256 != receipt.raw_prompt_sha256
            || source.hook_event_name != USER_PROMPT_SUBMIT
            || source.challenge_id != challenge_id
            || source.decision_id != decision_id
            || source.host_event_id_supplied != receipt.ingress.host_event_id_supplied
            || !receipt.ingress.trusted_local_host
            || receipt.ingress.hook_event_name != USER_PROMPT_SUBMIT
        {
            return Err(corrupt("source event does not match the receipt"));
        }
        let intent = rd_read::<RequesterDecisionIntent>(&ddir.join("intent.json"))?
            .ok_or_else(|| corrupt("decision intent is missing"))?;
        if intent.source_event_id != source.source_event_id
            || intent.raw_prompt_sha256 != receipt.raw_prompt_sha256
            || intent.kind != receipt.kind
            || intent.decision_id != decision_id
        {
            return Err(corrupt("intent does not match the receipt"));
        }
        rd_verify_records(
            &ddir,
            session_id,
            challenge_id,
            &decision_id,
            &challenge,
            &receipt,
        )?;
        receipt.raw_path = ddir.join("raw.bin");
        Ok(receipt)
    }
    pub fn read_requester_decision(
        &self,
        reference: &DecisionReference,
    ) -> Result<RequesterDecisionReceipt, SessionError> {
        if reference.decision_id != rd_decision_id(&reference.session_id, &reference.challenge_id) {
            return Err(rd_err(
                "requester_decision_unknown_reference",
                "the decision reference does not name this challenge's decision",
            ));
        }
        let receipt = self.rd_load_receipt(&reference.session_id, &reference.challenge_id)?;
        if &receipt.reference != reference {
            return Err(rd_err(
                "requester_decision_conflict",
                "reference does not match the stored decision",
            ));
        }
        Ok(receipt)
    }
    pub fn find_requester_decision_for_challenge(
        &self,
        session_id: &str,
        challenge_id: &str,
    ) -> Result<Option<RequesterDecisionReceipt>, SessionError> {
        let ddir = self
            .rd_challenge_dir(session_id, challenge_id)?
            .join("decision");
        ensure_real_directories(&self.root, &ddir)?;
        if read_regular_file_if_exists(&ddir.join("receipt.json"))?.is_none() {
            return Ok(None);
        }
        self.rd_load_receipt(session_id, challenge_id).map(Some)
    }
    pub fn pending_requester_decisions(
        &self,
        session_id: &str,
    ) -> Result<Vec<PendingRequesterDecision>, SessionError> {
        let root = self.rd_root_dir(session_id)?;
        let mut pending = Vec::new();
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(pending),
            Err(source) => return Err(SessionError::Io { path: root, source }),
        };
        for entry in entries {
            let entry = entry.map_err(|source| SessionError::Io {
                path: root.clone(),
                source,
            })?;
            ensure_real_directories(&self.root, &entry.path())?;
            let ddir = entry.path().join("decision");
            match fs::symlink_metadata(&ddir) {
                Ok(_) => {}
                Err(source) if source.kind() == ErrorKind::NotFound => continue,
                Err(source) => return Err(SessionError::Io { path: ddir, source }),
            }
            ensure_real_directories(&self.root, &ddir)?;
            if read_regular_file_if_exists(&ddir.join("receipt.json"))?.is_some() {
                continue;
            }
            let has = |name: &str| ddir.join(name).is_file();
            let stage = if has("source_event.json") {
                "source_event"
            } else if has("raw.bin") {
                "raw"
            } else if has("intent.json") {
                "intent"
            } else {
                "begun"
            };
            pending.push(PendingRequesterDecision {
                challenge_id: entry.file_name().to_string_lossy().into_owned(),
                stage: stage.into(),
            });
        }
        pending.sort_by(|a, b| a.challenge_id.cmp(&b.challenge_id));
        Ok(pending)
    }
    pub fn consume_requester_challenge(
        &self,
        reference: &DecisionReference,
        grant_id: &str,
    ) -> Result<RequesterConsumption, SessionError> {
        if !requester_valid_id(grant_id) {
            return Err(rd_err(
                "requester_decision_conflict",
                "grant id is malformed",
            ));
        }
        let _lock = self.acquire_runtime_lock(&reference.session_id)?;
        self.read_requester_decision(reference)?;
        let cdir = self.rd_challenge_dir(&reference.session_id, &reference.challenge_id)?;
        let record = RequesterConsumption {
            reference: reference.clone(),
            grant_id: grant_id.into(),
        };
        if let Some(existing) = rd_read::<RequesterConsumption>(&cdir.join("consumed.json"))? {
            return if existing == record {
                Ok(existing)
            } else {
                Err(rd_err(
                    "requester_decision_conflict",
                    "challenge was already consumed by a different grant",
                ))
            };
        }
        if self.intake_is_finished(&reference.session_id)? {
            return Err(SessionError::WorkspaceFinished {
                session_id: reference.session_id.clone(),
            });
        }
        rd_put_same(&self.root, &cdir, "consumed.json", &record)?;
        Ok(record)
    }
    pub fn read_requester_consumption(
        &self,
        session_id: &str,
        challenge_id: &str,
    ) -> Result<Option<RequesterConsumption>, SessionError> {
        let cdir = self.rd_challenge_dir(session_id, challenge_id)?;
        rd_read(&cdir.join("consumed.json"))
    }
}
