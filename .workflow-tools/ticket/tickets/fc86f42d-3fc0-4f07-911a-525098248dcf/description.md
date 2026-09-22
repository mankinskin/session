# Goal

Establish the missing join between a runtime continuity thread and its captured transcripts so handoffs and context can be session-owned (subticket 1 of the flattening tracker). Foundational — no directory move yet.

# Problem (verified)

- `SessionRuntimeContext` (`memory-api/crates/session-api/src/model.rs:51`) has `workspace_session_id` but no `session_id`.
- `SessionRunLineage` (`model.rs:43`) has `run_id` + `predecessor_run_id` but no captured-`session_id`.
- Captured sessions (`SessionLinks`, `model.rs:298`) do not reference a thread/run.
- So "handoff owned by the session" is currently undefined — there is nothing tying a transcript to a runtime thread/run.

# Solution Design

1. Introduce a canonical `session_id` for the continuity thread. Simplest: make `workspace_session_id` the session id (alias) or add `session_id` to `SessionRuntimeContext` populated at `init_runtime_context` (store.rs:608).
2. Stamp each captured transcript with its owning `session_id` + `run_id`: extend `SessionRunLineage` (or the capture manifest) so a run records the captured `session_id`, and the capture path (`capture_copilot_hook`) records the active `run_id`/thread.
3. Provide a resolver `runs_for_session(session_id) -> Vec<run>` and `session_for_run(run_id)` so readers can navigate the join.
4. Keep serde defaults so existing records without the join still deserialize.

# Non-goals

- Moving files on disk (subticket 2).

# Acceptance Criteria

1. Runtime context exposes a stable `session_id` for the thread.
2. Each run records its captured `session_id`; the join is navigable both directions.
3. Old records without the join deserialize with empty/derived defaults.
4. Focused test asserts the thread↔run↔transcript join round-trips.

# Traceability

- Parent: flattening tracker.
- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913`.
# Review Findings (2026-07-26)

Body was clobbered in commit `0c49f48` with an unrelated store-refactor note; the correct requirements above were restored from revision `17ad067`.

Re-review against the restored ACs (136 tests pass):
- AC1 (stable `session_id`) — MET: `session_id` + `canonical_session_id()` in `crates/session-api/src/model.rs` (~L89/L97).
- AC2 (run records captured `session_id`; join navigable both directions) — NOT MET: `captured_session_id` field exists (`model.rs` ~L79) but is always written `None` (worktree_runtime.rs ~L200/L258); no `runs_for_session()` / `session_for_run()` resolvers exist. The join is not navigable.
- AC3 (old records deserialize with defaults) — MET: `#[serde(default)]` present.
- AC4 (focused round-trip test) — NOT MET: no test asserts session↔run↔transcript navigation.

Remaining work: populate `captured_session_id` on run creation, add both-direction resolvers, and add the round-trip test. Sent back to implementation.
