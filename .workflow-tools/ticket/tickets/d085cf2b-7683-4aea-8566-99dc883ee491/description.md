## Objective

Implement track-to-track relationships per spec [c737328d](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) requirement R6.

## Scope

- Exactly one persisted track edge kind: `depends_on`. `contains` is expressed as `depends_on` (a parent track depends on its child tracks) — no separate `contains` kind.
- `overlaps` is derived, never stored: two tracks overlap when they both depend on the same child track, computed at query time.
- `depends_on` edges are written on **both** the depending track and the depended-on track's manifest, so "which larger tracks depend on me" is an O(1) lookup on the child's own manifest, never a full scan.
- Write-side consistency: the write path must guarantee both sides are updated together. Choose and implement a mechanism (transactional write vs. repair/reconciliation pass) — the spec leaves the mechanism open, but this ticket must pick one and document it.
- **Reciprocity check**: add a health/consistency check (unit test and/or repair routine) that detects a `depends_on` edge present on one side but missing on the other, so drift is caught.

## Code anchors

- memory-api/crates/session-api/src/model.rs
- memory-api/crates/session-api/src/store_persistence_types.rs

## Acceptance criteria

- Maps to spec AC4: `depends_on` edges written from a parent to a child are readable in reverse from the child's own manifest without a scan; a derived `overlaps` query over two tracks depending on the same child returns that overlap without any stored `overlaps` edge.
- A reciprocity check test fails if a one-sided edge is constructed and passes once both sides are written.

## Spec

[c737328d Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) — R6; AC4.

## Parent

Epic [d28afbc0](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).