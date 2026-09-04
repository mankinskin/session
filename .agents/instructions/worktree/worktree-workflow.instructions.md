---
description: "Use when deciding whether a task needs worktree isolation, or as the entry point into the full bootstrap → claim → work → commit → rebase → merge worktree protocol. Covers the decision to isolate, the loop overview with links to each step's focused file, naming, and escalation triggers."
---

## Why

The capture hook never provisions worktrees. For qualifying implementation work, the implementation agent explicitly creates a worktree and registers it with the session tool. [worktree-provisioning.instructions.md](../session/worktree-provisioning.instructions.md) documents the decision and registration policy; this workflow owns the branch, claim, rebase, and merge protocol, split across the focused files below.

Multiple agents editing the same checkout at the same time is the failure mode this protocol exists to prevent: one agent's `cargo fmt`, revert, or `git add -A` silently swallows another agent's in-progress work, and the resulting commit cannot be attributed to either session. A worktree provides structural isolation when a task needs that protection; it is not the default execution mode for every implementation session.

## When This Applies

Use this protocol only after choosing a worktree for concrete isolation: overlapping active file ownership, requester-required branch isolation, or a planned Git operation that needs an independent branch. [AGENTS.md](../../../AGENTS.md#task-routing) makes the main checkout the default after a board check. Ticket size, file count, submodules, and risk alone do not trigger this protocol.

Before this protocol starts, resolve the target repository from the active VS
Code workspace root and verify the candidate with `git -C <candidate> rev-parse
--show-toplevel`. The location of this instruction file, an absolute path in
prompt metadata, a pasted artifact path, or the inherited current directory is
not target-repository evidence. "Repository root" and "main checkout" in every
file this workflow links to mean that verified target repository, never the
repository containing guidance that happened to be loaded.

## The Loop

One worktree-backed implementation task, start to merge. Each step is its own
focused instruction file — load only the step you need instead of the whole
protocol:

1. **[Bootstrap](worktree-bootstrap.instructions.md)** — the implementation agent creates the worktree and branch from `main`, then registers it with `session_check_in`.
2. **[Claim](worktree-claim.instructions.md)** — the implementation agent checks in on the session store and the ticket board.
3. **[Work](worktree-work.instructions.md)** — all edits, builds, and tests for the worktree-backed task happen inside the worktree.
4. **[Commit](worktree-commit.instructions.md)** — commits land on the feature branch, never on `main`.
5. **[Rebase](worktree-rebase.instructions.md)** — in every affected submodule first, then in the superproject, the feature branch rebases onto that repository's updated `main` and resolves every conflict on its own side.
6. **[Mark ready and merge](worktree-merge.instructions.md)** — the agent checks out of the board with a `ready-to-merge:` reason, moves the ticket to `in-review`, then the same session fast-forwards every affected `main` (submodules first, superproject last) and tears its own worktree down.

A task that touches a Git submodule also needs [worktree-submodules.instructions.md](worktree-submodules.instructions.md), and every step above requires the recursive branch check in [worktree-submodule-branch-check.instructions.md](worktree-submodule-branch-check.instructions.md) before its first edit.

All steps belong to the implementation session — the same session that creates the worktree does the work and finishes the merge.

## Naming

| Thing | Form | Example |
|---|---|---|
| Branch | `agent/<full-session-uuid>/<slug>` | `agent/<full-session-uuid>/<slug>` |
| Worktree path | `.worktrees/<full-session-uuid>/<slug>` | `.worktrees/<full-session-uuid>/<slug>` |

`<full-session-uuid>` is the complete session UUID. `<slug>` is a lowercase hyphenated shortening of the task title, 40 characters or fewer. One session, one active slug directory, one branch, one worktree — never two active slug directories for one session UUID.

Use the final topic slug when creating the worktree; no hook-created `session` placeholder exists. Existing flat `.worktrees/<short-id>-<slug>` worktrees remain supported during transition and are not migrated. More than one valid candidate is an `AmbiguousSessionWorktree` error, not a selection.

`.worktrees/` is git-ignored at the repository root. Never commit a worktree directory.

## Escalation triggers

Stop and escalate rather than improvising when:

- `session_check_in` reports a worktree conflict.
- The board shows the ticket actively held by another `agent_id`.
- `git branch --show-current` prints `main`, prints blank (detached HEAD), or
  is not yet checked, at the worktree top level **or in any populated
  submodule, recursively** — see [worktree-submodule-branch-check.instructions.md](worktree-submodule-branch-check.instructions.md).
- A rebase conflict is semantic rather than textual.
- `git merge --ff-only` fails during integration — `main` moved after the branch rebased; rebase again ([worktree-rebase.instructions.md](worktree-rebase.instructions.md)) and retry the merge yourself rather than treating it as someone else's problem.
