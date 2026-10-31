---
description: "Use when marking a worktree-backed branch ready and merging it into main. Covers the bottom-up submodule-first integration sequence, the gitlink containment invariant, worktree-ctl sync, and worktree teardown."
---

## 6. Mark ready, then merge (implementation session)

Only after the rebase is clean and validation passes (see [worktree-rebase.instructions.md](worktree-rebase.instructions.md)):

```
board_check_out {
  workspace: "default",
  ticket_id: "<ticket id>",
  agent_id: "<agent id>",
  handoff_reason: "ready-to-merge: agent/<full-session-uuid>/<slug> @ <commit sha> — rebased onto local `main`, <validation command> passed"
}
```

Then move the ticket to `in-review`. The `ready-to-merge:` prefix is the marker recorded in `board_history` documenting that the branch was integrable at that point, even though this same session now proceeds to merge it immediately rather than waiting for a separate session to pick it up.

Immediately continue into 7 (Merge) in the same session — do not stop after marking ready. There is no handoff to a different orchestrator session for the merge itself; the implementation session that produced and validated the branch is also the one that integrates it, precisely because it is the only session that has seen the rebase resolve cleanly and the validation pass.

## 7. Merge (same implementation session)

**The implementation session merges its own branch into `main`.** Do not wait for or delegate to a separate root-orchestrator session to perform this step — do it directly, right after step 6, while validation is still fresh.

### Bottom-up integration sequence (canonical)

