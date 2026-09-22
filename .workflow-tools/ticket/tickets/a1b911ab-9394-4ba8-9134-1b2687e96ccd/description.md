## Objective

Let an MCP server resolve `session_id` to the worktree that session should write
into, using only the session id plus a one-time filesystem discovery — no
hand-managed routing file.

## Background

`session-workspace-resolver` used to read a machine-local side-car index at
`.session-routing/worktree-index.json`. That index has been deleted; resolution
is now anchored on the process working directory. Because every MCP server is
launched once at the main checkout and never restarted per session, every
session resolved to the main checkout regardless of which worktree it owned.
The plumbing carrying `session_id` through the proxy was already in place end
to end; only the lookup body needed a real implementation.

Hard-failing on a missing assignment was also a deadlock: a session could not
check in until it had resolved, and could not resolve until it had checked in.

## Scope

Resolution only. Worktree lifecycle, recycling, eager creation in the
`UserPromptSubmit` hook, and the rewrite of `worktree.sh` as a Rust binary are
tracked on 5e6cf4f8, which depends on this ticket.

## Acceptance Criteria

1. Given a session whose worktree exists at `.worktrees/<short-id>-<slug>`, a
   `tools/call` carrying that `session_id` resolves to that worktree, with no
   routing index present anywhere on disk.
2. Discovery uses a glob fast path on `.worktrees/<short-id>-*`; exactly one
   match resolves, zero matches falls through to a scan of `.worktrees/*/` for
   `.session/sessions/<session_id>/session.json`.
3. Two or more glob matches fail with a distinct named error. No arbitrary
   choice is ever made between candidates.
4. Successful discovery is cached for the proxy process lifetime; a second call
   for the same session performs no additional filesystem walk. Misses are not
   cached, since the hook may create the worktree immediately afterwards.
5. When nothing is discovered, resolution fails with `MissingSessionWorktree`.
   Resolution never silently falls back to the main checkout.
6. A worktree assignment recorded in the session store always takes precedence
   over a discoverable worktree.

## Non-Goals

- Worktree creation, locking, reclamation, or reuse. See 5e6cf4f8.
- Reaping the pre-existing orphaned `.worktrees/` directories that git no
  longer registers.
- Any change to the merge protocol or the root-orchestrator merge monopoly.

## Live verification, 2026-08-07

End-to-end verification in worktree `.worktrees/70abae1b-session-worktree-discovery` on branch `agent/70abae1b-session-worktree-discovery`:

- Discovery resolves a session to its worktree with no routing index, no explicit workspace argument, and no session record present. Confirmed via a ticket read and a board check-in that both landed in the worktree store.
- Resolver suite: 24 passed, 0 failed.

### Defect found and fixed: a hook-written main-pointing record defeated discovery

The capture hook infers the worktree from its own working directory, which is always the main checkout, and writes `worktree.path = <main checkout>, branch = main` on every `UserPromptSubmit`. The rule "a recorded assignment always wins over a discoverable worktree" then treated that inference artifact as authoritative, resolved the session to the main checkout, and the guard refused every call with `main checkout mutations are blocked`. The session was locked out of its own tool surface, self-healing only in the window between deleting the record and the next user prompt.

Fix applied: a receipt whose path refers to the same directory as the main checkout is distrusted and discovery is attempted instead; if nothing is discoverable the receipt is still honored, preserving prior behavior. A receipt pointing at any non-main path keeps winning over discovery, unchanged.

### AC 7 (new, DONE)

A recorded assignment that points at the main checkout does not defeat discovery. Covered by tests `a_main_pointing_record_does_not_defeat_discovery` and `a_main_pointing_record_is_honored_when_nothing_is_discoverable`.

### AC 8 (new, OPEN) — guard scope

When a session resolves to the main checkout, the guard currently refuses every call, including plain reads, which is what turned a mis-routed session into a total lockout. Decision taken: refuse mutations only and let reads through. There is currently NO read/write classification anywhere in `session-workspace-resolver` or `mcp-toolmon` — the proxy blocks unconditionally before any notion of mutation exists — so this requires introducing one. It must be deny-by-default: anything not positively classified as a read is treated as a mutation.

### Open design conflict — explicit workspace override

