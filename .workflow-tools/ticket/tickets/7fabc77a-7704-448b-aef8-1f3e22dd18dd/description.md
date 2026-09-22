## Objective

Remove the legacy `.session/runtime/` fallback shims now that the flattened `.session/sessions/<id>/` layout is canonical and the legacy tree has been deleted after byte-and-hash verification.

Back-compat for `runtime/workspaces/` was deliberately dropped by user decision during the iteration review of `7a4f9c3d`. That decision amended `7a4f9c3d` AC3 and `0a45bedb` AC5 to no longer require fail-open legacy reads; this ticket carries out the corresponding code removal.

## Scope

- Remove `legacy_runtime_paths_for_workspace` and `legacy_active_workspace_session_path` from `memory-api/crates/session-api/src/store/config/persistence.rs`.
- Remove the legacy fallback branches that call them, notably the `NotFound` fallback in `read_runtime_context` in `memory-api/crates/session-api/src/store/config/worktree_runtime.rs`.

## Acceptance Criteria

1. No symbol named `legacy_*` remains in the session store config module.
2. No code path reads `.session/runtime/`.
3. The `session-api`, `session-mcp`, `session-cli`, and `ticket-api` suites still pass.

## Notes

Other worktrees or machines may still hold a `.session/runtime/` tree. Removal is fail-closed by design per the user decision; stale trees are abandoned, not migrated.

## Incident (2026-07-28)

- On 2026-07-28 a stale `~/.cargo/bin/session-mcp.exe` (installed before the session-store flatten commit) wrote a handoff to the legacy `.session/runtime/workspaces/<id>/` layout, recreating a tree that had been deleted.
- Source code was verified correct; this was a tooling-install staleness issue, not a code regression.
- Consequence for this ticket: when removing the legacy fallback shims, ALSO sweep any residual `.session/runtime/` tree, and consider adding a startup assertion or test that fails loudly if a legacy path is ever written, so stale-binary drift is detected rather than silently tolerated.

## Resolution (2026-07-28)

Implemented and reviewed. Removed `legacy_runtime_paths_for_workspace`, `legacy_active_workspace_session_path`, the now-dead `runtime_root()` helper, and the legacy fallback branches in `resolve_workspace_session_id` and `read_runtime_context`. `read_runtime_context` now surfaces `SessionError::RuntimeContextNotFound` directly. Residual `.session/runtime/` tree swept after confirming its `context.json` and handoff `4e7cbb8d` were content-identical to the relocated copies under `.session/sessions/fe3ca43f-9d1c-4334-b2bf-2324936121f9/`.

Guard added: `writes_never_target_legacy_runtime_tree` in `memory-api/crates/session-api/src/store_tests/runtime/pins_and_workflow_mutation.rs`, plus `read_runtime_context_missing_surfaces_not_found`.

### Accepted limitation

The guard is an in-process test and **would not have caught the incident that motivated it** — that was a separately-installed stale binary running old compiled code, which no test in this crate can reach. The user reviewed this limitation on 2026-07-28 and accepted it as-is with no follow-up ticket: structural removal of the path-builders is considered sufficient, since no code in this repository can now construct a `.session/runtime/` path at all. Do not reopen this as a defect; if stale-binary drift recurs, it is an install-hygiene problem, not a code gap.

Validation at review time: `cargo build --workspace` clean; 334 passed / 0 failed across session-api, session-mcp, session-cli, ticket-api.