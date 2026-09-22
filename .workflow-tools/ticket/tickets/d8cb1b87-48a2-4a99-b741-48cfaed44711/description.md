## Corrected Scope

The repository audit default is `max_file_lines = 400`. A prior tracker update incorrectly used a non-default per-run override as the acceptance criterion. That override has been removed from the live validation contract; historical session transcripts and handoffs remain immutable diagnostic evidence only.

## Current Default-Audit Queue

The canonical unparameterized `mcp_audit-mcp_audit` run against `memory-api` reports these session-api `line_count` violations, in descending size order:

1. `crates/session-api/src/transcript_feedback.rs` — 1,000 lines.
2. `crates/session-api/src/hook.rs` — 726 lines.
3. `crates/session-api/src/peek.rs` — 665 lines.
4. `crates/session-api/src/model.rs` — 567 lines.
5. `crates/session-api/src/store/helpers/storage.rs` — 518 lines.
6. `crates/session-api/src/bin/copilot-capture-hook.rs` — 516 lines.
7. `crates/session-api/src/hook/tests.rs` — 412 lines.

## Required Validation

- Run `rtk cargo test -p session-api --lib` for every implementation slice.
- Run canonical `mcp_audit-mcp_audit` against `memory-api` **without** `max_file_lines`; acceptance requires no session-api `line_count` finding above the default 400-line limit.
- Validation spec `vt-session-api-audit-line-count` is deliberately marked failed until that default-audit gate passes.