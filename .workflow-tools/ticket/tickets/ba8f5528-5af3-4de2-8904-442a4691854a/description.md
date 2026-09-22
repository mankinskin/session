## Problem
`render_handoff_record_markdown` renders target tickets as bare short IDs and omits both a higher-level goal and upward program context. Manual edits to the generated exemplar are destroyed by the next handoff generation.

## Goal
Generate legible, reproducible handoff markdown from durable package data, resolving ticket metadata through existing `ticket-api` integration and never requiring post-generation edits.

## Authoritative Spec
`5e52039d` (`agent-workflow/handoff-package-schema`, Handoff Package Schema). A separate agent owns the spec update.

## Target Paths
- `memory-api/crates/session-api/src/model/handoff.rs`
- `memory-api/crates/session-api/src/store.rs`
- `memory-api/crates/session-api/src/store/config/handoff_finish.rs`
- `memory-api/crates/session-api/tests/handoff_folder_storage.rs`
- `memory-api/crates/session-api/tests/handoff_roundtrip.rs`
- `.session/sessions/910b25a7-3917-42c6-bf5f-d860221ac7e2/handoffs/a9519525-4f52-48df-a884-cff638f6d0db/handoff.md`

## Acceptance Criteria
1. `target_tickets` evolves from bare IDs to structured entries with at least a ticket ID and author-supplied `why` text. Serde accepts legacy JSON arrays of strings so existing stored handoffs remain readable.
2. `render_handoff_record_markdown` emits the prose higher-level objective near the beginning and an upward-context breadcrumb that presents ancestor role, human title, and clickable entity reference in epic-to-phase-to-leaf order.
3. Generated markdown includes a per-ticket table. Each row contains a clickable ticket reference and title plus auto-resolved “what it does” from ticket-api, and preserves the author-supplied “why it belongs in this handoff.”
4. A missing or unresolvable referenced ticket produces a clear fallback row using available authored/ID data; rendering neither panics nor fails handoff creation. Tests cover the fallback behavior.
5. Regenerate handoff `a9519525-4f52-48df-a884-cff638f6d0db` entirely through the generator and diff it with the hand-authored exemplar. The generated artifact demonstrably has: a stated high-level goal up front, an epic-to-phase-to-leaf breadcrumb, real titles replacing every bare ticket ID, and clickable references following repository policy.

## Related Work
`e4f84414` is the closest markdown-renderer precedent. `f77e35d8` changes the terminal renderer for different narrative fields; no dependency is intended. `7bb007e9`, `a6f17580`, `6431985e`, and `0d3fdba6` are adjacent but non-blocking work.


## Validation Evidence
- Merge commit: `26c7ce7c`
- Validation spec: `session-api-tests` (`cargo test -p session-api`)
- Validation execution: `exec-session-api-handoff-upward-context-20260806` (`passed`)
- Supporting checks: `cargo build --workspace`, `cargo check -p session-mcp`, `cargo check -p session-cli`
- Coverage: structured `target_tickets`, resolver-backed title/what-it-does rendering, upward-context breadcrumb rendering, fallback row behavior for unresolved tickets, no double-linking in existing links or code spans, and regenerated exemplar proof with all four quality checks passing.
- Reviewed deviation: the exemplar at `.session/sessions/910b25a7-3917-42c6-bf5f-d860221ac7e2/handoffs/a9519525-4f52-48df-a884-cff638f6d0db/handoff.md` was regenerated and diffed for proof, but the stored file was not overwritten in the isolated worktree because that path was untracked there.
- Acceptance verdicts: AC1 met; AC2 met; AC3 met; AC4 met; AC5 met with reviewed deviation.
## Review Verdict (2026-08-06)
Validation: `cargo test -p session-api` passed, 262 tests.
- AC1 met — `SessionHandoffTargetTicket` with a custom deserializer that still accepts legacy string arrays.
- AC2 met — `render_handoff_record_markdown` emits the higher-level objective and an epic-to-phase-to-leaf upward-context breadcrumb.
- AC3 met — `resolve_handoff_ticket_display` populates the per-ticket table with resolved title and "what it does", preserving the authored "why".
- AC4 met — `handoff_markdown_degrades_when_target_ticket_is_unresolvable` covers the fallback row.
- AC5 met with reviewed deviation — regenerate-and-diff proof accepted; the historical exemplar a9519525 was correctly left untouched per the recorded immutability decision.
Verdict: all acceptance criteria satisfied. Cannot transition to done yet — the ticket store blocks it while dependency 742dbc65 remains at in-review.
