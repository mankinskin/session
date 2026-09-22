# [session-api] Token-efficient transcript range peeking

## Purpose
Provide a token-efficient way to inspect a specific range of turns in a persisted session transcript without reading the entire file. This mirrors the `peek` CLI tool's line-range behavior but operates at the semantic turn level for session transcripts.

## Scope
- Add a turn-range peeking API to `SessionStoreConfig` in `session-api`
- Allow specifying a start turn index and an end turn index (inclusive, 0-based)
- Allow specifying a window size around a target turn index
- Return a subset of `SessionTurn`s matching the range or window
- Expose this peeking capability through the `SessionStoreConfig` read path

## Out of Scope
- Exposing this through CLI/MCP/HTTP in this slice (handled by follow-up tickets)
- Full-text search within the peeking API (already handled by `query_sessions`)

## Acceptance Criteria
1. `SessionStoreConfig` has a `peek_transcript_range(session_id, start_turn, end_turn)` method that returns only the turns in that range.
2. `SessionStoreConfig` has a `peek_transcript_window(session_id, target_turn, window_size)` method that returns a window of turns centered around the target.
3. Out-of-bounds ranges are handled gracefully (truncated to available turns instead of panicking).
4. Focused unit tests cover range peeking, window peeking, and out-of-bounds handling.

## Validation Plan
- Run `cargo test -p session-api` to verify all unit tests pass.
- Add focused tests in `crates/session-api/src/store.rs` covering range and window peeking.