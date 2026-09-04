---
description: "Use when authoring, reviewing, or implementing durable session workflow graphs with session_workflow tools, node kinds, categories, entity anchors, or batch mutations."
---

## Session Workflow Authoring

Use the durable session workflow graph for multi-step work whose progress,
dependencies, or resume state should remain inspectable.

### Node Model

`kind` is a closed behavioral enum because finish and handoff logic branches on
it:

| `kind` | Behavior | Required side data |
|---|---|---|
| `ticket` | Gates on authoritative ticket state | `ticket_urn` |
| `spec` | Gates on authoritative spec state | `spec_urn` |
| `validation` | Gates on authoritative validation outcome | `validation_spec_id` |
| `task` | Descriptive work; local status only | none |

For custom labels, keep `kind="task"` and set the free-text `category`. Do not
invent a new `kind`. For example, a review criterion is
`kind="task", category="review-criterion"`.

`requirement` is `required` or `optional`. Node status is `pending`,
`in-progress`, `blocked`, `done`, or `deferred`.

### Entity References

- `ticket_urn` belongs only on `kind="ticket"` and participates in finish
  gating.
- `spec_urn` belongs only on `kind="spec"` and participates in finish gating.
- `anchor_urn` accepts a ticket or spec URN on any node kind. It preserves
  context and resumability but never participates in finish gating.
- Pin an entity when it belongs to the session-wide context rather than one
  workflow node.

Do not overload a gating URN to attach context to a task or validation node.

### Batch Mutations

Prefer `session_workflow_add_nodes` and `session_workflow_add_edges` when adding
multiple nodes or links. Each tool validates the full array and persists once;
one bad element rejects the whole batch and reports `nodes[index]` or
`edges[index]`. Duplicate node IDs and duplicate edges retain single-item
no-op behavior.

Use single-item tools for isolated mutations. Add all nodes before adding edges
that reference them.

### Known Rejections

| Rejection | Fix |
|---|---|
| Invalid node kind | For a custom label, use `kind=task` with `category="<your-label>"`. |
| Invalid requirement | Use `requirement=required` or `requirement=optional`. |
| Invalid edge kind | Use `kind=depends-on` or `kind=order`. |
| Invalid node status | Use a legal status, for example `status=in-progress`. |
| Non-ticket node sets `ticket_urn` | Use `anchor_urn` for a non-gating reference, or pin the ticket. |
| Non-spec node sets `spec_urn` | Use `anchor_urn` for a non-gating reference, or pin the spec. |
| Edge references an unknown node | Add both endpoint nodes first, then add the edge. |

### Review Criteria Example

Persist criteria as anchored task nodes in one atomic call:

```json
{
  "workspace": "<workspace>",
  "workspace_session_id": "<workspace-session-id>",
  "nodes": [
    {
      "node_id": "criterion-correctness",
      "kind": "task",
      "category": "review-criterion",
      "requirement": "required",
      "title": "Verify behavioral correctness",
      "anchor_urn": "ce://context-engine/tickets/<ticket-id>"
    },
    {
      "node_id": "criterion-tests",
      "kind": "task",
      "category": "review-criterion",
      "requirement": "required",
      "title": "Verify focused test coverage",
      "anchor_urn": "ce://context-engine/tickets/<ticket-id>"
    }
  ]
}
```

Then link the criteria in one atomic call:

```json
{
  "workspace": "<workspace>",
  "workspace_session_id": "<workspace-session-id>",
  "edges": [
    {
      "from": "criterion-tests",
      "to": "criterion-correctness",
      "kind": "depends-on"
    }
  ]
}
```

Update criterion status with `session_workflow_set_status`; do not replace the
task with a ticket or validation kind unless its finish-gating semantics truly
change.
