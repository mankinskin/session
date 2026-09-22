# Goal

Persist structured session handoffs and resume the same durable workspace under a new capture run.

## Scope

- Persist a handoff record before rendering the next-session prompt.
- Include `workspace_session_id`, outgoing `run_id`, handoff ID, pins, workflow status, blockers, validation state, and exact resume command.
- Resume the same workspace ID with a new run ID linked to the outgoing/predecessor run.
- Add explicit, idempotent finish behavior gated by required workflow nodes and validation outcomes.
- Preserve completed context, workflow, handoff, and run lineage as history.
- Provide core APIs consumed by session CLI/MCP and generated `/handoff` guidance.

## Acceptance Criteria

1. Handoff persistence completes before prompt rendering.
2. Every handoff payload includes the durable workspace ID, outgoing run ID, handoff ID, and exact resume command.
3. Resume rejects accidental reuse of the outgoing run ID and records predecessor linkage.
4. Finish rejects incomplete required nodes or missing/failed required validation.
5. Optional nodes may remain incomplete only when explicitly deferred with a reason.
6. Finish accepts a complete graph and is idempotent.
7. Focused tests cover handoff/resume continuity, crash-safe ordering, and finish gates.

## Depends on

- Durable workflow persistence `70cd7056-c342-4433-ad60-5bc798f61aa6`.

## Specs

- Durable workflow `c677182e-90da-4ac3-8b94-9e2e97c825cf`.
- Handoff prompts `9e04ff58-9160-4766-b307-74c0fb32a92c`.