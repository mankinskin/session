FULL metrics rollup: session count, status, aggregate tool/cost metrics, duration, token cost, per-agent breakdown.

**Code anchors**: memory-api/crates/session-api/src/subagent_rollup.rs, memory-api/crates/session-api/src/tool_metrics.rs

**Acceptance**: Rollup returns session count, status distribution, tool/cost aggregates, duration, per-agent breakdown.

**Batch 1 validation bar** (shared with 185a00a2, 9cd9440e): End-to-end session lifecycle test covering check-in → handoff write → finish → reload, asserting on-disk layout and track fields, PLUS MCP and CLI round-trip through new track query surface. Base commands: `cargo test -p session-api && cargo test -p session-mcp`, plus memory-kernel board tests.


## Scope change (decided architecture — reparented under epic d28afbc0)

Per spec [c737328d Session merge and pickup](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) (R5) and the settled interview architecture, `track` is no longer a free-form tag: it is a first-class `Track` entity persisted under `.session/tracks/<id>/` (delivered by ticket [a2194b92 Track entity and store](../../.ticket/tickets/a2194b92-d0b2-4eb8-a2a6-975919ab4035/ticket.toml)). This ticket's query/rollup surface must read from that entity's manifest — including `anchor_ticket_id`, member session ids, and the bidirectional `depends_on` track edges from [d085cf2b Bidirectional depends_on track edges](../../.ticket/tickets/d085cf2b-7683-4aea-8566-99dc883ee491/ticket.toml) — rather than scanning sessions by a bare `track_id` string. The rollup must never duplicate ticket-api status; it reads live through `anchor_ticket_id`.

This ticket is now a child of epic [d28afbc0 Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml). It depends on [a2194b92 Track entity and store](../../.ticket/tickets/a2194b92-d0b2-4eb8-a2a6-975919ab4035/ticket.toml) in addition to its existing dependencies.