# Goal

Let agents supply structured handoff narrative through the surfaces instead of pasting prose into chat (subticket 3 of `6431985e`). Depends on model (subticket 1) and population (subticket 2).

# Code touchpoints

- Handoff entrypoints: `create_handoff_result` / `render_handoff_terminal` (`memory-api/crates/session-api/src/store.rs:1383+`).
- Terminal render: `render_handoff_record_terminal` (`store.rs:2108`) must print the new narrative/blockers/new_entities sections.
- session-cli `handoff` command and session-mcp `session_handoff` tool input schemas.
- The generated `/handoff` prompt template.

# Solution Design

1. Add optional `--summary`, `--remaining` (repeatable), `--decisions` (repeatable), `--blockers` (repeatable) inputs to the session-cli `handoff` command and matching fields to the session-mcp `session_handoff` input.
2. Thread those into the request consumed by `create_handoff_record` (populated in subticket 2).
3. Extend `render_handoff_record_terminal` to render the narrative, blockers, and `new_entities` sections.
4. Update the `/handoff` prompt to instruct agents to fill these structured fields rather than free prose.

# Acceptance Criteria

1. `handoff` via CLI and MCP accepts and persists summary/remaining/decisions/blockers.
2. Terminal render shows the new sections when populated and omits them when empty.
3. The `/handoff` prompt references the structured fields.
4. Focused test covers CLI/MCP input → persisted record round-trip.

# Traceability

- Parent: `6431985e-e729-426b-9f91-66ad4b1c6fe6`.
- Depends on population subticket.
- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913` (AC5).