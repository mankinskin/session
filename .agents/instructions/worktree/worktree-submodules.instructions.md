---
description: "Use when a worktree-backed task touches a Git submodule. Covers submodule initialization requirements, per-submodule branch cutting, and deepest-first commit/rebase/merge ordering."
---

## Submodules

This repository is a superproject with submodules (`memory-api`, `memory-viewers`, `context-stack`, `viewer-api`, `workflow-tools`), normally tracking `main`. `workflow-tools` nests `memory-kernel` and the domain repos as its own submodules. A new superproject worktree does not populate them, which is why bootstrap initializes them (see [worktree-bootstrap.instructions.md](worktree-bootstrap.instructions.md)). Initialization may leave detached checkouts: attach branches and run the [recursive branch check](worktree-submodule-branch-check.instructions.md) before edits or builds.

When the change touches a submodule:

- Bootstrap must initialize every submodule the build needs, not just the one being edited. The root `Cargo.toml` lists workspace members inside several submodules, so `cargo` fails to load the workspace with `failed to read <submodule>/Cargo.toml` if any are left uninitialized.
- Cut a matching `agent/<full-session-uuid>/<slug>` branch inside that submodule's checkout within the worktree before editing it. Its intermediate superproject ancestors must also be on feature branches; untouched children may remain on `main`. Update the immediate parent's `.gitmodules` branch tracking to match and retain `update = merge` or `rebase`. Verify with the recursive check in [worktree-submodule-branch-check.instructions.md](worktree-submodule-branch-check.instructions.md) — detached checkouts are never a supported working state.
- Commit the submodule first, then the superproject pointer — the deepest-first rule in submodule.instructions.md (`../../../../.agents/instructions/repository/submodule.instructions.md`) is unchanged.
- Rebase ([worktree-rebase.instructions.md](worktree-rebase.instructions.md)) and merge ([worktree-merge.instructions.md](worktree-merge.instructions.md#bottom-up-integration-sequence-canonical)) apply to the submodule branch too: follow the canonical bottom-up sequence there.
