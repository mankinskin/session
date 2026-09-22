## Measured problem

The session store is `.session/sessions/<session-id>/{session.json, transcript.json, events.json, tool-metrics.json}` and contains 238 session directories. The ticket store is `.ticket/tickets/<ticket-uuid>/ticket.toml`.

| Signal | Sessions populated |
| --- | ---: |
| `metadata.ticket_id` | 0 |
| `worktree.branch` | 0 |
| `links.ticket_ids` non-empty | 0 |

Branch-shape inference recovers nothing because the branch field itself is empty. Attribution attributes nothing.

The transcripts contain the missing evidence:

- 163 sessions contain a recorded ticket-MCP tool call carrying a ticket-id argument.
- 44 sessions contain a `ce://` ticket URN.
- 108 sessions contain at least one worked-on call: `update_ticket` 101, `board_check_in` 36, `close_ticket` 13, `cancel_ticket` 3.
- Only 8 sessions contain an `agent/<8hex>-<slug>` or `.worktrees/<8hex>-<slug>` string in text, so branch-shape inference alone is the wrong signal.

## Settled design

Extend `memory-api/crates/session-api/src/store/config/ticket_backfill.rs` with a transcript-signal extractor, additive to the existing branch-shape inference.

### Linked tier -> `links.ticket_ids` (multi-valued)

Union every ticket id appearing in a recorded ticket-tool call's arguments under any of these keys: `id`, `ticket_id`, `from`, `to`, `root`. Also accept `ce://` ticket URNs found in turn content. Accept both full UUIDs and 8-hex short ids; resolve every candidate by prefix against `.ticket/tickets/` and discard any candidate that does not resolve to a real ticket directory. Preserve the existing rule: write nothing that fails to resolve.

### Strict tier -> `metadata.ticket_id` (single-valued)

Populate only from an explicit work-claim call: the `ticket_id` argument of `board_check_in` and `board_check_out`, `board_update_files`, or `board_release_lease`. If exactly one distinct ticket id is claimed across those calls, write it. If zero or more than one ticket id is claimed, leave `metadata.ticket_id` unset and rely on the linked tier. Do not guess a primary by frequency: spot checks show sessions legitimately touch 4-10 tickets each, so a frequency heuristic would fabricate attribution.

### Tool-name matching gotcha

Tool names are server-prefixed and the prefix changed over time: `mcp_ticket-mcp_update_ticket`, `mcp_rmcp5_update_ticket`, and `mcp_rmcp6_get_ticket` all appear in the store and together account for roughly 250 additional references. Match tool names by suffix (`update_ticket`, `get_ticket`, `board_check_in`, and related names), never by the `mcp_ticket-mcp_` prefix, or those sessions are silently dropped.

Read tool calls from both `event_meta.tool_requests_json` and `event_meta.tool_arguments_json`, since either field may carry the payload. Tool-call arguments are nested JSON objects, not double-encoded strings.

## Acceptance criteria

1. `backfill_ticket_links` extracts ticket ids from transcript tool-call arguments in addition to branch shape, matching tool names by suffix so `mcp_rmcp5_*` and `mcp_rmcp6_*` aliases are included.
2. Every extracted candidate is resolved against `.ticket/tickets/` and unresolvable candidates are discarded; the "write nothing that fails to resolve" invariant is preserved.
3. `metadata.ticket_id` is written only when exactly one distinct ticket id is claimed via a board check-in style call; ambiguity leaves it unset.
4. Unit tests cover suffix matching across all three server prefixes, multi-ticket sessions leaving the strict tier unset, single-check-in sessions populating the strict tier, and unresolvable ids being discarded.
5. After running the backfill with `--write` against the real store, the count of sessions with non-empty `links.ticket_ids` is greater than 150 from a baseline of 0, and the count with `metadata.ticket_id` is greater than 30 from a baseline of 0. Both counts are reported as measured numbers read back from the store, not predicted.
6. A newly captured session, recorded after the current `copilot-capture-hook` binary with mtime `2026-08-05 01:30` was installed, is read back from `.session/sessions/` and shown to have a non-empty `worktree.branch`, confirming forward capture-time inference is live. If `worktree.branch` is empty, record the empty result as a blocker on this ticket rather than closing the ticket.

AC6 verifies an already-shipped fix in commit `eacb5bfd`. AC6 is included because a prior report predicted the fix's effect instead of measuring the effect; the two sessions that disproved that prediction were captured before the rebuilt binary was installed.

## Relevant implementation context

`memory-api/crates/session-api/src/model.rs` defines `SessionTurn` with `tool_name: Option<String>` and `event_meta: Option<SessionTurnEventMeta>`. `SessionTurnEventMeta` holds `tool_requests_json: Option<serde_json::Value>` and `tool_arguments_json: Option<serde_json::Value>`. The existing backfill function is `SessionStoreConfig::backfill_ticket_links`; the CLI entry point is `cargo run --manifest-path memory-api/tools/cli/session-cli/Cargo.toml -- backfill-ticket-links --write`. Capture-time inference is in `memory-api/crates/session-api/src/store/config/worktree_capture_inference.rs`, invoked by `memory-api/crates/session-api/src/bin/copilot-capture-hook.rs`.