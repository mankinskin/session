## Objective

Make ticket-api and session-api store resolution deterministically anchor to the
repository-root store, instead of enumerating every `.ticket` / `.session`
directory found under `.worktrees/*`.

## Observed behavior

The resolver treats each git worktree's nested store as an equal candidate.
Two distinct failure modes result:

1. `ticket-mcp` and `session-mcp` refuse outright with
   `workspace selector 'default' for session '<id>' is unanchored; refused to
   select a store from candidates: <16 paths>` — one root store plus one per
   worktree.
2. The `ticket` CLI, invoked from the repository root, silently resolves to a
   worktree store (observed: `.worktrees/fdd059ed-command-agent-execution/.ticket`)
   and writes there. `--workspace` does not correct this; a separate
   `--index-root` / `TICKET_INDEX_ROOT` knob governs the index.

`session_check_in` cannot bootstrap out of failure mode 1: the anchor check
runs before the tool body, so the call that would create the assignment is
itself rejected for having no assignment.

## Impact

Blocked one implementation worker outright, forced every sub-agent onto
disk-fallback for ticket reads, and blocked all ticket-store writes for a
full iteration. Promoted ahead of ticket 5e6cf4f8 by user decision on
2026-08-08 because no further epic progress can be recorded until it is fixed.

## Acceptance criteria

- Store resolution never treats a `.worktrees/*/` nested store as a candidate
  when resolving from the repository root.
- `--workspace <repo-root>` alone is sufficient to pin both the store and the
  index; `--index-root` follows `--workspace` by default, while an explicitly
  passed `--index-root` overrides the default.
- `session_check_in` can create a worktree assignment for a session that has
  none, without the anchor precheck rejecting it first.
- A session running in `.worktrees/<name>` still resolves to that worktree's
  store when that is the intended target.
- A repository-root invocation resolves to the root store without enumerating
  sibling `.worktrees/*` stores, then is rejected by the existing main-checkout
  guard with the main-checkout reason and instructions to check in to a
  worktree, rather than a ~16-entry candidate list.

## Rescoping note (2026-08-08)

Main-checkout mutations remain blocked. The original acceptance criterion 5
was withdrawn because it contradicted commit `4637548f` ("feat(session): route
MCP calls through active worktrees so tool operations stay scoped to the
initiating checkout"), which deliberately rejects a resolved target equal to
the repository root via `MainCheckoutMutationBlocked`.

Blocking reads as well as writes was reviewed and confirmed intentional; no
read/write classification will be added to the guard. The guard is named for
mutations but also rejects `read_file`, and that behavior was accepted as-is by
user decision.

The fix is proxy-only (`mcp-toolmon`). The known accepted limitation is that
the `ticket`/`session` CLI bootstrap path remains broken and is not addressed
by this ticket. Closing that gap requires a separate ticket.

Governing spec authored for this work:
`5d9e5a99-74b5-4f4e-9e6f-0a6cc3741ddf`.
