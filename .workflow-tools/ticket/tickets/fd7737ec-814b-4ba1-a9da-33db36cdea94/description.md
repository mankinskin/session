FAIL-OPEN migration strategy: pre-existing sessions have track_id = null, NO backfill pass.

**Code anchors**: memory-api/crates/session-api/src/store.rs (load/reconcile logic)

**Acceptance**: Existing sessions load without error, track_id reports null for pre-existing sessions.