//! Adapter from session's existing `CopilotHookEvent` capture shape onto the
//! kernel's generic `EventEnvelope` and domain-manifest contracts.
//!
//! Per `transcripts/15-09-2026_entity-kernel-all-domains/06-domain-adoption.md`,
//! session's existing hook envelope is the closest existing analog to the
//! kernel event envelope, so this module validates the kernel's
//! generalization against session's real capture shape. It is purely
//! additive: it performs no I/O, does not replace [`crate::hook::CopilotHookEvent`],
//! and does not alter session capture, transcript, or tool-execution
//! behavior. It only converts already-captured data into the kernel's
//! shapes on demand.

use chrono::{DateTime, Utc};
use memory_kernel::model::{
    domain::{DomainId, DomainSchemaVersion, EntityTypeId, EntityTypeSchemaVersion},
    domain_manifest::{
        DomainManifest, DomainManifestError, EntityTypeMembership, EntityTypeStatus,
    },
    entity::EntityId,
    event::{EventEnvelope, MutationOperation},
};
use uuid::Uuid;

use crate::hook::CopilotHookEvent;

/// Session's kernel `DomainId`.
pub fn session_domain_id() -> DomainId {
    DomainId::new("session").expect("static domain id literal is non-empty")
}

/// Session's single kernel `EntityTypeId`: the captured session record.
pub fn session_entity_type_id() -> EntityTypeId {
    EntityTypeId::new("session-record").expect("static entity type id literal is non-empty")
}

/// Session's kernel domain manifest: one active entity type,
/// `session-record`. Declares no new business schema; it only registers the
/// existing session-record concept under the kernel's manifest contract.
pub fn session_domain_manifest() -> Result<DomainManifest, DomainManifestError> {
    DomainManifest::new(
        session_domain_id(),
        DomainSchemaVersion(1),
        vec![EntityTypeMembership {
            entity_type_id: session_entity_type_id(),
            schema_version: EntityTypeSchemaVersion(1),
            status: EntityTypeStatus::Active,
        }],
    )
}

/// Errors converting a [`CopilotHookEvent`] into a kernel [`EventEnvelope`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HookEventEnvelopeError {
    #[error("session_id '{0}' is not a valid kernel entity id")]
    InvalidSessionId(String),
}

/// Convert a captured [`CopilotHookEvent`] plus its owning `session_id` into
/// a kernel [`EventEnvelope`] describing `operation` against the session
/// entity.
///
/// Pure conversion: reads `event.captured_at` (falling back to
/// `occurred_at_fallback`), `event.event_id` / `event.parent_event_id`
/// (falling back to fresh/self UUIDs when absent or non-UUID), and
/// `event.data_json` (attached as post-mutation state). It does not mutate
/// `event`, perform I/O, or change how session capture builds
/// `CopilotHookEvent` values.
pub fn envelope_from_hook_event(
    event: &CopilotHookEvent,
    session_id: &str,
    operation: MutationOperation,
    occurred_at_fallback: DateTime<Utc>,
) -> Result<EventEnvelope, HookEventEnvelopeError> {
    let entity_id: EntityId = Uuid::parse_str(session_id)
        .map_err(|_| HookEventEnvelopeError::InvalidSessionId(session_id.to_string()))?;

    let event_uuid = event
        .event_id
        .as_ref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .unwrap_or_else(Uuid::new_v4);

    // Correlate with the parent event when present so a related chain of
    // captured events shares one correlation id; otherwise this event
    // correlates with itself.
    let correlation_id = event
        .parent_event_id
        .as_ref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .unwrap_or(event_uuid);

    let occurred_at = event.captured_at.unwrap_or(occurred_at_fallback);

    let mut envelope = EventEnvelope::new(
        event_uuid,
        session_domain_id(),
        session_entity_type_id(),
        entity_id,
        operation,
        occurred_at,
        correlation_id,
        DomainSchemaVersion(1),
        EntityTypeSchemaVersion(1),
    );

    if let Some(data) = &event.data_json {
        envelope = envelope.with_post_state(data.clone());
    }

    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_kernel::model::event::MutationOperation;

    fn sample_event() -> CopilotHookEvent {
        CopilotHookEvent {
            event_id: Some("11111111-1111-1111-1111-111111111111".to_string()),
            parent_event_id: None,
            event_type: Some("tool_execution".to_string()),
            captured_at: Some(DateTime::<Utc>::MIN_UTC),
            turn_id: None,
            message_id: None,
            tool_call_id: None,
            tool_name: None,
            tool_success: None,
            reasoning_text: None,
            tool_requests_json: None,
            tool_arguments_json: None,
            data_json: Some(serde_json::json!({"tool": "read_file"})),
            raw_event_json: None,
        }
    }

    #[test]
    fn session_domain_manifest_registers_one_active_entity_type() {
        let manifest = session_domain_manifest().unwrap();
        assert_eq!(manifest.domain_id, session_domain_id());
        let active: Vec<_> = manifest.active_entity_types().collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].entity_type_id, session_entity_type_id());
    }

    #[test]
    fn envelope_from_hook_event_uses_event_id_captured_at_and_post_state() {
        let event = sample_event();
        let session_id = "22222222-2222-2222-2222-222222222222";
        let envelope = envelope_from_hook_event(
            &event,
            session_id,
            MutationOperation::Update,
            DateTime::<Utc>::MIN_UTC,
        )
        .unwrap();

        assert_eq!(envelope.domain_id, session_domain_id());
        assert_eq!(envelope.entity_type_id, session_entity_type_id());
        assert_eq!(envelope.entity_id, Uuid::parse_str(session_id).unwrap());
        assert_eq!(
            envelope.event_id,
            Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
        );
        assert_eq!(envelope.occurred_at, DateTime::<Utc>::MIN_UTC);
        assert_eq!(
            envelope.post_state,
            Some(serde_json::json!({"tool": "read_file"}))
        );
    }

    #[test]
    fn envelope_from_hook_event_falls_back_to_occurred_at_and_fresh_ids() {
        let mut event = sample_event();
        event.event_id = None;
        event.captured_at = None;
        let fallback = DateTime::<Utc>::MIN_UTC;

        let envelope = envelope_from_hook_event(
            &event,
            "22222222-2222-2222-2222-222222222222",
            MutationOperation::Create,
            fallback,
        )
        .unwrap();

        assert_eq!(envelope.occurred_at, fallback);
        // A fresh event_id is generated and correlates with itself.
        assert_eq!(envelope.correlation_id, envelope.event_id);
    }

    #[test]
    fn envelope_from_hook_event_rejects_non_uuid_session_id() {
        let event = sample_event();
        let err =
            envelope_from_hook_event(&event, "not-a-uuid", MutationOperation::Update, Utc::now())
                .unwrap_err();
        assert_eq!(
            err,
            HookEventEnvelopeError::InvalidSessionId("not-a-uuid".to_string())
        );
    }
}
