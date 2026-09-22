## Objective

Implement the canonical `fan_out` / `merge` / `pickup` operations at the session-api layer per spec [c737328d](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) requirement R4 (building on the edge model and track edges).

## Scope

- `fan_out`: one session emits N target-less handoff records (N edges), appending each id to the emitting session's `emitted_handoff_ids`.
- `merge`: one session picks up N open handoffs, binding each handoff's `target_session_id` to itself and appending each id to its `picked_up_handoff_ids`.
- `pickup`: claim one open handoff, binding its target (the single-handoff case of `merge`).
- These are canonical vocabulary — no other terms ("split", "join", "spawn") are introduced for these operations at this or any downstream (CLI/MCP) layer.
- `merge` is always an explicit, caller-invoked operation; it never runs automatically.
- These operations compose with `Track` membership: a session record's `track_id` and the track's member session ids are consistent after `fan_out`/`merge`.

## Code anchors

- memory-api/crates/session-api/src/model/handoff.rs
- memory-api/crates/session-api/src/store/config/handoff_finish.rs

## Acceptance criteria

- Unit tests exercise `fan_out` (1 source → N target-less handoffs), `pickup` (claim one open handoff, binding target), and `merge` (N handoffs claimed by one session).
- Contributes to spec AC1 (graph reconstruction correctness after these operations) and is a direct dependency of the round-trip fixture (AC6).

## Spec

[c737328d Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) — R4.

## Parent

Epic [d28afbc0](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).