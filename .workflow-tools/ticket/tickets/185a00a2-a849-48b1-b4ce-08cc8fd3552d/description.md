MCP tools for querying sessions by track_id and track-scoped rollup.

**Code anchors**: memory-api/tools/mcp/session-mcp/src/server.rs

**Acceptance**: MCP tools can query by track_id and return rollup metrics.

**Batch 1 validation bar** (shared with 938a7ae9, 9cd9440e): End-to-end session lifecycle test covering check-in → handoff write → finish → reload, asserting on-disk layout and track fields, PLUS MCP and CLI round-trip through new track query surface. Base commands: `cargo test -p session-api && cargo test -p session-mcp`, plus memory-kernel board tests.


## Scope change (decided architecture — reparented under epic d28afbc0)

Per spec [c737328d Session merge and pickup](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) (R5), this ticket's MCP surface is now scoped strictly to **querying** the first-class `Track` entity by `track_id` and returning track-scoped rollup metrics from ticket [938a7ae9 Track query and rollup surface](../../.ticket/tickets/938a7ae9-570e-40c4-91f5-d32d2fae0b4f/ticket.toml). It does **not** cover the `fan_out`/`merge`/`pickup` operation verbs — those MCP tools are owned by ticket [1d378109 CLI + MCP exposure of fan_out / merge / pickup](../../.ticket/tickets/1d378109-28d2-442b-a2a1-4e18cd716327/ticket.toml). Coordinate naming and tool boundaries with that ticket so query/rollup tools and operation-verb tools do not overlap or duplicate.

This ticket is now a child of epic [d28afbc0 Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).