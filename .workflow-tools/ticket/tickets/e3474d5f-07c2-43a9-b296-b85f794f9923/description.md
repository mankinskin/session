## Problem

`session-api`/the `session` CLI has no `prune`/`gc` subcommand for dangling
session records. This session alone had 5 dangling session records that had
to be deleted by hand from the store because no prune/gc command exists —
there is no supported way to detect or remove orphaned/dangling
`session.json` entries short of manual filesystem surgery.

This ticket is scoped to the tooling gap only: adding a `prune`/`gc`
subcommand. It does not cover investigating the root cause of why
`session.json` records go missing/dangling in the first place — that is
explicitly out of scope here.

## Acceptance Criteria

- A `session` CLI subcommand (`prune` or `gc`) exists that detects dangling
  session records in the store (e.g. records with no corresponding
  `session.json` or other integrity-broken state) and removes them.
- The subcommand supports a dry-run/preview mode that reports what would be
  removed without deleting anything, mirroring the preview/apply pattern used
  elsewhere in this repo's tooling (e.g. `board_clean_preview` /
  `board_clean_apply`).
- The equivalent capability is exposed via `session-mcp` as a named tool, not
  only the CLI.
- Removing a dangling record is logged/reported so the operator can see what
  was pruned and why it was judged dangling.
- A regression test reproduces a dangling-record scenario (a session record
  with no backing `session.json`) and asserts the prune/gc command detects
  and removes it, and that a live/valid session record is left untouched.

## Context

Discovered during this session: 5 dangling session records had to be deleted
by hand from the `.session` store because no prune/gc command existed to do
it safely.

## Depends On

None recorded yet.

## Blocks

Nothing yet.