Add track_id, anchor_ticket_id, parent_session_id (all NULLABLE) to SessionRecord. Activate spawned_session_id. No migration work in this ticket.

**Code anchors**: memory-api/crates/session-api/src/model.rs (SessionRecord struct)

**Acceptance**: Schema fields present, no load errors on existing sessions.

**Batch 1 note** (2026-07-27): Dependency on 0a45bedb CONFIRMED KEPT by user decision. Both tickets in same batch (Batch 1: session store flatten + track schema + query surface).