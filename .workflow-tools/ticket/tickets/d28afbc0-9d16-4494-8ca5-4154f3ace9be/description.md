## Objective

Make N→1 merge and 1→N split of sessions representable, queryable, and validated, so work that
converges across independently started sessions can be merged onto a shared track and later
handed back out — with a complete provenance chain.

Source request: transcripts/29-07-2026_session-merge-pickup-workflow/input.clean.md
Interview record (all decisions): tmp/interview-session-merge-pickup.md

## Problem

Every provenance link in session-api today is 1:1, and `track` is a free-form string with no
entity behind it:

- `SessionRecord.parent_session_id` / `.spawned_session_id` — singular `Option<String>`
- `SessionRunLineage.predecessor_run_id` — singular
- `SessionHandoffPackage.predecessor_handoff` — singular
- `SessionRecord.track_id` — free-form string, no store, no metadata, no track-to-track edges

Consequence: a session that merges three predecessors cannot record that fact anywhere, and
tracks cannot contain or depend on other tracks.

## Decided architecture

1. **Handoffs are the provenance edges.** No separate MergeRecord and no separate edge store.
   Source sessions reference the handoffs they emitted; target sessions reference the handoffs
   they picked up. The graph is literal without duplicating the handoff primitive.
2. **A handoff is strictly one binary edge**: exactly one source, exactly one target.
   `fan_out` = one session emits N handoff records. `merge` = one session picks up N handoff
   records. No hyperedges.
3. **Target is bound at pickup time.** A handoff is created target-less and is a claimable,
   initially open edge. Pickup writes `target_session_id` on the handoff and appends to the
   picking-up session's `picked_up_handoff_ids`. Unclaimed handoffs form a queryable backlog.
4. **Canonical vocabulary** (CLI/MCP verbs, record types, spec language):
   `handoff` (1→1), `fan_out` (1→N), `merge` (N→1), `pickup` (consume any handoff).
5. **First-class Track entity** under `.session/tracks/<id>/`. Session-side coordination only:
   id, title, `anchor_ticket_id` (the epic), member session ids, track-to-track edges, rollup
   cache. All work state stays in ticket-api and is read through `anchor_ticket_id`.
6. **Track relationships use a single edge kind: `depends_on`.** `contains` is expressed as
   `depends_on` (a parent track depends on its child tracks). `overlaps` is derived, not
   stored: two tracks overlap when they depend on the same child track. Edges are written
   bidirectionally on both tracks so reverse lookup is O(1) and never requires a scan.
7. **Explicit merge, advisory detection.** Merge is always a deliberate operation. Advisory
   convergence detection flags candidates using all three signals — shared ticket/spec ids in
   `SessionLinks`, overlapping owned files on the board, and same `track_id` — but never merges
   on its own.
8. **Legacy fields.** Remove the three superseded cross-session fields:
   `SessionRecord.parent_session_id`, `SessionRecord.spawned_session_id`, and
   `SessionHandoffPackage.predecessor_handoff`. **Keep `SessionRunLineage.predecessor_run_id`** —
   it links runs within one session and is what makes `session_runtime_resume` work; handoff
   edges connect different sessions and do not replace it.
9. **Migration.** Backfill only where the removed singular links are actually populated; skip
   unpopulated records silently.

## Acceptance criteria

- A session can record N picked-up handoffs and M emitted handoffs; the provenance graph
  reconstructs every edge in both directions from persisted records alone.
- `fan_out`, `merge`, and `pickup` exist as operations on the CLI and MCP surfaces.
- A Track entity exists with bidirectional `depends_on` edges; `overlaps` is computable.
- Advisory convergence detection reports candidate merges and never mutates.
- `session_runtime_resume` behavior is unchanged; `run_lineage_init_resume_creates_distinct_linked_run`
  still passes.
- **Unit tests**: provenance graph reconstruction from persisted records.
- **Scripted round-trip fixture**: orchestrator starts 2 track sessions → each emits a handoff →
  merged into 1 shared-track session → shared session fans out back into 2 handoffs → both
  finish → merged back into the orchestrator. Asserts every provenance edge reconstructs and no
  work is lost or duplicated.

## Children

Existing tickets to reparent under this epic:

- 938a7ae9 — Track query and rollup surface
- 185a00a2 — MCP tool surface for track query and rollup
- f35f4dd9 — Embed a persistent step graph in the handoff package

New child tickets to create (Ticket Refinement Agent):

- Handoff edge model: emitted/picked-up handoff ids on `SessionRecord`, target binding at pickup
- Track entity and store under `.session/tracks/<id>/`
- Bidirectional `depends_on` track edges + derived `overlaps`
- `fan_out` / `merge` / `pickup` operations (API, then CLI, then MCP)
- Advisory convergence detection
- Removal of the three cross-session singular fields + conditional backfill migration
- Orchestrator round-trip fixture

## Related

- spec c677182e — Durable session workflow graph and handoff continuity
- spec 5e52039d — Handoff Package Schema
- spec b71658f1 — Iteration Loop Workflow
- tickets fd7737ec, ab02e15a (done) — added the track/lineage fields this epic supersedes

## Key files

- memory-api/crates/session-api/src/model.rs
- memory-api/crates/session-api/src/model/handoff.rs
- memory-api/crates/session-api/src/model/workflow.rs
- memory-api/crates/session-api/src/model/links.rs
- memory-api/crates/session-api/src/store/config/handoff_finish.rs
- memory-api/crates/session-api/src/store_persistence_types.rs
