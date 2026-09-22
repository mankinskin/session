# Goal

Stop storing each captured event's payload twice, which makes session review and replay disproportionately expensive.

# Problem (evidence)

`.session/sessions/beca8ec5-.../events.json` is 13,494 lines. Each event stores its payload in both `data_json` and a near-identical `raw_event_json` (the latter repeating the same fields plus `id/timestamp/parentId`). Every `tool.execution_complete` is also shadowed by a separate `tool.execution_result` carrying overlapping data. Prior noise bugs `7769da57` and `c851f3af` are done but addressed different concerns; this duplication is uncovered.

# Code touchpoints (verified)

- `CopilotHookEvent` (`memory-api/crates/session-api/src/hook.rs:32`) declares both `data_json: Option<Value>` and `raw_event_json: Option<Value>`. The wrapper-only fields in `raw_event_json` (`id`, `timestamp`, `parentId`) already map to the struct's `event_id`, `captured_at`, and `parent_event_id` — so `raw_event_json` is fully derivable/redundant.
- `store_helpers.rs:308` fingerprints events via `json_fingerprint(&event.data_json)` — dedup already keys on `data_json`, confirming it is the canonical payload.
- `audit.rs:96` reads `.data_json` (never `raw_event_json`), confirming no reader depends on the raw copy.

# Solution Design

1. Persist `data_json` as the single canonical payload. Strip `raw_event_json` on the capture/persist path (either `#[serde(skip_serializing)]` plus clearing on ingest, or drop the field from the persisted `PersistedSessionEvents` while keeping it transient on the inbound hook DTO for one release for back-compat reads).
2. Collapse `execution_complete` / `execution_result`: dedupe by `(tool_call_id, event_type)`, keeping `execution_complete` as canonical and merging any unique `execution_result` payload into it. Preserve the structured tool-result payload restored by `7769da57` — do not regress to dropping result bodies.
3. Add a back-compat read shim so existing `events.json` files that still carry `raw_event_json` load unchanged.

# Acceptance Criteria

1. New captures persist each event payload under exactly one key (`data_json`); `raw_event_json` is absent from newly written `events.json`.
2. `execution_complete`/`execution_result` no longer both persist overlapping payloads for the same `tool_call_id`, or the retention of both is explicitly documented with the unique data each carries.
3. Session read/query/audit surfaces remain functionally equivalent (existing tests pass; `data_json`-based dedup and audit unaffected).
4. Existing `events.json` files with `raw_event_json` still deserialize.
5. A size-regression check demonstrates a measurable reduction re-capturing a representative session.

# Traceability

- Related: `7769da57-a8f6-4e72-a860-c8263d5a360e`, `c851f3af-433a-496e-a586-28631de142ce`.
- Epic: `effba966-f0a8-4d7d-b289-b7feba826cf8`.
# Review Findings (2026-07-26)

Re-review against the 5 ACs (134 tests pass):
- AC1 (`raw_event_json` absent from new captures) — NOT MET: still serialized in `crates/session-api/src/hook.rs` (~L74) via `skip_serializing_if = "Option::is_none"`, and `store_tests/capture/query_and_worktree.rs` (~L28-72) asserts it persists.
- AC2 (collapse `execution_complete`/`execution_result`) — PARTIAL: dedup exists in `store/helpers/events.rs` (~L29-59) but the unique-payload retention is undocumented.
- AC3 (read/query/audit unaffected) — MET: `audit.rs` (~L109) reads `data_json`.
- AC4 (back-compat deserialize) — MET.
- AC5 (size-regression check) — NOT MET: no size assertion in `store_tests/`.

Remaining work: actually stop persisting `raw_event_json` (and update the asserting test), document AC2 unique-payload retention, and add the AC5 size-regression check. Sent back to implementation as the fresh task.
