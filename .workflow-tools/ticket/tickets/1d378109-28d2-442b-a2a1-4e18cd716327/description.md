## Objective

Expose `fan_out` / `merge` / `pickup` as CLI subcommands and MCP tools per spec [c737328d](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) requirement R4.

## Scope

- CLI verb names: `fan_out`, `merge`, `pickup` (plus `handoff` for the 1→1 target-less emit) — no other terminology.
- MCP tool names use the same canonical vocabulary.
- Also expose the unclaimed-handoff backlog query (from the edge-model ticket) as a CLI/MCP surface, so callers can discover open handoffs before calling `pickup`/`merge`.

## Coordination note

Ticket [185a00a2 MCP tool surface for track query and rollup](../../.ticket/tickets/185a00a2-a849-48b1-b4ce-08cc8fd3552d/ticket.toml) owns MCP tools for **querying tracks by `track_id` and track-scoped rollup** — it does not cover `fan_out`/`merge`/`pickup`. This ticket owns the **operation** verbs (`fan_out`/`merge`/`pickup`/`handoff`) and the unclaimed-handoff backlog query. Do not duplicate track query/rollup tools here; do not add operation verbs to 185a00a2.

## Code anchors

- memory-api/tools/mcp/session-mcp/src/server.rs
- memory-api CLI entry point for session-api (session CLI crate/binary)

## Acceptance criteria

- CLI and MCP round-trip tests exist for `fan_out`, `merge`, `pickup`, and the unclaimed-handoff backlog query.
- Verb names match the canonical vocabulary table in spec R4 exactly.

## Spec

[c737328d Session merge and pickup: handoff-edge provenance graph and first-class tracks](../../.spec/specs/c737328d-a97e-4250-bf9a-390224ab57fd/spec.toml) — R4.

## Parent

Epic [d28afbc0](../../.ticket/tickets/d28afbc0-9d16-4494-8ca5-4154f3ace9be/ticket.toml).