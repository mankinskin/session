//! Session-owned decision records; fixture ingress is not human identity proof.
use crate::SessionError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub use uuid::Uuid;

pub const REQUESTER_DECISION_SCHEMA_VERSION: u32 = 1;
pub const DECISION_COMMAND: &str = "mission-decision";
pub const USER_PROMPT_SUBMIT: &str = "UserPromptSubmit";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Approve,
    Reject,
    Replan,
}
impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::Replan => "replan",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        match value {
            "approve" => Some(Self::Approve),
            "reject" => Some(Self::Reject),
            "replan" => Some(Self::Replan),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDecision {
    pub kind: DecisionKind,
    pub mission_id: Uuid,
    pub revision: u64,
    pub plan_digest: String,
    pub review_snapshot_digest: String,
    pub challenge_id: String,
}
pub(crate) fn requester_valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}
pub(crate) fn requester_lower_hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
/// Accept only the complete standalone command, optionally followed by one newline.
pub fn parse_decision_prompt(raw: &[u8]) -> Option<ParsedDecision> {
    let text = std::str::from_utf8(raw).ok()?;
    let line = text.strip_suffix('\n').unwrap_or(text);
    if line.contains(['\n', '\r']) {
        return None;
    }
    let parts: Vec<&str> = line.split(' ').collect();
    let [command, kind, mission, revision, plan, review, challenge] = parts.as_slice() else {
        return None;
    };
    if *command != DECISION_COMMAND {
        return None;
    }
    let kind = DecisionKind::parse(kind)?;
    let mission_id = Uuid::parse_str(mission).ok()?;
    if mission_id.to_string() != *mission {
        return None;
    }
    let revision_value: u64 = revision.parse().ok()?;
    if revision_value.to_string() != *revision {
        return None;
    }
    if !requester_lower_hex64(plan)
        || !requester_lower_hex64(review)
        || !requester_valid_id(challenge)
    {
        return None;
    }
    Some(ParsedDecision {
        kind,
        mission_id,
        revision: revision_value,
        plan_digest: (*plan).into(),
        review_snapshot_digest: (*review).into(),
        challenge_id: (*challenge).into(),
    })
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequesterChallengeRequest {
    pub session_id: String,
    pub challenge_id: String,
    pub mission_id: Uuid,
    pub mission_root: PathBuf,
    pub revision: u64,
    pub plan_digest: String,
    pub review_snapshot_digest: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequesterChallenge {
    pub schema_version: u32,
    pub session_root: PathBuf,
    #[serde(flatten)]
    pub request: RequesterChallengeRequest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequesterDecisionRef {
    pub session_id: String,
    pub challenge_id: String,
    pub decision_id: String,
    pub source_event_id: String,
}
pub type DecisionReference = RequesterDecisionRef;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionIngress {
    pub hook_event_name: String,
    pub host_event_id_supplied: bool,
    pub trusted_local_host: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequesterDecisionReceipt {
    pub schema_version: u32,
    pub reference: DecisionReference,
    pub kind: DecisionKind,
    pub mission_id: Uuid,
    pub revision: u64,
    pub plan_digest: String,
    pub review_snapshot_digest: String,
    pub raw_prompt_sha256: String,
    pub ingress: DecisionIngress,
    /// Derived from store layout by the resolver, never trusted from disk.
    #[serde(skip)]
    pub raw_path: PathBuf,
}
impl RequesterDecisionReceipt {
    pub fn read_raw_prompt(&self) -> Result<Vec<u8>, SessionError> {
        let bytes = std::fs::read(&self.raw_path).map_err(|source| SessionError::Io {
            path: self.raw_path.clone(),
            source,
        })?;
        use sha2::{Digest, Sha256};
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if digest != self.raw_prompt_sha256 {
            return Err(SessionError::RequesterDecision {
                code: "requester_decision_corrupt",
                message: "raw prompt bytes do not match the receipt hash".into(),
            });
        }
        Ok(bytes)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequesterConsumption {
    pub reference: DecisionReference,
    pub grant_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRequesterDecision {
    pub challenge_id: String,
    pub stage: String,
}
/// Fault injection is not an authority input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionFault {
    AfterIntent,
    AfterRawPersisted,
    AfterSourceEvent,
    AfterEventMirror,
}

/// Local host ingress; no agent tool surface may expose its constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPromptIngress {
    hook_event_name: String,
    raw_prompt: Vec<u8>,
    host_event_id: Option<String>,
}
impl HostPromptIngress {
    pub fn trusted_local_hook(
        hook_event_name: impl Into<String>,
        raw_prompt: impl Into<Vec<u8>>,
        host_event_id: Option<String>,
    ) -> Self {
        Self {
            hook_event_name: hook_event_name.into(),
            raw_prompt: raw_prompt.into(),
            host_event_id,
        }
    }
    pub fn hook_event_name(&self) -> &str {
        &self.hook_event_name
    }
    pub fn raw_prompt(&self) -> &[u8] {
        &self.raw_prompt
    }
    pub fn host_event_id(&self) -> Option<&str> {
        self.host_event_id.as_deref()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequesterDecisionIntent {
    pub schema_version: u32,
    pub session_id: String,
    pub challenge_id: String,
    pub decision_id: String,
    pub kind: DecisionKind,
    pub mission_id: Uuid,
    pub revision: u64,
    pub plan_digest: String,
    pub review_snapshot_digest: String,
    pub raw_prompt_sha256: String,
    pub source_event_id: String,
    pub host_event_id_supplied: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequesterSourceEvent {
    pub schema_version: u32,
    pub session_id: String,
    pub source_event_id: String,
    pub host_event_id_supplied: bool,
    pub hook_event_name: String,
    pub raw_prompt_sha256: String,
    pub challenge_id: String,
    pub decision_id: String,
}
impl SessionError {
    pub fn requester_code(&self) -> Option<&'static str> {
        match self {
            Self::RequesterDecision { code, .. } => Some(code),
            _ => None,
        }
    }
}
