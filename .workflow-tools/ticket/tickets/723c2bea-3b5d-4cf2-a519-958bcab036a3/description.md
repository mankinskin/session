## Objective

Prevent worktree submodule population from writing `core.worktree` into shared submodule configuration, where the most recently populated worktree can hijack every checkout.

## Problem

When a worktree submodule is populated, Git writes `core.worktree` into the shared `.git/modules/<name>/config` instead of a per-worktree configuration. The observed value in `.git/modules/memory-api/config` was `../../../../../.worktrees/5cd0b38e-session/memory-api`, a path relative to the shared submodule gitdir that the main checkout also reads. The incident occurred after the affected worktree directories were deleted manually with `rm -rf`.

Even before deletion, the latest worktree population silently redirects the shared key to that worktree. After a manual or otherwise non-Git removal, the key dangles: root `git status` fails with `fatal: cannot chdir` and then a submodule `git status --porcelain=2` failure; `git -C memory-api worktree prune` fails with the same error; and `worktree.sh new` cannot create any worktree. The failure blocks every agent using the repository.

Stale `.git/modules/<name>/worktrees/<entry>/gitdir` registrations are a secondary, milder symptom. Removing those entries alone was insufficient. The recovery that worked was unsetting `core.worktree` from each affected shared config, then pruning the submodule and root worktree registrations. The verified steady state has `core.worktree` absent for all five populated submodules: `memory-viewers`, `context-stack`, `memory-api`, `viewer-api`, and `memory-kernel`.

## Scope

Ensure worktree population keeps shared submodule configuration free of hijacking `core.worktree` values; provide supported recovery; and extend `doctor`. `doctor` currently checks only stale registrations, not `core.worktree`. Because prune cannot self-heal a dangling key, `doctor` is the load-bearing detection surface. Related work: ticket 72314c5e (worktree `rename`) and ticket 5e6cf4f8 (Rust rewrite of `worktree.sh`), where the `doctor` implementation will most likely land.

## Done

The requirements part passes with regression coverage for shared configuration isolation, manual deletion resilience, supported recovery, and `doctor` diagnostics.
Landed: shared-config core.worktree hijack repair recorded under ticket 5e6cf4f8.