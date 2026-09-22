Tracker for work related to routing session workflow diagnostics upward out of rendering paths and validating workflow graph structure (dangling edges, duplicate node ids) before handoff writes and via the audit pipeline.

Created during review of ticket d1b3a6c9 "Route workflow diagnostics upward and add structural workflow-graph validation" to satisfy the ticket-store graph_participation health check, which flagged that ticket as disconnected from the depends_on graph. This tracker gives standalone session-api/audit-api workflow-validation bug fixes a place to attach rather than remaining isolated nodes.

Linked children:
- d1b3a6c9 "Route workflow diagnostics upward and add structural workflow-graph validation" (in-review)