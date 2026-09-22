# Goal

Render the durable session workflow as a compact terminal dependency graph or deterministic Mermaid flowchart.

## Scope

- Add a terminal renderer with node status, required/optional markers, live ticket state, and dependency order.
- Add deterministic Mermaid `flowchart` rendering with escaped labels and stable node IDs.
- Ensure both formats represent the same nodes, edges, statuses, blockers, and completion state.
- Return diagnostics for unresolved ticket URNs without mutating workflow state.
- Expose renderer functions for later CLI/MCP transport wiring.

## Acceptance Criteria

1. Terminal and Mermaid output is deterministic across repeated renders.
2. Both formats include ticket-backed and session-only nodes and all persisted edges.
3. Labels are safely escaped and graph output remains syntactically valid for representative titles.
4. Rendering does not change persisted workflow bytes.
5. Focused snapshot or exact-output tests cover branching, blocked, optional, completed, and unresolved-node cases.

## Depends on

- Durable workflow persistence `70cd7056-c342-4433-ad60-5bc798f61aa6`.

## Spec

`c677182e-90da-4ac3-8b94-9e2e97c825cf`.