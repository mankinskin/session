## Problem

`git ls-files | grep events.json` returns **76 tracked files totalling ~99MB** under `.session/sessions/**`. These were committed before the current `.gitignore` rule was added. New `events.json` files are correctly ignored (verified via `git check-ignore`), so the repo is in a half-migrated state: forward-tracking is right, history is bloated.

Discovered during the review of `4817a5cc`.

## Decision (user, 2026-07-27)

Remove the tracked blobs with `git rm --cached`. **Do not rewrite history.** The objects stay in the pack; the working tree and future commits are clean. Purging history was explicitly rejected.

## Acceptance criteria

1. All 76 tracked `events.json` files are removed from the index with `git rm --cached` and the removal is committed.
2. No working-tree `events.json` file is deleted from disk.
3. `git ls-files | grep events.json` returns zero results afterwards.
4. `git check-ignore` confirms the files are ignored going forward, so they do not reappear as untracked-then-staged.
5. The commit message records that history was intentionally not rewritten and why.

## Non-goals

- `git filter-repo` / BFG / any history rewrite.
- Changing which artifacts the session store writes.
- Resolving the broader `4817a5cc` git-tracking policy (blocked on the session workspace-model redesign).
