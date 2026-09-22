## Goal
Build coded support that reduces what GitHub Copilot sends or reuses in model-facing context by guarding tool results upstream and emitting compact prompt-facing state views.

## Scope
- classify model-facing tool results into retain, summarize, reference-only, and drop-from-prompt
- suppress duplicate tool lifecycle wrappers, repeated unchanged state checks, and routine retry narration
- preserve artifact pointers for long outputs instead of inlining spill-file contents
- expose compact bootstrap and handoff oriented state views that keep durable findings without replaying raw workflow chatter

## Important boundary
- `session-api` capture hooks remain diagnostic visibility only
- this ticket targets upstream request shaping and prompt-facing compaction, not post-hoc transcript storage changes

## Inputs
- spec: `.spec/specs/f5e0df47-d0ec-4456-b268-689f2a41ecd7`
- immediate workflow guidance ticket: `.ticket/tickets/1c1ebfd1-4478-401f-a9ad-efcc2ff53b16`

## Acceptance notes
- model-facing session state keeps durable findings and drops raw boilerplate by default
- long artifacts remain addressable via pointers and bounded extraction
- duplicate suppression is deterministic and test-covered
- the implementation demonstrably improves upstream prompt compactness rather than only archival transcript readability

## Validation evidence (2026-07-09)
- `cargo test -p session-api` passed after adding representative fixture-style cases in `memory-api/crates/session-api/src/peek.rs` for:
	- repeated terminal/status checks (normalized dedupe)
	- duplicate lifecycle wrapper narration suppression
	- retry narration variant suppression
	- spill-pointer reference-only classification vs oversized inline blob summarization
- `cargo test --manifest-path memory-api/tools/cli/session-cli/Cargo.toml` passed after expanding `memory-api/tools/cli/session-cli/tests/cli.rs` prompt-pack fixture assertions for dropped/summarized/reference-only counts and reason markers.

## Review evidence (2026-07-09 follow-up)
- Transcript review across `.session/sessions/fc7ae564-d732-486d-a495-55fdb865397e` and `.session/sessions/95c5d8f1-34cd-4cc5-ac34-c7f6fd0d34a2` showed persistent over-labeling of `run_in_terminal` sync completions as `sync-terminal-state-ambiguous` when no explicit ambiguity signal was present.
- Hardened `memory-api/crates/session-api/src/hook.rs` ambiguity heuristics to require explicit ambiguity indicators (background/timeout/input-needed markers) instead of flagging all sync successes lacking exit metadata.
- Added regression coverage in `memory-api/crates/session-api/src/hook/tests.rs` for both cases:
	- sync completion without ambiguity signals does not emit blocker fields
	- sync completion with explicit background signal still emits `sync-terminal-state-ambiguous`
- Re-ran validations after the hook-path update:
	- `cargo test -p session-api`
	- `cargo test --manifest-path memory-api/tools/cli/session-cli/Cargo.toml`

## Quantitative compactness evidence (2026-07-09)
- Added measurable tool-output compactness assertions in `memory-api/crates/session-api/src/peek.rs`:
	- prompt-pack fixture now uses repeated tool-state checks plus spill-pointer and oversized inline payload cases (`dropped_turns >= 2`, `included <= 5` on the tool-heavy fixture)
	- unit coverage remains focused on normalized repeated state-check suppression and pointer-vs-inline payload handling
- Added CLI-level quantitative gate in `memory-api/tools/cli/session-cli/tests/cli.rs` (`peek_prompt_pack_meets_quantitative_compactness_gate`) to validate output counts through the command boundary (`dropped >= 2`, `included <= 3`) on the seeded tool-output fixture.
- Added cross-boundary replay regression in `memory-api/crates/session-api/src/hook/tests.rs` (`transcript_normalization_and_prompt_pack_tool_result_consistency`) validating hook normalization plus downstream prompt-pack behavior in one path:
	- sync `run_in_terminal` completion without explicit ambiguity remains non-blocking
	- prompt-pack output remains consistent with tool-result normalization without relying on transcript-narration suppression
- Validation rerun after this pass:
	- `cargo test -p session-api` (pass)
	- `cargo test --manifest-path memory-api/tools/cli/session-cli/Cargo.toml` (pass)

## Rule rationale
- Keep `session-api` transcript capture diagnostic-only.
- Perform compactness controls before prompt reuse with deterministic guards.
- Preserve pointer-addressability for large artifacts while avoiding default inline payload expansion.