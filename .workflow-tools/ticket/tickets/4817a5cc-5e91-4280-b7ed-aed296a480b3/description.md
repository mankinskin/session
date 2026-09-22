# Goal

Make durable session artifacts persist in git for later feedback/research loops, while keeping machine-local pointers/locks ignored (subticket 4 of the flattening tracker). Depends on the flattened layout + handoff folders.

# Problem (verified)

- Root `.gitignore`:
  ```
  .session/*
  !.session/sessions/
  !.session/sessions/default/**
  .session/sessions/**/session-capture-stop.log
  ```
  Tracks `sessions/**` (222 files tracked) but never re-includes `runtime/`, so handoffs/context/finish are git-ignored.
- `memory-api/.gitignore:5` ignores `.session/` wholesale (inconsistent with root).

# Solution Design

1. Rewrite root `.gitignore` to track durable artifacts under `sessions/<session_id>/` (`context.json`, `handoffs/**`, `finish.json`, `runs/**/transcript.json`) and ignore only `.session/local/` and `*.lock` + capture logs.
2. Align `memory-api/.gitignore` with the same policy (stop ignoring the whole `.session/`).
3. Split tracking by **durability, not directory**: local pointer + lock ignored; everything session-owned tracked.
4. Guard repo bloat: land event-capture de-dup (`67d7c279`) before bulk-tracking `events.json`; decide whether `runs/**/events.json` is tracked or ignored (recommend track transcript, keep events optional/ignored initially).

# Acceptance Criteria

1. New handoffs/context/finish under `sessions/<session_id>/` are git-tracked.
2. `.session/local/` pointers and `*.lock` remain ignored.
3. Root and `memory-api` `.gitignore` express one consistent policy.
4. A documented decision records the events.json tracking choice and its size rationale.

# Traceability

- Parent: flattening tracker. Depends on layout-flatten + handoff-folder subtickets.
- Prerequisite: event de-dup `67d7c279` (land before bulk-tracking events).
- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913`.

## Review outcome (2026-07-27) — FAILED

A live review returned FAIL. Two of four ACs fail:

- **AC1 FAIL.** Handoffs / context / finish do not live under `.session/sessions/<session_id>/`. `persistence.rs` (memory-api/crates/session-api/src/store/config/persistence.rs, ~L142-156) writes them to `runtime_root()/workspaces/<workspace_session_id>/` → `context.json`, `handoffs/`, `finish.json`. `git check-ignore -v .session/runtime` resolves to `.gitignore:35:.session/*`, so the entire runtime tree is ignored. Zero handoff/context/finish files are tracked anywhere in the repo. The ticket's own stated core problem is still present and directly verifiable.
- **AC2 PASS (rule present, unverified).** The `.session/local/` and `*.lock` ignore patterns exist, but no such artifact is on disk to prove the rule fires.
- **AC3 PASS.** Root `.gitignore` and `memory-api/.gitignore` now express a byte-identical `.session` block. The inconsistency named in the problem statement is gone.
- **AC4 FAIL.** No documented decision for the `events.json` tracking choice exists anywhere (rule store, spec sections, docs).

Not a policy failure: the untracked `.session/sessions/*` directories shown by `git status` are `??` (new, addable) rather than `!!` (ignored) — they are simply uncommitted captures.

Stale claim to disregard: a prior session transcript asserted this policy "matches acceptance criteria", citing passing session-api tests. Those tests do not exercise `.gitignore` behavior, and `git check-ignore` contradicts the claim.

## Decision (user, 2026-07-27)

**AC1 is correct as written; the files are misplaced.** Do not relax the AC to match the current layout. The correct fix is not a `.gitignore` patch — the entire "workspace" model for sessions needs to be redesigned under epic `16e4063a` (Track-scoped multi-session execution), which this ticket is now `linked` to. This ticket stays blocked behind its existing `depends_on` dependency `41ed4585` (store handoffs as folders), which is still `new`; the user confirmed that edge is valid, not stale.

## Added acceptance criterion

5. A test asserts the `.gitignore` rules directly — that `runtime/workspaces/*/handoffs` (or its post-redesign equivalent) is NOT ignored, and that `.session/local/` and `*.lock` ARE ignored. Passing session-api unit tests must never again be cited as evidence for git-tracking behavior.

## Split out

The ~99MB / 76 already-committed `events.json` files are now tracked separately by ticket `580019e8` (untrack with `git rm --cached`, no history rewrite) and are out of scope here.
