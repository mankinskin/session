# Goal

Store each handoff as a folder containing the canonical `handoff.json` plus a rendered `handoff.md`, so markdown handoffs sit next to the JSON for human review, feedback, and research loops (subticket 3 of the flattening tracker). Depends on the layout flatten.

# Problem (verified)

- `create_handoff_record` (`memory-api/crates/session-api/src/store.rs:1343`) writes a single flat file `handoffs/<handoff_id>.json`.
- A terminal render already exists (`render_handoff_record_terminal`, store.rs:2108) but is never persisted — the human-readable form is thrown away.

# Solution Design

1. Change the handoff writer to create a folder `handoffs/<handoff_id>/` and write:
   - `handoff.json` — canonical record (unchanged schema, or delta form per `96f9ffaa`).
   - `handoff.md` — deterministic markdown rendering of the record.
2. Add a `render_handoff_record_markdown(&SessionHandoffRecord) -> String` producing headed sections (summary, remaining, decisions, blockers, pins, workflow, validation, resume command) — reuse the terminal renderer's structure.
3. Update `create_handoff_result.record_path` and CLI/MCP outputs to point at the folder (and/or the `.md`).
4. Back-compat read: still resolve legacy flat `handoffs/<id>.json`.

# Acceptance Criteria

1. A new handoff persists a folder with both `handoff.json` and `handoff.md`.
2. `handoff.md` deterministically reflects the record's fields.
3. Legacy flat `handoffs/<id>.json` records still load.
4. Focused test asserts folder creation + markdown content + JSON round-trip.

# Traceability

- Parent: flattening tracker. Depends on layout-flatten subticket.
- Related: narrative payload `6431985e` (richer md), delta form `96f9ffaa`.
- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913`.