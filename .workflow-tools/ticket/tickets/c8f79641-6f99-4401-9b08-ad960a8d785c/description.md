Implement the next `session-api` batch in the nested `memory-api` workspace by turning the current store plan into a real filesystem write path.

# Implemented Slice
- Added a persisted session manifest shape and a persisted transcript shape.
- Added `SessionStorePlan::persist()` to create the session directory and write `session.json` plus `transcript.json`.
- Added `SessionStoreConfig::persist_capture()` for the first end-to-end capture-to-disk flow.
- Extended `SessionError` with explicit serialization and filesystem error variants.
- Added focused unit tests for file creation, persisted JSON content, and repeat-write overwrite behavior.

# Validation
- ValidationSpec: focused `session-api` persistence tests.
- ValidationExecution: passed `cargo test -p session-api`.

# Evidence Trail
- DocEvidenceRecord candidates: `crates/session-api/Cargo.toml`, `crates/session-api/src/error.rs`, `crates/session-api/src/lib.rs`, and `crates/session-api/src/store.rs`.
- ValidationLogCapture / ValidationLogRetrieval: `cargo test -p session-api` output captured in the current terminal session.
- Linked spec: `823b22cf-c0dc-46c6-a03d-00cdd3c4c83a`.

# Remaining Work
- Add ingest surfaces that emit `SessionCaptureRequest` from actual Copilot hooks.
- Decide whether repeated writes should remain overwrite-based or move to append-only transcripts.
- Add read/query surfaces once downstream consumers are ready.