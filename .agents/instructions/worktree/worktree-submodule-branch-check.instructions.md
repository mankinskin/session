---
description: "Use before edits or builds in a worktree-backed task, and after submodule initialization, to verify recursive attached branches and feature branches for task-owned edits."
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

The top level must print `agent/<full-session-uuid>/<slug>`. Every nested
checkout must be attached: an immediate parent on `main` requires child
`main`; a feature parent permits child `main` or a feature branch. Untouched
children may remain on `main`. Before editing a child in this worktree,
cut/checkout its matching session feature branch and make sure every ancestor
between it and the worktree root is also on a feature branch. Never edit
first and repair branches afterward. A blank line is detached HEAD: stop and
attach the intended branch without discarding work or moving commits backward.
The general configuration and gitlink checks remain in the
[submodule contract](https://github.com/mankinskin/workflow-tools/blob/main/.agents/instructions/repository/submodule.instructions.md#branch-integration-and-gitlinks).

This check is not one-and-done: re-run it after every `git submodule
update --init <name>` performed mid-session (a submodule initialized late,
to satisfy a workspace member list, starts detached at its recorded SHA and
needs the same branch verification before it is safe to edit).