A decision was taken that an explicit `workspace` argument should override the guard. This conflicts with an existing deliberate invariant: `ResolveRequest::relative_workspace` is documented "It is never a selector", is confined to a path relative to the already-resolved worktree, and an absolute value is rejected via `AbsoluteRelativeWorkspace`. That invariant exists to stop a caller escaping its assigned worktree. Implementing the override as stated would invert it. NOT implemented pending a decision on whether the override should be constrained to worktrees already belonging to the same session rather than an arbitrary path.
## Decisions, 2026-08-07 (supersede earlier answers in this ticket)

**Explicit workspace override: DROPPED.** `ResolveRequest::relative_workspace` stays "never a selector". The session id remains the sole selector for which worktree a call resolves to, and an absolute `workspace` value stays rejected. Rationale: the containment invariant is what stops a session reaching into a sibling agent's worktree, and the deadlock that motivated an escape hatch was fixed by distrusting main-pointing records instead.

**AC 8 guard scope: REVERSED to block everything.** When a session resolves to the main checkout, ALL tool calls are refused, reads included. Rationale: allowing reads would let an agent silently consume state from the wrong checkout, which is a worse failure than a loud refusal. No read/write classification is introduced, and none is needed. AC 8 therefore requires NO code change — the existing unconditional block is the intended behavior and should be documented as deliberate rather than treated as a defect.

**Anchor durability caveat.** The capture hook rewrites the session record on every prompt with `worktree.path = <main checkout>, branch = main`, observed at 12:39, 12:55 and 13:23 on 2026-08-07. Any explicit anchor written by `session check-in` is therefore transient and will be clobbered by the next prompt. Correct routing currently depends on the resolver distrusting main-pointing records, not on the anchor persisting. Making the hook record the real worktree is tracked on ticket 5e6cf4f8-120c-4674-95de-d7b79c99f5b3.
### Finding: a hook-written session record cannot be corrected by `session check-in`

Attempting to record a correct anchor for session `70abae1b-14c4-4033-9265-d37fe08b02b2` failed twice with:

```
session error: session 70abae1b-14c4-4033-9265-d37fe08b02b2 ownership mismatch for worktree check-in
```

This occurred with `--owner-id copilot-agent-70abae1b` and again with `--owner-id copilot-agent`, the latter matching the existing record's `metadata.agent_id` exactly. A `--predecessor-session-id` rotation variation failed identically. Both the main store and the worktree store were left unchanged.

The consequence is that a session record written by the capture hook — which always points at the main checkout — cannot be repaired through the supported CLI path. The ownership guard rejects even the identity that wrote the record. Correct routing therefore rests entirely on the resolver distrusting main-pointing records; there is currently no way to author a correct explicit anchor. The ownership check needs to either accept the recording identity or expose a supported takeover path.
## Correction, 2026-08-07: the hook preserves, it does not rewrite

An earlier note in this ticket claimed the capture hook rewrites `metadata.worktree` on every prompt. That is incorrect. `memory-api/crates/session-api/src/store/config/worktree_capture_inference.rs:25` early-returns with `if record.metadata.worktree.is_some() { return Ok(()); }`, and the persistence merge at `helpers/storage.rs:251` prefers `incoming.worktree.or(existing.worktree)`. The hook only infers a worktree when none is recorded. The main-pointing value was inferred once, during the first capture in the main checkout, and has been faithfully preserved since; the refreshing `captured_at` timestamps were mistaken for rewrites.

The practical consequence is the opposite of what was recorded: a CORRECT assignment, once written, is also preserved indefinitely. Bootstrapping a session therefore only requires getting the right value in place once.

### Defect: a hook-written record is permanently unclaimable

`check_in_worktree` (`memory-api/crates/session-api/src/store/config/worktree_runtime.rs:15`) rejects unless BOTH hold:

```
existing_record.metadata.agent_id  == request.owner_id
existing_record.metadata.ticket_id == request.ticket_id
```

A hook-written record carries no `ticket_id`, while `validate_worktree_request` (`helpers/storage.rs:262`) requires the caller to supply a non-empty one. The second comparison is therefore `None != Some(..)` for every possible caller, so no caller can ever claim a hook-created assignment. The only way through today is to delete the record and re-create it via check-in.

Acceptance criteria (new, OPEN):
1. A session record created by the capture hook can be claimed via `check_in_worktree` without deleting it first.
2. The ownership check tolerates an absent `ticket_id` on the existing record, treating an unclaimed record as claimable rather than as a mismatch.
3. A regression test covers claiming a hook-written record that has `agent_id` set and `ticket_id` absent.


## 2026-08-07 Implementation Update

### User Decisions

