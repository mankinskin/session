---
description: "Use when claiming a worktree-backed implementation task before its first edit. Covers the session_check_in and board_check_in claim sequence, conflict escalation, and the working-tree baseline."
---

## 2. Claim (implementation agent, before the first edit)

Two claims, both required, in this order.

Resolve session identity and use the closing traceability footer described in [session-identity-and-handoff.instructions.md](../session/session-identity-and-handoff.instructions.md).

**Session claim** — records the authoritative session-to-worktree-to-branch assignment and rejects a second session claiming the same worktree:

```
session_check_in {
  workspace: "default",
  session_id: "<this session id>",
  owner_id: "<agent id>",
  ticket_id: "<full ticket uuid>",
  worktree_path: "<path to .worktrees/<full-session-uuid>/<slug>>",
  branch: "agent/<full-session-uuid>/<slug>"
}
```

**Board claim** — records ticket and file ownership so other agents can see the scope is taken:

```
board_check_in {
  workspace: "default",
  ticket_id: "<ticket id>",
  agent_id: "<agent id>",
  intent: "branch=agent/<full-session-uuid>/<slug> worktree=.worktrees/<full-session-uuid>/<slug> — <one-line intent>",
  files: ["<repo-relative path>", "..."]
}
```

A board entry has no dedicated branch column, so the branch and worktree ride in the `intent` prefix in exactly the `branch=… worktree=… — …` form above. That prefix is what a later reader greps to answer "which branch holds this ticket's work".

If `session_check_in` reports a worktree conflict, or the board shows the ticket already actively held by a different `agent_id`, **stop and escalate**. Do not proceed on a shared worktree.

After both claims succeed, persist `git status --short` as the session's
worktree baseline. Later commit, review, and handoff reports classify changes
relative to that baseline; they do not attribute every dirty path to the active
agent. Refresh the baseline only after a committed checkpoint, and retain the
previous checkpoint pointer.

Before the first edit, also run the recursive submodule branch check in
[worktree-submodule-branch-check.instructions.md](worktree-submodule-branch-check.instructions.md) — a successful claim does not by itself prove every submodule is on the expected branch.
