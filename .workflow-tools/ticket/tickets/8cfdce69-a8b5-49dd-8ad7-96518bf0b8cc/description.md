# [session-api] Transcript skeleton peeking

## Purpose
Provide a token-efficient way to inspect the structure of a session transcript by returning only the metadata/signatures of turns (sequence, role, captured_at, tool_name, and content length/summary) without pulling the full content of long messages.

## Scope
- Add a transcript skeleton peeking API to `SessionStoreConfig` in `session-api`
- Define a `SessionTurnSkeleton` model that excludes the full `content` string but includes its length in characters or a truncated preview (e.g., first 60 chars)
- Return a list of turn skeletons for a given session
- Expose this capability through `SessionStoreConfig`

## Out of Scope
- Exposing this through CLI/MCP/HTTP in this slice (handled by follow-up tickets)

## Acceptance Criteria
1. `SessionStoreConfig` has a `peek_transcript_skeleton(session_id)` method that returns a list of `SessionTurnSkeleton`s.
2. `SessionTurnSkeleton` contains `sequence`, `role`, `captured_at`, `tool_name`, `content_len`, and a short `content_preview` (first 60 characters).
3. The full content of the turns is never loaded into memory or serialized as part of the skeleton response.
4. Focused unit tests cover skeleton generation and preview truncation.

## Validation Plan
- Run `cargo test -p session-api` to verify all unit tests pass.
- Add focused tests in `crates/session-api/src/store.rs` covering skeleton peeking.