# Goal

Prevent a handoff from being "stale at birth" by detecting durable entities the run created or owned that are absent from the workflow graph and pinned entities.

# Problem (evidence)

In session `beca8ec5`, ticket `8f364a0c` was created mid-run but never added via `workflow_add_node`/`workflow_promote`, so outgoing handoff `816f0807` carried the same 16 pins as the incoming record and the new ticket appeared in neither `pinned_entities` nor the workflow graph. The next agent can only learn it exists from prose. Spec `8c880efc` AC3 allows adding discovered work but nothing gates handoff completeness.

# Code touchpoints (verified)

- `create_handoff_record` (`memory-api/crates/session-api/src/store.rs:1343`) assembles `workflow` (via `workflow_snapshot`) and `pinned_entities` (via `view_runtime_context`) with no completeness check.
- `SessionWorkflowGraph.nodes` (`model.rs:189`) each carry an optional `ticket_urn`; pins carry `urn`. These two sets are the "represented entities" universe to diff against.
- `new_entities` (added by parent `6431985e`) is the authoritative list of run-created/owned URNs.

# Solution Design

1. In `create_handoff_record` (and `finish_workflow`), after building the snapshot and pins, compute `represented = { node.ticket_urn } ∪ { pin.urn }`.
2. Compute `session_entities` from `new_entities` (from `6431985e`) plus board `owned_files`-derived ticket ownership for this session's agent id.
3. `missing = session_entities \ represented`. For each missing entity emit a non-fatal warning carrying the exact remediation call (`workflow_add_node --ticket-urn <urn>` or `runtime_pin <urn>`).
4. Surface warnings in the `SessionHandoffResult` (new `#[serde(default, skip_serializing_if)] completeness_warnings: Vec<...>`) and in the terminal render (`render_handoff_record_terminal`, `store.rs:2108`).
5. Optional strict mode (config or flag) that turns missing required-scope entities into a `FinishBlocked`/handoff error.

# Non-goals

- Auto-adding nodes or auto-pinning (remediation is suggested, not performed).

# Acceptance Criteria

1. Handoff surfaces a warning enumerating session-created/owned entities absent from workflow nodes and pins.
2. Each warning includes the exact remediation call.
3. Optional strict mode fails the handoff on missing required-scope entities.
4. Focused test covers the create-ticket-without-node case (mirrors `8f364a0c`).

# Traceability

- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913` (AC3/AC5).
- Depends on `6431985e` for `new_entities`.
- Epic: `effba966-f0a8-4d7d-b289-b7feba826cf8`.