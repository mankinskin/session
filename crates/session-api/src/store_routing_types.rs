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
    let urn = EntityUrn::from_str(trimmed).map_err(|err| {
        SessionError::InvalidEntityUrn(format!(
            "'{trimmed}' (expected ce://<workspace>/<store>/<entity> with store ticket|spec|rule|dossier, details: {err})"
        ))
    })?;
    let workspace_path = urn.workspace().to_string();
    let store = urn.store();
    let entity_id = urn.entity().to_string();

    let kind = match store {
        "ticket" | "tickets" => SessionPinnedEntityKind::Ticket,
        "spec" | "specs" => SessionPinnedEntityKind::Spec,
        "rule" | "rules" => SessionPinnedEntityKind::Rule,
        "dossier" | "dossiers" | "transcript" | "transcripts" => SessionPinnedEntityKind::Dossier,
        _ => {
            return Err(SessionError::InvalidEntityUrn(format!(
                "'{trimmed}' has unsupported store '{store}' (expected ticket|spec|rule|dossier)"
            )))
        },
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

    #[test]
    fn parse_entity_urn_accepts_valid_stores() {
        let parsed_ticket = parse_entity_urn("ce://default/ticket/t-123").unwrap();
        assert_eq!(parsed_ticket.workspace_path, "default");
        assert_eq!(parsed_ticket.kind, SessionPinnedEntityKind::Ticket);
        assert_eq!(parsed_ticket.entity_id, "t-123");

        let parsed_spec = parse_entity_urn("ce://default/specs/spec-abc").unwrap();
        assert_eq!(parsed_spec.kind, SessionPinnedEntityKind::Spec);

        let parsed_rule = parse_entity_urn("ce://default/rules/rule-xyz").unwrap();
        assert_eq!(parsed_rule.kind, SessionPinnedEntityKind::Rule);

        let parsed_dossier = parse_entity_urn("ce://default/dossier/13-09-2026_my-slug").unwrap();
        assert_eq!(parsed_dossier.kind, SessionPinnedEntityKind::Dossier);
        assert_eq!(parsed_dossier.entity_id, "13-09-2026_my-slug");

        let parsed_transcript = parse_entity_urn("ce://default/transcripts/13-09-2026_transcript-slug").unwrap();
        assert_eq!(parsed_transcript.kind, SessionPinnedEntityKind::Dossier);
    }

    #[test]
    fn parse_entity_urn_rejects_raw_paths_and_unsupported_stores() {
        let err_path = parse_entity_urn("path:transcripts/13-09-2026_my-slug").unwrap_err();
        assert!(err_path.to_string().contains("expected ce://<workspace>/<store>/<entity>"));

        let err_store = parse_entity_urn("ce://default/unknown_store/entity-1").unwrap_err();
        assert!(err_store.to_string().contains("unsupported store 'unknown_store'"));
    }
}
