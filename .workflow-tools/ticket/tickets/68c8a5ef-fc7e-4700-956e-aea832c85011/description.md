# Goal

Remove the two structural smells in the handoff workflow snapshot (subticket 1 of `96f9ffaa`) — low-risk schema cleanup, independent of the delta work.

# Problem (verified)

- `SessionWorkflowSnapshot` (`memory-api/crates/session-api/src/model.rs:226`) has `pub workflow: SessionWorkflowGraph`, so it serializes as double-nested `workflow.workflow.{nodes,edges}` inside the handoff record's own `workflow` field.
- `SessionWorkflowNodeResolution.live_ticket_state` (`model.rs:212`) is denormalized: it re-states state already derivable at read time from node status + the live ticket resolver (`workflow_snapshot`, `store.rs:1147`, rebuilds it every call anyway).

# Solution Design

1. Flatten the snapshot: either `#[serde(flatten)]` the graph or rename `workflow` → hoist `nodes`/`edges` to the snapshot level, eliminating `workflow.workflow`. Update `workflow_snapshot` (`store.rs:1147`) and all readers (`workflow_render_terminal`, mermaid render, handoff render).
2. Drop `live_ticket_state` from the persisted `SessionWorkflowNodeResolution`; recompute it on demand where readers need it (the resolver is already invoked in `workflow_snapshot`). If a persisted resolution list is still required, keep only `node_id` + diagnostics.
3. Provide a back-compat deserialization path so existing records with `workflow.workflow` and `live_ticket_state` still load.

# Acceptance Criteria

1. Newly persisted snapshots no longer contain a nested `workflow.workflow` object.
2. `live_ticket_state` is no longer persisted; live state is derived at read time.
3. Existing records with the old shape still deserialize.
4. Workflow render / mermaid / handoff render output is unchanged for a given graph.
5. Focused test covers old-shape load + new-shape round-trip.

# Traceability

- Parent: `96f9ffaa-6514-480a-afbd-b345cc206863`.
- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913`.