The superproject records submodules as gitlinks. During feature work, every
gitlink must be contained in the child's selected attached branch; a feature
superproject may reference feature-child commits. **Before and after integration
into superproject `main`, every gitlink must be contained in child `main`.**
Recursively, an immediate parent on `main` requires child checkouts on `main`;
a feature parent permits `main` or feature children. See the
[branch/tracking contract](https://github.com/mankinskin/workflow-tools/blob/main/.agents/instructions/repository/submodule.instructions.md#branch-integration-and-gitlinks).
Commit hashes remain Git's storage format, not a substitute for branch
attachment. The prior failure left orphaned commits reachable only through
gitlinks and vulnerable to `git gc`.

Never merge the superproject branch while any affected submodule branch is unmerged. For a change spanning submodules and the superproject, run this sequence from the superproject root:

1. **Pin before rewriting history.** In each affected submodule, pin the feature tip before rebasing: `git -C <sm> branch rescue/pre-rebase-<short-sha> <sha>`. Also pin any recorded gitlink that will be superseded: `git -C <sm> branch rescue/gitlink-<short-sha> <gitlink-sha>`.
2. **Integrate each affected submodule, deepest first.** Rebase `agent/<full-session-uuid>/<slug>` onto that submodule's `main`, resolve conflicts there and validate it. Before switching any child superproject to `main`, integrate its descendants and restore its `.gitmodules` tracking to `main` with `update = merge` or `rebase`; commit those task-owned changes on its feature branch. Then fast-forward its authorized main checkout: `git -C <sm> checkout main && git -C <sm> merge --ff-only agent/<full-session-uuid>/<slug>`. Do not force-switch a branch occupied by another linked worktree.
3. **Bump gitlinks on the superproject feature branch.** Run `git add <sm>` for every merged submodule and stage the task-owned `.gitmodules` changes restoring child `main` tracking. Commit these updates, so the feature branch records each submodule's new `main` tip and tracking configuration. Resolve local overrides too.
4. **Verify containment before the superproject merge.** Run the recursive invariant check below; every declared submodule, including nested and untouched children, must pass. Also verify branch attachment and indexed tracking under the submodule contract.
5. **Rebase and integrate the superproject last.** Rebase `agent/<full-session-uuid>/<slug>` onto superproject `main`, resolve conflicts on the feature branch, then fast-forward superproject `main` with `git merge --ff-only agent/<full-session-uuid>/<slug>`.
6. **Re-verify after the superproject merge.** Repeat the recursive containment, attached-branch and tracking checks. Every parent and child in the integrated main checkout must be on `main`.

```bash
rtk git submodule status --recursive
rtk git submodule foreach --recursive '
  branch=$(git symbolic-ref --quiet --short HEAD) || exit 1
  test "$branch" = main || {
    echo "BAD $displaypath: expected main, found $branch" >&2
    exit 1
  }
  git merge-base --is-ancestor "$sha1" refs/heads/main || {
    echo "BAD $displaypath: recorded gitlink $sha1 not contained in main" >&2
    exit 1
  }
  echo "ok $displaypath $sha1"
'
```

`git submodule status --recursive` prefixes `+` for a checkout differing from
the recorded gitlink, `-` for an uninitialized child, and `U` for conflicts.
Reject these markers at the clean-integration gate. `foreach` skips
uninitialized children, so its success never waives the status check.
Neither command validates `.gitmodules` tracking or local overrides; run
the branch/tracking guard too. Rescue branches preserve history but do not
authorize a checkout under a parent on `main`.

`worktree-ctl merge` automates the nested fast-forwards and enforces the containment invariant itself (it checks gitlink containment before and after, and refuses an unmerged submodule branch), but the manual sequence above remains authoritative for the rescue-branch pinning step, which no `worktree-ctl` subcommand performs.

`worktree-ctl merge` (and therefore `sync`) also self-heals a gitlink violation when there is exactly one safe resolution: if a submodule's recorded gitlink is a strict fast-forward ahead of that submodule's local `main` — reachable only through a detached HEAD or an unnamed commit, with no divergence to arbitrate — it fast-forwards `main` to the recorded commit automatically and reports `auto-fixed gitlink: <submodule> local main fast-forwarded to <sha>`; `--dry-run` reports the same action as `auto-fix gitlink: ...` without mutating anything. A violation is left for manual resolution whenever the fix is not unique: true divergence (recorded commit and local `main` share no fast-forward path) or a recorded commit missing from the submodule's object database still error out exactly as before.

Containment repair alone is not branch-policy compliance. After any tool
repair, attach the authorized checkout branches and recheck tracking and
recursive parent/child rules before further work.

### One-command shorthand: `worktree-ctl sync`

For the common case — no rescue-branch pinning needed — `worktree-ctl sync <name>` composes step 5 (rebase, see [worktree-rebase.instructions.md](worktree-rebase.instructions.md)) and step 7 (merge) behind one command: it rebases every affected submodule then the superproject worktree onto local `main`, and only if that rebase succeeds does it fast-forward every affected submodule `main` then the superproject `main`. A rebase conflict stops `sync` before any merge is attempted — resolve the conflict by hand inside the worktree (`git -C <path> add <resolved files> && git -C <path> rebase --continue`), then rerun `worktree-ctl sync <name>` to finish the remaining rebase steps and merge. `--dry-run` prints the full combined plan (every submodule rebase/skip, the superproject rebase, every submodule fast-forward/skip, and the superproject fast-forward) without mutating anything.

Because every affected branch has rebased onto its repository's `main`, each integration is a fast-forward and must be asserted as one:

```bash
git -C <sm> checkout main && git -C <sm> merge --ff-only agent/<full-session-uuid>/<slug>
git checkout main && git merge --ff-only agent/<full-session-uuid>/<slug>
```

If any `--ff-only` fails, the target `main` moved after the branch rebased. Do not merge — send the branch back through [worktree-rebase.instructions.md](worktree-rebase.instructions.md) for a fresh rebase. Never resolve a conflict on `main`.

Tear down after a successful merge:

```bash
./target/debug/worktree-ctl.exe remove <name>
```

This runs `git worktree remove --force .worktrees/<full-session-uuid>/<slug>`, `git worktree prune`, then `git branch -d agent/<full-session-uuid>/<slug>`. The session-UUID parent directory is removed only when empty, so a sibling slug preserves the parent.

`worktree-ctl remove` refuses a dirty worktree unless `--force` is explicit. Its successful force path uses `git worktree remove --force`, which bypasses Git's refusal to remove initialized submodules once the branch is confirmed merged (the fast-forward above already proves it) and needs no prior deinit step. The CLI uses `-d`, never `-D`, so git refuses to delete a branch that was not actually merged.

One sharp edge to avoid, not perform: never run `git submodule deinit` inside a linked worktree. It rewrites `submodule.*` in `.git/config`, which is **shared by every worktree of the repository**, so deinitializing inside the worktree silently deinitializes the main checkout too. `--force` on `git worktree remove` handles initialized submodules directly and makes that deinit step both unnecessary and harmful — do not add it back. If a submodule deinit ever happens by accident (a hand-typed command, another tool), repair with `git submodule init && git submodule update --init --recursive` in the main checkout and confirm with `git submodule status` that no entry carries a `-` prefix; working-tree files are not lost when this happens, only the config registration. `worktree-ctl doctor` diagnoses and repairs exactly this state. Every mutating `worktree-ctl` subcommand (`new`, `rebase`, `merge`, `sync`, `remove`, `rename`, `finish`, `doctor`) must be run from the main checkout, never from inside a linked worktree.
