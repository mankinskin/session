## Problem
`SessionHandoffPackage` has no durable higher-level objective or structured upward-context data. `create_handoff_record` therefore cannot distinguish an implementation-ready handoff with missing program context from a well-formed handoff.

## Goal
Add durable, backward-compatible package data and conditional creation-time validation for the already-decided upward-context contract.

## Authoritative Spec
`5e52039d` (`agent-workflow/handoff-package-schema`, Handoff Package Schema). A separate agent owns the spec update; this task must remain compatible with that update.

## Target Paths
- `memory-api/crates/session-api/src/model/handoff.rs`
- `memory-api/crates/session-api/src/store/config/handoff_finish.rs`
- `memory-api/crates/session-api/tests/handoff_folder_storage.rs`
- `memory-api/crates/session-api/tests/handoff_roundtrip.rs`

## Acceptance Criteria
1. `SessionHandoffPackage` persists a structured upward-context ancestor chain whose entries contain an entity URN, a human title, and a role (for example epic, phase, or parent), plus a prose `higher_level_objective` field that explains why the current phase exists now. Round-trip tests prove the fields survive JSON persistence.
2. Creation determines “claims implementation-ready” from the existing derived readiness rule: objective is non-empty and `open_escalations` is empty. When that condition is true, absent or empty `higher_level_objective` or ancestor chain rejects creation with `SessionError::HandoffPackageIncomplete` before handoff artifacts are written.
3. When the derived readiness condition is false, absent upward-context fields emit warnings but creation persists the exploratory handoff. Tests prove the warning path does not change the existing readiness derivation.
4. Existing on-disk `handoff.json` data that lacks the new optional-on-wire fields remains deserializable. The task does not change the existing path existence validation for `target_files` or path-shaped `context_anchors`.

## Related Work
`7bb007e9` is an adjacent generic conformance-gate effort that may later share mechanism but is not a blocking dependency. `f77e35d8`, `a6f17580`, `6431985e`, and `0d3fdba6` are adjacent handoff work with different field sets or scope. `e4f84414` is a markdown-renderer precedent, not scope for this task.


## Validation Evidence
- Merge commit: `26c7ce7c`
- Validation spec: `session-api-tests` (`cargo test -p session-api`)
- Validation execution: `exec-session-api-handoff-upward-context-20260806` (`passed`)
- Supporting checks: `cargo build --workspace`, `cargo check -p session-mcp`, `cargo check -p session-cli`
- Coverage: legacy `target_tickets` deserialization, readiness-gated upward-context enforcement, warning-path persistence, breadcrumb/table rendering, fallback rendering for unresolved tickets, and regeneration-vs-exemplar proof with no manual hand edit required for the proof run.
- Acceptance verdicts: AC1 met; AC2 met; AC3 met; AC4 met.
## Review Verdict (2026-08-06)
Validation: `cargo test -p session-api` passed, 262 tests.
- AC1 initially UNMET — the folder round-trip test never asserted the deserialized `higher_level_objective` or `upward_context` values.
- AC1 now MET on branch `agent/742dbc65-handoff-roundtrip-assertions`: `handoff_persists_as_folder_with_json_and_markdown` now asserts the prose objective plus the full ordered ancestor chain, with explicit `entity_urn`/`title`/`role` checks on the first entry.
- AC2 met — `create_handoff_record` rejects ready-but-contextless handoffs before writing files (`ready_handoff_missing_upward_context_fails_before_writing_files`).
- AC3 met — `non_ready_handoff_missing_upward_context_persists` proves the warning path preserves readiness derivation.
- AC4 met — `legacy_target_ticket_strings_and_absent_context_fields_deserialize`; existing path validation unchanged.
Commits: submodule `5802526`, outer `9505b147`. NOT yet merged to main.
Blocked on: fast-forward merge deferred because main's `memory-api` submodule holds an unrelated agent's uncommitted changes.



## Iteration Decision (2026-08-08)

Retain existing commit `9505b147` on `agent/742dbc65-handoff-roundtrip-assertions` as WIP. Restore or recreate the branch worktree, rebase onto current `main`, complete the missing AC1 deserialized-equality assertions, and run `cargo test -p session-api --test handoff_roundtrip`.
