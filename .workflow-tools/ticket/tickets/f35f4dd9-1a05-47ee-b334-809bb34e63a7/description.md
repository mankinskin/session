## Problem

`session_handoff` persists `objective`, `target_tickets`, `target_files`, `decisions`, `validation`, `non_goals`, `context_anchors`, `open_escalations`, `risk_notes`, `predecessor_handoff`. None of these encode the next steps. The most important part of a handoff — the plan the next session should execute — is missing, so each session restarts planning from scratch.

The durable workflow graph (`session_workflow_add_node` / `add_edge` / `set_status`) already has sufficient primitives: `kind="task"` nodes, `category` labels, `order` / `depends-on` edges, per-node status. What is missing is the link and the materialization path.

## Decisions (interview-resolved)

- Store an **embedded snapshot of nodes + edges** in the handoff record, so the handoff is self-contained and readable without the session store.
- The step graph is **required** for a package to be implementation-ready.
- Nodes and target tickets are **interchangeable and independent**: a caller may supply workflow nodes, target ticket ids, or both. There can be nodes without tickets and tickets without nodes. The system validates that supplied ids exist and materializes both into the handoff.
- A node may carry content distinct from a ticket id, and may qualify the operation on a ticket id (e.g. "review", "implement").
- Ordering / "what comes next" is expressed via the existing `order` and `depends-on` edges. No new `next_step_node_id` field.
- The step graph lives in the session store only. Do NOT mirror it onto target tickets.

## Spec

Update the existing reviewed spec **5e52039d Handoff Package Schema** (`.spec/specs/5e52039d-aabc-434d-bdf3-eca63e312476/`, linked ticket d3af78d7). Do not create a new spec.


## Scope change (decided architecture — reparented under epic d28afbc0)

Per spec [c737328d Session merge and pickup](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) (R1, R2, R7), the `predecessor_handoff` field this ticket's acceptance criteria currently mention is being **removed** (see ticket [12641ad0 Remove parent_session_id / spawned_session_id / predecessor_handoff](../../.ticket/tickets/12641ad0-3eea-48e7-927d-20b814b1b7e3/ticket.toml)) and superseded by the binary handoff provenance edge (`emitted_handoff_ids`/`picked_up_handoff_ids`, target bound at pickup — ticket [0869353b Handoff edge model](../../.ticket/tickets/0869353b-417c-4ce0-82bb-333e9fd39945/ticket.toml)). This ticket's embedded step-graph snapshot is orthogonal to and composes with that edge model: the step graph still lives on the handoff record, but any acceptance-criteria wording or code referencing `predecessor_handoff` must be updated to reference the new edge fields instead when this ticket is implemented after the edge-model ticket lands.

This ticket is now a child of epic [d28afbc0 Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).