1. **Ownership relaxation scope:** tolerate both gates. A record with no `ticket_id` is unclaimed, so any owner may claim it. The hook's `agent_id` is a generic placeholder, so relaxing only the `ticket_id` gate would leave hook-written records unclaimable at the `agent_id` gate.
2. **Hook fix ownership:** the narrow capture-hook fix moved from ticket `5e6cf4f8-120c-4674-95de-d7b79c99f5b3` to this ticket as a new acceptance criterion. Ticket `5e6cf4f8-120c-4674-95de-d7b79c99f5b3` retains only eager worktree creation and the `worktree.sh`-to-Rust rewrite.
3. **Fresh-session capture:** when a brand-new session has no record and no discoverable worktree, the hook deliberately skips capture. No main-checkout capture fallback will be added.

### Implemented

- **Ownership relaxation (DONE):** `memory-api/crates/session-api/src/store/config/worktree_runtime.rs` treats a missing or whitespace-only `metadata.ticket_id` as unclaimed. Any owner may claim an unclaimed record; claimed records still require matching `agent_id` and `ticket_id`; successful claims persist the claimant owner and ticket. Covering tests: `check_in_worktree_claims_unclaimed_hook_record` and `check_in_worktree_rejects_mismatched_claimed_owner`.
- **Capture-hook worktree resolution (DONE):** `memory-api/crates/session-capture-hook/src/main.rs` derives the capture-time worktree root from the resolved `store_root` parent, and `initialize_session_routing` resolves through `SessionWorkspaceResolver` rather than process current directory. `memory-api/crates/session-api/src/store/config/worktree_capture_inference.rs` adds `SessionStoreConfig::replace_main_worktree_inference`, which replaces only a stale main-checkout assignment once a real worktree resolves. Covering tests: `capture_inference_uses_resolved_store_parent_not_process_directory`, `user_prompt_submit_discovers_the_session_worktree_from_main_cwd`, `user_prompt_submit_without_discoverable_worktree_does_not_assign_main`, and `user_prompt_submit_replaces_a_stale_main_checkout_assignment`.

### Acceptance Criteria Status

- **New capture-hook criterion (DONE):** The capture hook records the resolved worktree and never persists a main-pointing assignment. A stale main-pointing assignment is corrected once a real worktree resolves.
- **Defect: a hook-written record is permanently unclaimable, criterion 1 (DONE):** an absent or whitespace-only `ticket_id` makes the record unclaimed. Covered by `check_in_worktree_claims_unclaimed_hook_record`.
- **Defect: a hook-written record is permanently unclaimable, criterion 2 (DONE):** any owner may claim an unclaimed record despite the hook placeholder `agent_id`. Covered by `check_in_worktree_claims_unclaimed_hook_record`.
- **Defect: a hook-written record is permanently unclaimable, criterion 3 (DONE):** a claimed record with mismatched owner or ticket remains rejected. Covered by `check_in_worktree_rejects_mismatched_claimed_owner`.

### Implementation Finding

A second inference call site, `initialize_session_routing`, was discovered during implementation. `initialize_session_routing` wrote into the anchor/main `.session` store that the resolver reads and was the true source of the wrong main-pointing record; earlier ticket notes attributed the problem to a single call site. Fresh-session skip behavior remains deliberate when no record and no discoverable worktree exist.


## 2026-08-07 Spec Linkage and Review Handoff

Spec [0f5acbfe session-worktree-routing](.spec/specs/0f5acbfe-743b-4f1e-abfd-54628e49fb5f/spec.toml) now documents the resolution chain, the capture-hook fix, the check-in ownership relaxation, and the validation evidence. Ticket a1b911ab is linked from the spec's `related_tickets`.

### Live routing evidence

`board_show` called with `workspace: "default"` from this session resolves to `C:/Users/linus/git/context-engine/.worktrees/70abae1b-session-worktree-discovery`, not the main checkout, confirming session-id routing works end to end through the installed binaries.

The main checkout's session record still reads `path: C:/Users/linus/git/context-engine, branch: main` (captured 14:26, before the fix was installed). The new `replace_main_worktree_inference` path is expected to overwrite it on the next `UserPromptSubmit` now that the corrected `copilot-capture-hook` binary is installed. That self-heal is the remaining observable confirmation.

Stale `.session-routing/worktree-index.json` in the main checkout — a leftover of the removed registry design that held a wrong session-to-checkout mapping — has been deleted.

Verified 2026-08-08: acceptance criteria already satisfied by merged work; no additional code change required.
