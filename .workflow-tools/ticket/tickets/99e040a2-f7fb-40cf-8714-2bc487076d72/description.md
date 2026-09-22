# Problem

Session-to-worktree assignment state is written to the main checkout at `.session/sessions/<uuid>/session.json`, although transcript capture already writes each session record inside the assigned worktree. The duplicate main-checkout registry keeps the main checkout dirty with other sessions' churn, has blocked `worktree-ctl new` twice, and caused an accidental `git stash` of live session state.

The pollution sources are anchor-registry paths in `memory-api/crates/session-capture-hook/src/main.rs` at lines 344 and 425 and provisioning persistence at line 369. The anchor is `MCP_MAIN_CHECKOUT` when set, otherwise the process CWD (line 242). `UserPromptSubmit` provisioning runs before capture-store resolution (lines 63 and 72-81), so capture already targets the session worktree. Explicit `--store-root` remains authoritative (line 746).

# Confirmed Decisions

1. Remove the main-checkout session-to-worktree index entirely. Discover a session worktree from directory layout alone.
2. New worktrees use `.worktrees/<full-session-uuid>/<slug>`. Exactly one active slug directory may exist for one session id; more than one is an error.
3. New branches mirror the directory: `agent/<full-session-uuid>/<slug>`.
4. Support both layouts during transition. New sessions use nested layout; existing flat worktrees are not migrated and must continue working, including `.worktrees/16263c13-session`, `.worktrees/3a921774-session`, and `.worktrees/23e67e65-worktree-ctl-invariant`.
5. Keep `.session/` git-tracked. Session records written in a worktree are committed on the feature branch and arrive on `main` through normal merge.
6. Federate cross-session readers now: enumerate the main-checkout store plus every `.worktrees/*/.session/sessions/*` store, then route all six named readers through that enumerator.

# Scope Boundary with Related Work

Ticket `674e8e44` owns the earlier assignment-persistence and provisioning-observability repair. This follow-on replaces the main-checkout-registry architecture after that baseline; it does not duplicate the original persistence repair, backfill decisions, or hook-event-observability work. Coordinate shared resolver and provisioning changes with `674e8e44`.

# Ordered Work Units

1. **Nested provisioning layout and dual-layout discovery** in `memory-api/crates/session-worktree-provision`: create new paths and branches using the full UUID nesting; recognize nested and legacy flat worktrees for reuse/reclaim; reject more than one active nested slug for one session id. No in-ticket prerequisite; this establishes the filesystem contract.
2. **Registry-free workspace resolution** in `memory-api/crates/session-workspace-resolver`: remove the anchor `.session` registry as a resolution prerequisite, discover both layouts from `.worktrees`, read assignment metadata from the matched worktree session record, and preserve explicit missing/ambiguous errors plus main-checkout mutation blocking. Depends on work unit 1.
3. **Remove anchor-registry writes** in `memory-api/crates/session-capture-hook`: route provisioning and session persistence only to the assigned worktree store; retain explicit `--store-root` precedence and capture ordering. Depends on work units 1 and 2.
4. **Federated session-store enumerator and reader rewiring** in `memory-api/crates/session-api`: union the main-checkout store and every discovered worktree `.session/sessions` store; route `query_sessions`, `sessions_for_ticket`, `tool_metrics`, `backfill_ticket_links`, `find_handoff`, `list_unclaimed_handoffs`, and safety-critical `SessionStoreActivity` through the enumerator. Duplicate session ids need deterministic handling and an explicit error or documented precedence. Depends on work units 1 and 2; validate reclaim safety before use by provisioning policy.
5. **Worktree CLI layout surface** in `tools/worktree/worktree-ctl`: update `new`, `list`, `rebase`, `merge`, `remove`, `rename`, `finish`, `doctor`, dry-run output, and maintenance tests for full-UUID nested paths while retaining flat-layout operations. Depends on work unit 1.
6. **Guidance updates**: revise `.agents/instructions/session/worktree-provisioning.instructions.md`, `.agents/instructions/session/session-identity-and-handoff.instructions.md`, and `.agents/instructions/commit/branch-worktree.instructions.md` for the nested layout, full-UUID branches, transition compatibility, and no-main-registry rule. Depends on work units 1 through 5.

# Acceptance Criteria

1. A new `UserPromptSubmit` session provisions exactly one `.worktrees/<full-session-uuid>/<slug>` directory and branch `agent/<full-session-uuid>/<slug>`; a focused `session-worktree-provision` test asserts both exact paths.
2. After the new-session provisioning and capture flow, `git status --short .session` executed in the main checkout returns no modified or untracked paths. The test fixture must first establish a clean main-checkout `.session` baseline and then read the command output.
3. Workspace resolution succeeds for a new nested worktree and each listed legacy flat worktree without consulting an anchor `.session` assignment registry; resolver tests prove nested and flat discovery and assert `MissingSessionWorktree` for no match and an ambiguity error for multiple active matches.
4. A single session id with more than one active nested slug directory is rejected deterministically; a provisioning/resolver test asserts the exact ambiguity or duplicate-layout error and proves no arbitrary slug is selected.
5. `session-capture-hook` writes session assignment and capture state only inside the resolved session worktree, preserves explicit `--store-root` precedence, and passes its focused hook tests without adding main-checkout `.session` paths.
6. The federated enumerator returns records from the main checkout and worktree-local stores, and focused tests prove `query_sessions`, `sessions_for_ticket`, `tool_metrics`, `backfill_ticket_links`, `find_handoff`, and `list_unclaimed_handoffs` all see a worktree-local fixture record.
7. `SessionStoreActivity` reads the federated stores; a provisioning-policy test proves a worktree containing a recent or active session record cannot be reclaimed.
8. Every `worktree-ctl` subcommand and its `--dry-run` mode handles the nested full-UUID layout and still handles a legacy flat-layout fixture; the maintenance suite passes.
9. The three named guidance files describe the nested full-UUID directory and branch form, state that no main-checkout registry exists, and document flat-layout transition support; documentation validation passes.
10. Targeted suites for touched crates and `tools/worktree/worktree-ctl/tests/maintenance.rs` pass. Pre-existing unowned failures in `context-read` (7) and `memory-matrix`/`memory-kernel` (2) are excluded and reported separately rather than fixed here.

# Non-Goals

- Migrating existing flat worktrees.
- Untracking `.session/` or changing its merge/commit policy.
- Pushing commits or branches to any remote.
- Fixing the pre-existing unowned test failures in `context-read` (7) or `memory-matrix`/`memory-kernel` (2).

# Evidence Anchors

- Resolver registry-first and discovery behavior: `memory-api/crates/session-workspace-resolver/src/lib.rs` lines 195, 204, and 298; mutation guard at line 92.
- Assignment writer/reader/path construction: `memory-api/crates/session-api/src/store/config/worktree_runtime.rs` lines 1 and 151; `memory-api/crates/session-api/src/store/config/persistence.rs` line 104.
- Active-session marker reader/writer: `memory-api/crates/session-api/src/store/config/persistence.rs` lines 218 and 230.
- Federated-reader targets: `capture_query.rs:119`, `ticket_relation.rs:77`, `tool_metrics.rs:14`, `ticket_backfill.rs:9`, `handoff_pickup.rs:4`, and `session-worktree-provision/src/policy.rs:55`.
- Worktree CLI target and maintenance tests: `tools/worktree/worktree-ctl/src/main.rs` and `tools/worktree/worktree-ctl/tests/maintenance.rs`.
Landed: registry removed, positional discovery implemented and merged to main.