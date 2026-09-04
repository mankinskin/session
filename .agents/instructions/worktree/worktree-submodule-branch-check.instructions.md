---
description: "Use before the first edit inside a worktree-backed task, and again after any mid-session submodule init, to verify every submodule — recursively — is checked out on the expected feature branch rather than main or a detached HEAD."
---

## Why this check exists

`worktree-ctl bootstrap`/`new` (see [worktree-bootstrap.instructions.md](worktree-bootstrap.instructions.md)) populate nested submodules **detached at a
recorded SHA**, not on the feature branch — a submodule only lands on
`agent/<full-session-uuid>/<slug>` once the agent explicitly checks out or
creates that branch inside it, per "Cut a matching branch inside that
submodule's checkout" in [worktree-submodules.instructions.md](worktree-submodules.instructions.md). A worktree whose top-level `git branch
--show-current` correctly prints the feature branch can still have every
nested submodule sitting on `main` (or detached), and edits made there commit
straight to that submodule's `main` — exactly the failure this check exists
to prevent, discovered when spec-api and session-api changes landed on their
submodules' `main` branches while the superproject worktree looked correctly
isolated.

## The check

Before making any edit inside the worktree — and again immediately after
initializing any submodule not populated at bootstrap time — run a **recursive**
branch check from the worktree root:

```bash
git -C <worktree> branch --show-current
git -C <worktree> submodule foreach --recursive 'echo "$sm_path: $(git branch --show-current)"'
```

Every line must print `agent/<full-session-uuid>/<slug>` (top level) or a
name containing that same branch (nested submodules may use their own
`agent/<full-session-uuid>/<slug>` cut inside that submodule, but never
`main` and never blank/detached). A blank line means detached HEAD; `main`
means the submodule was never switched. Either result is an escalation
trigger per [worktree-workflow.instructions.md](worktree-workflow.instructions.md#escalation-triggers) — stop and cut/checkout the matching branch in that
submodule before editing anything under it, do not edit first and fix
after.

This check is not one-and-done: re-run it after every `git submodule
update --init <name>` performed mid-session (a submodule initialized late,
to satisfy a workspace member list, starts detached at its recorded SHA and
needs the same branch verification before it is safe to edit).
