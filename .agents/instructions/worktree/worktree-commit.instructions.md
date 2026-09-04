---
description: "Use when committing changes inside a worktree-backed implementation task. Covers branch verification before staging and the board-scoped staging rule."
---

## 4. Commit

Per [AGENTS.md](../../../AGENTS.md#quality-gates)'s commit rule, worktree-backed commits land on the feature branch inside the worktree; a small main-checkout change may commit directly to `main` after validation when only its explicit path is staged. [workflow.instructions.md](workflow.instructions.md) still governs staging batches, generated outputs, and message conventions, and [submodule.instructions.md](submodule.instructions.md) still governs deepest-first submodule ordering. Two additions for worktree-backed commits:

- Verify the branch before staging. `git -C <worktree> branch --show-current` must print `agent/<full-session-uuid>/<slug>`. If it prints `main`, stop — the session is in the wrong checkout. If the commit touches a submodule, verify that submodule's branch too, per [worktree-submodule-branch-check.instructions.md](worktree-submodule-branch-check.instructions.md) — a correct top-level branch does not guarantee the submodule is not on `main`.
- Stage only files the board entry claims. `git add -A` from an implementation session is forbidden; it is exactly how an unrelated agent's work gets swallowed.
