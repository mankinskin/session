# Goal

Make `SessionHandoffRecord` carry the durable knowledge-transfer payload that spec `8c880efc` AC5 and ticket `0647a212` scope already require, instead of stranding it in the chat transcript. **This is now a tracker parent decomposed into 3 subtickets.**

# Problem (evidence)

`SessionHandoffRecord` (`memory-api/crates/session-api/src/model.rs:251`) has `handoff_id, workspace_session_id, outgoing_run_id, created_at, resume_command, pinned_entities, workflow, validation` and no `blockers` or narrative fields.

- Observed record `816f0807-...json` persisted `validation: [{validation_spec_id, required:true}]` with `outcome=None` and no blockers; the entire findings/decisions/next-actions handoff lived only in session `beca8ec5` transcript.
- Root cause of `outcome=None`: `create_handoff_record` (`store.rs:1343`) calls `resolve_validation_gates(&context, validation, false)`, and `resolve_validation_gates` (`store.rs:1614`) only populates `outcome` for gates backed by a *required workflow validation node*; caller gates with no matching node pass through with `outcome=None`.

# Decomposition

1. `a6f17580` — Add narrative/blockers/new_entities fields to `SessionHandoffRecord` model (serde + round-trip).
2. `e8bdb7cf` — Populate resolved validation `outcome` + `new_entities` at handoff creation (depends on 1).
3. `f77e35d8` — Thread narrative fields through CLI/MCP/`/handoff` prompt (depends on 2).

Subticket ordering: 1 → 2 → 3. Parent `depends_on` all three.

# Acceptance Criteria (parent, satisfied by subtickets)

1. A persisted handoff record includes non-empty `summary`, `blockers`, and resolved validation `outcome` when the run recorded validation.
2. `new_entities` reflects tickets/specs created or owned during the run.
3. Old records without the new fields still deserialize.
4. Focused tests assert the new fields persist and round-trip.

# Non-goals

- Delta/baseline serialization (tracked in `96f9ffaa`).
- Auto-generating narrative from the transcript.

# Traceability

- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913` (AC5, Positions handoff row).
- Predecessor: `0647a212-9d2e-4943-9627-f854ce3f14c4` (unfinished contract).
- Epic: `effba966-f0a8-4d7d-b289-b7feba826cf8`.