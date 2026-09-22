## Context

Part of the workflow-tools migration roadmap (`transcripts/30-08-2026_spec-system-improvement-planning/ROADMAP.md`, Waypoint 5). Ticket and spec domains are fully migrated (1567/1567 tickets; 254/254 specs — 253 moved, 1 recreated at destination, see ticket 20a07418 for why). Session domain (310 records in `context-engine/.session`) is paused.

## Problem

`session move <id> --store-root context-engine/.session --to-workspace-root meta-workspace` normalizes the destination to the bare `meta-workspace/.session` directory, which is the **live, actively-in-use session store** for this and other concurrent agent sessions — not an isolated `.workflow-tools/session` destination like ticket/spec got. A prior full-scale run against this target caused a live-store collision incident (2 records briefly duplicated into the live store with a crashed/lockless apply); it was fully repaired with no data loss, but the migration was paused rather than retried against the same target.

## Required Work

1. Determine whether `session move` / `session.exe` supports an explicit destination override (e.g. `--to-store-root .workflow-tools/session` or equivalent) that bypasses `--to-workspace-root` normalization to the bare `.session` directory. If not, this is itself a schema/CLI gap to fix before any batch resumes.
2. Once a safe, isolated destination is confirmed, dry-run preflight one throwaway/low-risk record first (mirroring the ticket-domain pilot in `migration-execution.md`'s 2026-08-31T11:42:08Z entry) before resuming the full 310-record batch.
3. Resume the batch protocol from `cutover-runbook.md` (Mandatory Batch Protocol) for the remaining session records, with write-quiescence held against the live store for the duration of each batch.
4. Record every run in `transcripts/30-08-2026_spec-system-improvement-planning/migration-execution.md`.

## Non-Goals

- Do not retarget `--to-workspace-root meta-workspace` against the live `.session` store again without a verified isolated destination.
- Do not touch ticket or spec domain migration (already complete).

## Acceptance Criteria

All 310 `context-engine/.session` records are migrated to an isolated destination store (not the live `meta-workspace/.session` store), verified via post-apply discovery count match, with the live store's own content unaffected (byte-identical before/after for any records overlapping in ID space, none expected).



## Root Cause (confirmed by source inspection)

`workflow-tools/session/crates/session-api/src/move_domain.rs:29` hardcodes:

```rust
const SESSION_INDEX_DIR: &str = ".session";
```

The session domain's `--to-workspace-root` normalization always resolves the destination to `<workspace-root>/.session` and never consults the shared `.workflow-tools/<domain>` canonical-layout resolver in `workflow-tools/memory-kernel/src/discovery.rs` (which ticket-api and spec-api both go through — that's why their moves landed cleanly at `meta-workspace/.workflow-tools/{ticket,spec}` while session's landed at the live bare `meta-workspace/.session`). `session.exe move --help` confirms there is no `--to-store-root`/equivalent override, only `--store-root` for the *source* side.

**Fix options for the owning session to choose between:**
1. Change `SESSION_INDEX_DIR` resolution in `move_domain.rs` to consult the same `.workflow-tools/<domain>` canonical-layout discovery ticket/spec use, so `--to-workspace-root meta-workspace` naturally resolves to `meta-workspace/.workflow-tools/session`.
2. Add an explicit destination override flag (e.g. `--to-store-root`) to `session.exe move`, mirroring the existing source-side `--store-root`, and use that for this migration without changing default normalization behavior.

Option 1 is preferred for consistency with ticket/spec and closes the gap permanently; option 2 is a smaller, more surgical fix if changing default normalization risks other session-domain consumers.


## Correction and Fix Applied (2026-09-03)

The root cause recorded above was **incorrect**. `session-api`'s `move_domain.rs` was re-inspected against `memory_kernel::storage::move_kernel`: `SessionMoveDomain` already implements the same domain-neutral `MoveDomain` trait as `TicketMoveDomain`/`SpecMoveDomain`/`RuleMoveDomain`, `store_index_dir()` already is the harmonized trait function, and `move_kernel::plan_move` already resolves the move destination via `memory_kernel::workspace::resolve_store_root_from`, which is canonical-`.workflow-tools/<domain>`-aware exactly like ticket/spec. There was no move-kernel-level inconsistency.

**Actual root cause:** a *different*, narrower resolver — `session-workspace-resolver::ResolvedWorkspace::store_root()`, used by `session-capture-hook` to compute where to write live session capture data — did a naive `target_root.join(store_dir)` join with no canonical-layout awareness at all. This is what caused `meta-workspace/.session` to exist as a bare/legacy-shaped directory in the first place (this repo's own live session store), which the (correctly-implemented) move kernel then found and reused as the nearest existing store when no `.workflow-tools/session` canonical store existed yet — an entirely reasonable choice given what existed on disk, not a move-kernel bug.

**Fix applied** (harmonizes the *other* resolver with the move kernel's existing behavior, per the requested trait/memory-kernel-centralization approach):
1. Added `pub fn resolve_store_root_at_fixed_workspace(workspace, dir_name) -> PathBuf` to `workflow-tools/memory-kernel/src/workspace.rs`: prefers an existing canonical `.workflow-tools/<domain>` store, falls back to an existing legacy bare store, and defaults to the legacy bare path only when neither exists (preserving every existing caller's current default-creation location — no silent behavior change for repos that haven't provisioned a canonical store).
2. Changed `session-workspace-resolver::ResolvedWorkspace::store_root()` (`workflow-tools/session/crates/session-workspace-resolver/src/lib.rs`) to call this new shared helper instead of the naive join, bounded to the fixed `target_root` (no upward ancestor walk, preserving the worktree-mutation security boundary this resolver exists to enforce).
3. Added test `store_root_prefers_existing_canonical_layout_over_legacy`.

**Validation:**
- `cargo test --manifest-path workflow-tools/memory-kernel/Cargo.toml` — 175 passed, 0 failed (up from 167 in the last recorded run; unrelated crate growth).
- `cargo test --manifest-path workflow-tools/session/Cargo.toml -p session-workspace-resolver` — 23 passed, 0 failed (22 pre-existing + 1 new), confirming zero regression to existing store-resolution/escape-boundary behavior.
- `cargo test --manifest-path workflow-tools/session/Cargo.toml -p session-api --lib move_domain` — 1 passed.
- `cargo test --manifest-path workflow-tools/ticket/Cargo.toml -p ticket-api move`, `--manifest-path workflow-tools/spec/Cargo.toml -p spec-api move`, `--manifest-path workflow-tools/rule/Cargo.toml -p rule-api move`, `--manifest-path workflow-tools/audit/crates/audit-api/Cargo.toml move` — all passed, confirming the shared memory-kernel addition doesn't regress ticket/spec/rule/audit.
- Rebuilt `workflow-tools/target/release/session.exe`; provisioned `meta-workspace/.workflow-tools/session/` (empty canonical destination, mirroring how `.workflow-tools/spec` was provisioned for the spec domain); re-ran the dry-run preflight (`session move 0101b7ef-e717-4c94-bebd-c8d55f6aaa82 --store-root context-engine/.session --to-workspace-root . --dry-run`) — **`target_store_root` now resolves to `.workflow-tools/session`** (previously the live bare `.session`), `supported:true, blockers:[]`. Verified the live store itself was untouched (dry-run only).

## Status

The fix is implemented, tested, and verified end-to-end with a real dry-run against `context-engine`. The 310-record batch migration itself has **not** been run in this pass — per the same operational-safety reasoning as the ticket-domain pilot (large, hard-to-reverse action against a live, shared, concurrently-read store), it should get its own explicit go-ahead and a restore-tested single-record pilot (per `cutover-runbook.md`'s Mandatory Batch Protocol) before the full batch resumes.