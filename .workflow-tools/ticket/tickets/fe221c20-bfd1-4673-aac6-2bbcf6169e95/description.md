## Objective

Deliver the scripted orchestrator round-trip fixture (spec AC6) and the unit-level graph-reconstruction tests (spec AC1) that validate the whole epic end to end.

## Scope

- Unit tests (AC1): given persisted `SessionRecord` and handoff records covering a fan_out (1 source → N target-less handoffs, some claimed, some still open) and a merge (1 session with N entries in `picked_up_handoff_ids`), a graph-reconstruction routine built only from those persisted records recovers the full forward and reverse provenance graph, correctly distinguishing claimed vs. open edges, with no auxiliary store consulted.
- Scripted orchestrator round-trip fixture (AC6), driving the worked example from [transcripts/29-07-2026_session-merge-pickup-workflow/input.clean.md](../../../transcripts/29-07-2026_session-merge-pickup-workflow/input.clean.md):
  1. Orchestrator session `fan_out`s to start 2 track sessions.
  2. Each of the 2 track sessions independently emits one `handoff`.
  3. A shared-track session `merge`s (picks up) both handoffs into itself.
  4. The shared-track session finishes and `fan_out`s back into 2 new handoffs (one per originating track).
  5. Each originating track's session `pickup`s its respective handoff and finishes.
  6. The orchestrator `merge`s (picks up) both finishing handoffs back into itself.
- Assertions: every provenance edge reconstructs correctly from persisted records alone; the orchestrator, the 2 original track sessions, and the shared-track session each appear exactly once in the reconstructed graph; no handoff is claimed twice; no unit of work (ticket/spec id referenced via `SessionLinks`) is lost or duplicated across the cycle.

## Code anchors

- memory-api/crates/session-api/tests/ (new fixture/integration test)

## Acceptance criteria

- AC1 unit tests pass.
- AC6 scripted fixture passes and asserts the full invariant list above.

## Spec

[c737328d Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) — AC1, AC6.

## Parent

Epic [d28afbc0](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).