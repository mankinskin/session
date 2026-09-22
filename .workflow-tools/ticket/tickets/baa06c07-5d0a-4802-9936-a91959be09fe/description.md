## Objective

Backfill migration per spec [c737328d](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) requirement R9, run once before the legacy-field-removal ticket lands.

## Scope

- Migrate only records where `parent_session_id`, `spawned_session_id`, or `predecessor_handoff` is actually populated.
- For each populated record, derive the corresponding `emitted_handoff_ids`/`picked_up_handoff_ids` entries, creating a synthetic bound-target handoff edge where no handoff record already covers the link.
- Records with none of these fields populated are left untouched and are not visited beyond a version bump — no exhaustive rewrite of all historical session or handoff records.
- Run as a one-shot migration pass over existing `.session/sessions/**` and handoff stores.

## Code anchors

- memory-api/crates/session-api/src/store_persistence_types.rs
- memory-api/crates/session-api/src/model/handoff.rs

## Acceptance criteria

- A fixture with a populated legacy field produces a correctly derived handoff edge after migration.
- A fixture with unpopulated legacy fields is left byte-for-byte unchanged except for a version bump.
- This ticket must complete (and be validated) before the legacy-field-removal ticket removes the source fields.

## Spec

[c737328d Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) — R9; Migration section.

## Parent

Epic [d28afbc0](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).