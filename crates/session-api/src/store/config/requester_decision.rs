#[allow(unused_imports)]
use super::*;

pub(super) use crate::requester_decision::{
    DecisionFault, DecisionIngress, DecisionKind, DecisionReference, HostPromptIngress,
    ParsedDecision, PendingRequesterDecision, REQUESTER_DECISION_SCHEMA_VERSION,
    RequesterChallenge, RequesterChallengeRequest, RequesterConsumption, RequesterDecisionIntent,
    RequesterDecisionReceipt, RequesterSourceEvent, USER_PROMPT_SUBMIT, parse_decision_prompt,
    requester_lower_hex64, requester_valid_id,
};

pub(super) const REQUESTER_DIR: &str = "requester";
pub(super) fn rd_err(code: &'static str, message: impl Into<String>) -> SessionError {
    SessionError::RequesterDecision {
        code,
        message: message.into(),
    }
}
pub(super) fn rd_read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, SessionError> {
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
pub(super) fn rd_put_bytes(
    root: &Path,
    dir: &Path,
    name: &str,
    bytes: &[u8],
) -> Result<bool, SessionError> {
    ensure_real_directories(root, dir)?;
    fs::create_dir_all(dir).map_err(|source| SessionError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    ensure_real_directories(root, dir)?;
    write_new_synced(&dir.join(name), bytes)
}
pub(super) fn rd_put_same<T: Serialize + DeserializeOwned + PartialEq>(
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
pub(super) fn rd_fault(
    configured: Option<DecisionFault>,
    point: DecisionFault,
) -> Result<(), SessionError> {
    if configured == Some(point) {
        return Err(rd_err(
            "requester_decision_pending",
            format!("interrupted after {point:?}; redeliver the prompt to reconcile"),
        ));
    }
    Ok(())
}
pub(super) fn rd_verify_records(
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

pub(super) fn rd_decision_id(session_id: &str, challenge_id: &str) -> String {
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
    pub(super) fn rd_root_dir(&self, session_id: &str) -> Result<PathBuf, SessionError> {
        let dir = self
            .runtime_paths_for_workspace(session_id)?
            .workspace_dir
            .join(REQUESTER_DIR);
        ensure_real_directories(&self.root, &dir)?;
        Ok(dir)
    }
    pub(super) fn rd_challenge_dir(
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
    pub(super) fn rd_require_session(&self, session_id: &str) -> Result<(), SessionError> {
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
}

#[path = "requester_decision/ingestion.rs"]
mod ingestion;

#[path = "requester_decision/receipts.rs"]
mod receipts;
