## Objective

Implement the advisory, non-mutating convergence detector per spec [c737328d](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) requirement R8.

## Scope

- Flags merge *candidates* using all three signals together: (a) shared ticket or spec ids in `SessionLinks`, (b) overlapping owned files on the board, (c) same `track_id`.
- Read-only: must never mutate session, handoff, or track state. Output is a suggestion surfaced to the caller, never an automatic merge.
- `merge` remains an explicit, user- or agent-invoked operation regardless of detector output.

## Code anchors

- memory-api/crates/session-api/src/model/links.rs
- memory-api/crates/session-api/src/model.rs (board file ownership / track_id access)

## Acceptance criteria

- Maps to spec AC5: run against fixtures with each of the three signals present individually and in combination; returns candidate flags only and performs no writes to any session, handoff, or track record (verified by asserting store state is unchanged before/after detector runs).

## Spec

[c737328d Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) — R8; AC5.

## Parent

Epic [d28afbc0](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).