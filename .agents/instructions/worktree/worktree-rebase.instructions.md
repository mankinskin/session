---
description: "Use when rebasing a worktree-backed feature branch onto an updated main. Covers worktree-ctl rebase, hand conflict resolution, mandatory re-validation, and the shared stash-guard behavior with merge/sync."
---

## 5. Rebase onto main

Conflicts are resolved by the feature branch, never by the integrator. `worktree-ctl rebase` is submodule-aware: it iterates every affected submodule first (checking out `<name>`'s branch and rebasing it onto that submodule's local `main`, then committing the rebased gitlink), and only then rebases the superproject worktree itself:

```bash
./target/debug/worktree-ctl.exe rebase <name>
```

This rebases `<name>`'s branch (submodules, then the superproject) onto LOCAL `main`; it never fetches `origin`, and it stops on the first conflict and never auto-resolves or aborts, so conflict resolution still happens by hand inside the worktree:

```bash
git -C <worktree> status
# fix conflicts, then:
git -C <worktree> add <resolved files>
git -C <worktree> rebase --continue
```

After the rebase completes, re-run the validation commands from the handoff package. A rebase that compiles is not a rebase that passes — re-validate, do not assume.

If the rebase cannot be completed — a semantic conflict the agent cannot resolve — abort it with `git rebase --abort`, leave the branch as it was, and escalate with the conflicting paths named. Do not mark the branch ready.

`worktree-ctl rebase`, `merge`, and `sync` guard every mutation against an unclean working tree instead of failing on it. By default, an uncommitted or untracked change in the path about to be rebased/merged (a submodule, the worktree, or the main checkout) is stashed with `git stash push --include-untracked` before the operation and popped back afterward, reported as `stashed uncommitted changes in <label> (restored afterward)`. Pass `--auto-commit` to commit the dirty state instead (`worktree-ctl auto-commit before sync`) and carry it along with the rebase/merge rather than stashing it. If restoring a stash afterward genuinely conflicts with what the rebase/merge introduced at the same path, the command reports a non-zero exit naming the stash (`git -C <path> stash list`) rather than silently discarding it — treat that as a manual-resolution case, same as any other conflict.

Once the rebase is clean and validation passes, continue to [worktree-merge.instructions.md](worktree-merge.instructions.md).
