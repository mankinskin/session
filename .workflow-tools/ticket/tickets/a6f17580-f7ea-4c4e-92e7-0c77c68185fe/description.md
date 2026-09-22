# Goal

Extend the `SessionHandoffRecord` schema with the durable knowledge-transfer payload (subticket 1 of `6431985e`). Model + serde only — no population logic.

# Code touchpoints (verified)

- `SessionHandoffRecord` (`memory-api/crates/session-api/src/model.rs:251`) currently: `handoff_id, workspace_session_id, outgoing_run_id, created_at, resume_command, pinned_entities, workflow, validation`.
- `SessionHandoffResult` (`model.rs:265`) wraps `record + record_path + render`.

# Solution Design

Add to `SessionHandoffRecord`, all `#[serde(default, skip_serializing_if = "...")]` for backward compatibility:

- `summary: String` (`skip_serializing_if = "String::is_empty"`) — freeform run outcome.
- `remaining: Vec<String>` — remaining/next-action items.
- `decisions: Vec<String>` — decisions taken this run.
- `blockers: Vec<String>` — open blockers for the successor.
- `new_entities: Vec<SessionPinnedEntityHeader>` (or a lighter `Vec<String>` of URNs) — entities created/owned this run.

Do not change any writer yet; population is subticket 2. Only the struct, serde attributes, and a round-trip test.

# Acceptance Criteria

1. New fields exist on `SessionHandoffRecord` with serde defaults and skip-if-empty.
2. An old record JSON without the new fields deserializes with empty defaults.
3. A round-trip test asserts populated values serialize and deserialize identically, and that empty fields are omitted from JSON.

# Traceability

- Parent: `6431985e-e729-426b-9f91-66ad4b1c6fe6`.
- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913` (AC5).