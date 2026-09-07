use super::*;
use feedback_api::EntityUrn;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStorePaths {
    pub session_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub transcript_path: PathBuf,
    pub events_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRuntimePaths {
    pub workspace_dir: PathBuf,
    pub handoffs_dir: PathBuf,
    pub finish_path: PathBuf,
}

pub(super) fn validate_session_id(value: &str) -> Result<(), SessionError> {
    let session_id = value.trim();
    if session_id.is_empty() || uuid::Uuid::parse_str(session_id).is_err() {
        return Err(SessionError::InvalidSessionId(value.to_string()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ParsedEntityUrn {
    pub(super) workspace_path: String,
    pub(super) kind: SessionPinnedEntityKind,
    pub(super) entity_id: String,
}

pub(super) fn parse_entity_urn(
    entity_urn: &str
) -> Result<ParsedEntityUrn, SessionError> {
    let trimmed = entity_urn.trim();
    let urn = EntityUrn::from_str(trimmed)
        .map_err(|_| SessionError::InvalidEntityUrn(trimmed.to_string()))?;
    let workspace_path = urn.workspace().to_string();
    let store = urn.store();
    let entity_id = urn.entity().to_string();

    let kind = match store {
        "ticket" | "tickets" => SessionPinnedEntityKind::Ticket,
        "spec" | "specs" => SessionPinnedEntityKind::Spec,
        "rule" | "rules" => SessionPinnedEntityKind::Rule,
        _ => return Err(SessionError::InvalidEntityUrn(trimmed.to_string())),
    };

    Ok(ParsedEntityUrn {
        workspace_path,
        kind,
        entity_id,
    })
}

pub(super) fn parse_entity_urn_kind(
    entity_urn: &str
) -> Result<SessionPinnedEntityKind, SessionError> {
    Ok(parse_entity_urn(entity_urn)?.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_session_id_rejects_slug_shaped_id() {
        let error = validate_session_id("epic-kickoff-8fdfe135").unwrap_err();

        assert!(matches!(error, SessionError::InvalidSessionId(_)));
        assert!(error.to_string().contains("must be a UUID"));
    }
}
