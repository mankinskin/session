#[allow(unused_imports)]
use super::*;

impl SessionStoreConfig {
    pub(in super::super) fn rd_load_receipt(
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
