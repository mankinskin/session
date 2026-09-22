Implement the next `session-api` batch in the nested `memory-api` workspace by making transcript persistence append only and adding the first read/query plus hook-facing capture APIs.

# Implemented Slice
- Added append-only transcript merging so later captures extend the stored transcript instead of replacing prior turns.
- Added session read helpers that reconstruct persisted sessions from manifest plus transcript files.
- Added `SessionQuery` and a workspace-scoped query API for simple metadata and transcript text filtering.
- Added `capture_copilot_hook` as the first hook-facing helper that persists a `CopilotHookPayload` through the store.
- Added explicit read-side error variants for missing data, deserialization failures, and transcript rewrite conflicts.

# Validation
- ValidationSpec: focused `session-api` tests for append-only persistence, session reads, query behavior, and hook capture.
- ValidationExecution: passed `cargo test -p session-api`.

# Evidence Trail
- Spec: `36fd7849-65eb-405e-8cc5-70440f0cb7c2`.
- DocEvidenceRecord candidates: `crates/session-api/src/error.rs`, `crates/session-api/src/lib.rs`, and `crates/session-api/src/store.rs`.
- ValidationLogCapture / ValidationLogRetrieval: `cargo test -p session-api` output captured in the current terminal session.

# Remaining Work
- Expose session capture through an external hook, CLI, MCP, or HTTP surface.
- Add indexing or richer query semantics beyond small in-memory scans.
- Consider stronger atomicity for paired manifest/transcript filesystem writes.