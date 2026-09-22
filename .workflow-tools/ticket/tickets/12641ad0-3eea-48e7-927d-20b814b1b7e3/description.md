## Objective

Remove the superseded singular cross-session fields per spec [c737328d](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) requirement R7, after the backfill migration ticket below has run.

## Scope

- Remove `SessionRecord.parent_session_id`, `SessionRecord.spawned_session_id`, and `SessionHandoffPackage.predecessor_handoff`. The handoff edge graph (`emitted_handoff_ids`/`picked_up_handoff_ids`, target-bound-at-pickup) becomes the only cross-session lineage mechanism.
- **Explicitly KEEP `SessionRunLineage.predecessor_run_id`** — it links runs *within* one workspace session and backs `session_runtime_resume`; it is intra-session, not cross-session, lineage and is out of scope for removal.
- Existing test `run_lineage_init_resume_creates_distinct_linked_run` must continue to pass unmodified (spec AC3) — do not touch it as part of this removal.
- Update any spec 5e52039d (Handoff Package Schema) references to `predecessor_handoff` in place.

## Code anchors

- memory-api/crates/session-api/src/model.rs
- memory-api/crates/session-api/src/model/handoff.rs

## Acceptance criteria

- Maps to spec AC3: `run_lineage_init_resume_creates_distinct_linked_run` passes unmodified.
- Grep confirms no remaining references to `parent_session_id`, `spawned_session_id`, or `predecessor_handoff` outside of migration/backfill code and history/changelog text.
- `predecessor_run_id` is untouched.

## Spec

[c737328d Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) — R7; AC3.

## Parent

Epic [d28afbc0](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).