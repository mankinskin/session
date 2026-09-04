---
description: "Use when creating or renaming a session worktree for worktree-backed implementation work. Covers worktree-ctl bootstrap/new, offline submodule population, and the mandatory rename-only-on-scope-change rules."
---

## 1. Bootstrap (implementation agent)

Run from the repository root, on `main`, before dispatching the implementation agent:

```bash
./target/debug/worktree-ctl.exe new <full-session-uuid> <slug>
```

For a worktree that also needs the repository-local stores and generated
Copilot surfaces initialized, use the one-line bootstrap command instead:

```bash
./target/debug/worktree-ctl.exe bootstrap <full-session-uuid> <slug>
```

`bootstrap` creates or reuses the same worktree as `new`, then runs the
worktree's `init.sh`. `new` remains Git/submodule-only for callers that need
to defer repository initialization. Re-running `bootstrap` safely retries a
failed initializer without creating a second worktree; `--dry-run` reports
both actions without modifying either checkout.

This is the canonical invocation — it is the single source of truth for the exact git sequence, so hand-typed variants cannot drift from it. The CLI requires a full UUID and runs: `git worktree add .worktrees/<full-session-uuid>/<slug> -b agent/<full-session-uuid>/<slug> main` (branching directly from LOCAL `main`, no fetch and no origin dependency), then populates every submodule OFFLINE by giving each one its own linked worktree — `git -C <main-checkout>/<submodule> worktree add --detach .worktrees/<full-session-uuid>/<slug>/<submodule> <recorded-sha>` — rolling back the partial worktree on persistent failure. Local `main` is authoritative here, not `origin/main`: this repo's local `main` and its recorded submodule commits are routinely ahead of, or entirely absent from, `origin`, so origin is never a valid source for either. Pass `--dry-run` to print the exact commands without running them.

The branch is always cut from `main`, never from another feature branch. If the branch already exists, the worktree is being re-created for an interrupted task — use `git worktree add .worktrees/<full-session-uuid>/<slug> agent/<full-session-uuid>/<slug>` without `-b`.

Pass the resolved worktree path to the implementation agent in its context bundle. The agent does not derive the path itself.

Never run `git submodule update --init --recursive` inside a linked session
worktree. Git mutates the shared submodule `core.worktree` setting and can
orphan the main checkout or another session's nested checkout. `worktree-ctl`
already creates detached nested submodule worktrees from the recorded gitlinks.

**Before the first edit**, run the recursive branch verification in
[worktree-submodule-branch-check.instructions.md](worktree-submodule-branch-check.instructions.md) — bootstrap leaves every nested submodule detached, not on
the feature branch.

## 1b. Rename Only When Scope Changes

Create the worktree with its final topic slug. Rename only when the task scope materially changes, before the first edit and before claiming ([worktree-claim.instructions.md](worktree-claim.instructions.md)). Run the sequence from the repository root, with no shell or other process using the worktree as its current directory. Before renaming, check for uncommitted tracked changes:

```bash
git -C .worktrees/<name> diff --stat          # unstaged tracked changes
git -C .worktrees/<name> diff --stat --cached # staged tracked changes
```

Both commands must be empty; otherwise commit or stash the tracked changes first. Untracked `.session/sessions/` entries do not block a rename: the capture hook writes those continuously as background noise.

```bash
./target/debug/worktree-ctl.exe rename <full-session-uuid>/<current-slug> <full-session-uuid>/<topic-slug>
```

`git worktree move` is unusable in this repository because every worktree contains five submodule linked worktrees. `worktree-ctl rename` uses filesystem relocation, top-level repair, and branch rename instead.

The ordering is mandatory: `session_check_in` records `worktree_path` and `branch`, with no update surface or topic/slug field in [memory-api/crates/session-api/src/store.rs](memory-api/crates/session-api/src/store.rs). Renaming after check-in strands the stored path and branch.

Verify that the top-level repair kept every submodule populated:

```bash
git -C .worktrees/<full-session-uuid>/<topic-slug> submodule status
```

The output must show all populated submodules: `memory-viewers`, `context-stack`, `memory-api`, `viewer-api`, and `workflow-tools` (which nests `memory-kernel` and the domain repos recursively). Only when fewer are populated, run `git -C .worktrees/<full-session-uuid>/<topic-slug>/<submodule> worktree repair` for each affected submodule. Then `cd` into `.worktrees/<full-session-uuid>/<topic-slug>` and proceed to [Claim](worktree-claim.instructions.md).

### Renaming again when focus changes

Re-renaming is allowed but should be rare: run `./target/debug/worktree-ctl.exe rename <current-name> <target-name>` only when scope materially changes to a different feature or ticket, not for every sub-task. Re-run `session_check_in` with the new `worktree_path` and `branch` so the store is not stale, and run `board_check_in` when the claimed files change. Do not rename with uncommitted tracked modifications, staged or unstaged; commit or stash those first. Untracked `.session/sessions/` entries are capture-hook background noise and never block a rename. Never rename while a viewer, `cargo` build, or another agent has its current directory inside the worktree.
