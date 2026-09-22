## Problem
`render_handoff_record_markdown` currently renders `target_tickets` as bare backticked short IDs and cannot explain the higher-level goal or an upward program context. A prior handoff required a manual edit to generated `handoff.md`; regeneration destroys that edit.

## Goal
Make the higher-level objective, upward context, and per-ticket narrative durable handoff-package data and render the data automatically, with no post-generation hand editing.

## Scope
Parent tracker for the two implementation leaves below. Authoritative specification: `5e52039d` (`agent-workflow/handoff-package-schema`, Handoff Package Schema). The spec update is owned by another agent and is not part of this tracker.

Target implementation paths:
- `memory-api/crates/session-api/src/model/handoff.rs`
- `memory-api/crates/session-api/src/store/config/handoff_finish.rs`
- `memory-api/crates/session-api/src/store.rs`
- `memory-api/crates/session-api/tests/handoff_folder_storage.rs`
- `memory-api/crates/session-api/tests/handoff_roundtrip.rs`
- `.session/sessions/910b25a7-3917-42c6-bf5f-d860221ac7e2/handoffs/a9519525-4f52-48df-a884-cff638f6d0db/handoff.md`

## Acceptance Criteria
1. The package schema has both structured upward context (entity URN, human title, role) and prose `higher_level_objective`; persistence and creation behavior are covered by the schema/enforcement leaf.
2. A package that would otherwise be implementation-ready rejects missing upward-context fields with `SessionError::HandoffPackageIncomplete`; an already-not-ready exploratory package persists with warnings. The readiness determination remains based on a non-empty objective and no open escalations.
3. Generated markdown has a per-ticket table that resolves title and what-it-does from ticket-api and renders author-supplied why-in-this-handoff text. Existing string-form `target_tickets` JSON remains readable, and unresolved tickets degrade gracefully without panic or handoff failure.
4. Regenerating handoff `a9519525-4f52-48df-a884-cff638f6d0db` produces inspectably equivalent structural qualities to its hand-authored exemplar: high-level goal first, epic-to-phase-to-leaf breadcrumb, titles instead of bare IDs, and clickable references following repository policy.

## Related Work
`f77e35d8`, `7bb007e9`, `a6f17580`, `6431985e`, and `0d3fdba6` are adjacent handoff work, not blocking dependencies. `e4f84414` is the renderer-change precedent. No dependency edge to adjacent tickets is intended.


## Validation Evidence
- Merge commit: `26c7ce7c`
- Validation spec: `session-api-tests` (`cargo test -p session-api`)
- Validation execution: `exec-session-api-handoff-upward-context-20260806` (`passed`)
- Supporting checks: `cargo build --workspace`, `cargo check -p session-mcp`, `cargo check -p session-cli`
- Coverage: schema persistence and conditional enforcement in the schema/enforcement leaf, markdown rendering and fallback behavior in the rendering leaf, and the regenerated exemplar proof with four quality checks passing.
- Reviewed deviation: the exemplar at `.session/sessions/910b25a7-3917-42c6-bf5f-d860221ac7e2/handoffs/a9519525-4f52-48df-a884-cff638f6d0db/handoff.md` was regenerated and diffed for proof, but the stored file was not overwritten in the isolated worktree because that path was untracked there.
- Acceptance verdicts: AC1 met; AC2 met; AC3 met; AC4 met with reviewed deviation.