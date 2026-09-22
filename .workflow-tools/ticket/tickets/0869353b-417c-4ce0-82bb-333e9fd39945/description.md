## Objective

Implement the binary handoff provenance edge per spec [c737328d Session merge and pickup](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) requirements R1, R2, R3.

## Scope

- `SessionRecord` gains `emitted_handoff_ids: Vec<String>` and `picked_up_handoff_ids: Vec<String>`.
- A handoff record has exactly one source session and at most one target session (no arrays of sources/targets inside one record).
- A handoff is created target-less (claimable, open edge). A `pickup` write path binds `target_session_id` on the handoff record and appends the handoff id to the picking-up session's `picked_up_handoff_ids`. (Note: the `pickup` operation itself, plus `fan_out`/`merge`, are implemented by the API-ops ticket below — this ticket delivers the data model and the single-handoff bind-on-pickup write path it depends on.)
- Add an operation to list open (target-less) handoffs, optionally scoped by `track_id` or by source session — the unclaimed-handoff backlog query.
- No separate `MergeRecord` entity, no separate edge store — the handoff record remains the sole edge primitive.

## Code anchors

- memory-api/crates/session-api/src/model.rs
- memory-api/crates/session-api/src/model/handoff.rs
- memory-api/crates/session-api/src/store_persistence_types.rs

## Acceptance criteria

- Maps to spec AC1 (graph reconstruction from persisted records, distinguishing claimed vs. open edges) and AC2 (unclaimed-handoff backlog query scoped by track and by source session).
- `SessionRecord` round-trips `emitted_handoff_ids`/`picked_up_handoff_ids` through persistence.
- A handoff created without a target is queryable via the backlog listing; pickup binds `target_session_id` and is reflected in both the handoff record and the target session's `picked_up_handoff_ids`.

## Spec

[c737328d Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) — R1, R2, R3; AC1, AC2.

## Parent

Epic [d28afbc0](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).