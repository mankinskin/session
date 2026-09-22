## Objective
Author a specification for ticket-track, worktree, and file access leases.

## Context
Owner decision: “a lease model over ticket track, worktree, and files. The orchestrator session claims leases; sub-sessions INHERIT the orchestrator's leases. Unrelated host sessions (not sub-agents) hold no lease and their check-ins are rejected LOUDLY, e.g. `Worktree/Ticket/File already in use by session X`. Leases are released automatically by a hook on session finish.”
Audit: `tmp/test-coverage-audit/03-requirements.md`.

## Acceptance criteria
- Define claim, inheritance, conflict, and auto-release semantics.
- Specify the loud conflict message contract.
- Define lease scope and lifecycle.

## Out of scope
- Lease implementation.