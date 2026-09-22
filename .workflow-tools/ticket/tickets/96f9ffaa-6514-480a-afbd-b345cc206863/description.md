# Goal

Reduce handoff record size and remove structural smells so inter-agent records are compact and clean. **This is now a tracker parent decomposed into 2 subtickets** because it bundles one low-risk cleanup and one larger format change.

# Problem (evidence)

Incoming handoff `02d19bac` (8562 B) and outgoing `816f0807` (8561 B) differ by ~10 substantive lines yet ~97% of bytes are re-serialized verbatim. `SessionWorkflowSnapshot` (`model.rs:226`) serializes as double-nested `workflow.workflow.{nodes,edges}` and carries `resolutions[].live_ticket_state` (`model.rs:212`) already derivable from node status + the live resolver in `workflow_snapshot` (`store.rs:1147`).

# Decomposition

1. `68c8a5ef` — Flatten `SessionWorkflowSnapshot` (remove `workflow.workflow`) and drop denormalized `live_ticket_state`. Low risk, no format negotiation.
2. `89d1f983` — Delta/baseline handoff serialization with a full `materialize_handoff` reconstruct path (depends on 1, so the delta is defined over the flattened shape).

Subticket ordering: 1 → 2. Parent `depends_on` both.

# Acceptance Criteria (parent, satisfied by subtickets)

1. A subsequent handoff persists a delta significantly smaller than a full re-serialization for an unchanged workflow.
2. Full workflow can be reconstructed from baseline + deltas.
3. Snapshot no longer double-nests `workflow` and no longer stores redundant live ticket state.
4. Existing records still load.

# Non-goals

- Narrative/blockers fields (`6431985e`).

# Traceability

- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913`.
- Epic: `effba966-f0a8-4d7d-b289-b7feba826cf8`.