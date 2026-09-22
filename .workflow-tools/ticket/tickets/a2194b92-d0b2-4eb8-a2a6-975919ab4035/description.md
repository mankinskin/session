## Objective

Introduce the first-class `Track` entity per spec [c737328d](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) requirement R5.

## Scope

- `Track` is a first-class entity persisted under `.session/tracks/<id>/`, session-side only.
- Manifest holds: `id`, `title`, `anchor_ticket_id` (the epic/tracker ticket the track is rooted in), member session ids, track-to-track edges (populated by the track-edges ticket below), and a rollup cache.
- All work status (ticket state, progress) stays in ticket-api and is read live through `anchor_ticket_id` — the manifest must never persist a duplicated status/state field.
- CRUD/store operations for the track manifest (create, load, update member session ids, update rollup cache).
- This ticket supersedes the current free-form `SessionRecord.track_id` string tag with a real entity; `SessionRecord.track_id` continues to reference the track id as a foreign key.

## Code anchors

- memory-api/crates/session-api/src/model.rs
- memory-api/crates/session-api/src/store_persistence_types.rs

## Acceptance criteria

- A `Track` can be created, persisted under `.session/tracks/<id>/`, and reloaded with all manifest fields intact.
- No status/state field is persisted on the track manifest; a read helper resolves live state through `anchor_ticket_id` against ticket-api.

## Spec

[c737328d Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) — R5.

## Parent

Epic [d28afbc0](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).