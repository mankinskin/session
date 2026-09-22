# Goal

Make successive handoffs persist a compact delta instead of re-serializing ~97% identical bytes (subticket 2 of `96f9ffaa`). Depends on the schema cleanup (subticket 1) so the delta is defined over the flattened shape.

# Problem (verified)

Incoming handoff `02d19bac` (8562 B) and outgoing `816f0807` (8561 B) differ by ~10 substantive lines yet nearly all bytes are re-serialized verbatim. `create_handoff_record` (`memory-api/crates/session-api/src/store.rs:1343`) always writes a full record; there is no baseline reference.

# Solution Design

1. Add an optional delta representation to `SessionHandoffRecord`: `#[serde(default, skip_serializing_if = "Option::is_none")] baseline_handoff_id: Option<String>` plus `changed_nodes: Vec<...>`, `added_pins: Vec<...>`, `removed_pins: Vec<...>`.
2. On `create_handoff_record`, load the predecessor handoff (via lineage) as baseline; if present, diff workflow nodes and pins and persist only the delta with `baseline_handoff_id` set. If no baseline, persist a full record.
3. Provide a `materialize_handoff(handoff_id)` reader that walks `baseline_handoff_id` back to the nearest full record and replays deltas to reconstruct the complete `SessionHandoffRecord`. All existing readers (render, MCP get) go through materialization.
4. Guard against broken baseline chains (missing baseline → fall back to treating as full / error clearly).

# Non-goals

- Narrative/blockers fields (parent `6431985e`).

# Acceptance Criteria

1. A subsequent handoff over an unchanged workflow persists a delta significantly smaller than a full re-serialization.
2. `materialize_handoff` reconstructs the full record from baseline + deltas; output equals what a full record would have contained.
3. A broken/missing baseline chain is detected and reported, not silently wrong.
4. Existing full records still load and materialize (identity).
5. Focused tests cover full→delta→materialize and broken-chain cases.

# Traceability

- Parent: `96f9ffaa-6514-480a-afbd-b345cc206863`.
- Depends on schema-cleanup subticket.
- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913`.