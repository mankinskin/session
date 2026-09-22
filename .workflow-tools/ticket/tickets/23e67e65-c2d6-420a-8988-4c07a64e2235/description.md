## Incident

The context-engine superproject has five submodules: `context-stack`, `memory-api`, `memory-kernel`, `memory-viewers`, and `viewer-api`. A session merged several `agent/*` branches into superproject `main` while merging only the superproject side. The submodule-side branches were never merged into submodule `main`, leaving superproject gitlinks pointing at commits on no submodule branch.

The incident affected `memory-api` as a four-commit orphan line while submodule `main` advanced independently, `viewer-api` as a one-commit unbranched gitlink, and `memory-viewers` as a two-commit unbranched gitlink. Recovery required manual rescue branches and a real merge across two follow-up sessions.

Root cause: the superproject gitlink was treated as the record of truth while the submodule own branch was left behind.

Documentation is already corrected in commit `30b541b0`; this ticket is tooling-only.

## Tooling Gap

`tools/worktree/worktree-ctl` runs `worktree-ctl rebase` only in the superproject feature worktree, never rebasing the same-named `agent/*` branch in affected submodules. `worktree-ctl merge` fast-forwards nested submodule branches but does not verify the gitlink containment invariant: every gitlink recorded by the superproject must be contained in that submodule `main`.

The implementation must preserve the crate split: use `git2` for reads and shell out to `git` only for writes that libgit2 cannot express. Both subcommands already support `--dry-run`.

## Acceptance Criteria

1. `worktree-ctl rebase <name>` rebases the same-named `agent/*` branch in every affected submodule onto that submodule `main` before rebasing the superproject branch, and stops with a clear error on conflict rather than continuing.
2. `worktree-ctl merge <name>` refuses to merge the superproject branch unless every recorded gitlink is contained in the corresponding submodule `main`, reporting each violating submodule by name and SHA.
3. The gitlink containment invariant is re-verified after the superproject merge and a violation is reported as a failure.
4. A submodule whose branch is unmerged blocks the superproject merge with an actionable message naming the submodule and branch.
5. `--dry-run` on both subcommands prints the full planned per-submodule sequence without mutating anything.
6. Tests cover a clean bottom-up merge; an orphan gitlink recorded SHA on no branch being rejected; a backward gitlink where submodule `main` is ahead of the recorded SHA being detected; and dry-run producing no mutation.
7. Existing `worktree-ctl` tests continue to pass; the suite was 28 tests at last count.

## Out of Scope

- Repairing currently stranded branches.
- Changing documentation already corrected in `30b541b0`.
- Any change to the five submodules themselves.

## Notes

The test fixture will need to construct a real temporary superproject with submodules, which may not exist in the current test harness. Constructing that fixture is likely the largest single piece of work in this ticket.
## Update

Merged `agent/23e67e65-worktree-ctl-invariant` into `main` at `8ed1b6f8e21632544192dd604160c7f954b1e2a9`.

Validation: `cargo test -p worktree-ctl` passed 35 tests.

Documentation was already handled separately in commit `30b541b0`.

Landed: bottom-up submodule integration and gitlink containment merged to main; 35 tests pass.