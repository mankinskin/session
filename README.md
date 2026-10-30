# session

Copilot session capture, worktree assignment, and durable workflow tracking:
a `session` CLI, a `session-mcp` server, and the `session-capture-hook` that
records session events as they happen. Primary use case: look up which
worktree/branch a session owns, inspect a prior session's transcript, or
record a handoff package between sessions.

## Checkout selection

The main checkout is the default. Agents create or adopt a worktree only when
the current requester explicitly asks for one; overlap, dirty files, and risk
are reasons to coordinate or stop, not implicit permission to isolate.

`SessionStoreConfig::registered_worktree` is the shared assignment lookup.
It reads only the explicit `session_check_in` registry in the main session
store's `local/worktrees/`. Branch-only manifests and leftover directories
are not assignments. MCP and capture routing use `SessionWorkspaceResolver`
to validate that registration, without caching or scanning `.worktrees`.
Missing registrations use the invocation checkout. Broken or inactive
registrations remain errors rather than falling back to main.

### Return an assigned session to main

Use `session unassign` (MCP: `session_unassign`) with the **owning main**
workspace, recorded owner/ticket, exact recorded path/branch, and an audit
reason. This is the explicit recovery path when the assigned checkout has
already been deleted; ordinary calls still fail closed until it succeeds.

```bash
session --workspace /path/to/main --json unassign \
  --session-id <uuid> --owner-id <recorded-owner> --ticket-id <recorded-ticket> \
  --expected-worktree-path /path/to/main/.worktrees/<uuid>/<slug> \
  --expected-branch <recorded-branch> --reason "Requester selected main"
```

The shared `SessionStoreConfig::unassign_worktree` operation checks ownership
and assignment preconditions under the same main-store lock as check-in.
It clears the main manifest's worktree metadata, retains a receipt under
`local/worktree-unassignments/<uuid>/`, and removes the active registry entry
last. Errors restore the registry, manifest and audit snapshots. It does not
delete or modify any checkout content, copied stores, tickets, or transcripts.
The receipt returns `main_checkout`, the previous assignment, timestamp,
reason and `audit_path`. Subsequent unassigned calls use main without cached
routing. Repeating unassignment reports no assignment instead of fabricating
success; a later explicit check-in remains possible.

MCP permits only this recovery operation to target the owning main workspace
or its session store despite a broken assignment. Other operations retain
normal containment checks. Deploy the updated session CLI/MCP server and
`mcp-toolmon` together before relying on recovery.

## Quickstart

```bash
cargo build -p session --bin session --features cli
./target/debug/session --help
./target/debug/session lookup --session-id <uuid> --workspace . --toon
```

See [crates/session-api](crates/session-api) for the underlying model and
[crates/worktree-ctl](crates/worktree-ctl) for worktree lifecycle commands.

## Source layout

Source and test files stay within 500 lines and use responsibility-based Rust
modules, rather than textual `include!` fragments:

- `src/cli/` separates argument families, dispatch, workflow, governance,
  handoff, moves, and parser tests.
- `src/mcp/server/` separates input schemas and tool implementations by domain.
  Each tool module generates a named rmcp router; the server composes those
  routers without changing tool names or schemas. Router parity is tested.
- `src/bin/session-capture-hook/` separates ingress, checkout routing,
  tool-output inspection, and feedback processing.
- `crates/session-api/src/store/` separates storage configuration, transactional
  requester ingestion and receipts, intake filesystem checks, handoff rendering,
  and assignment/runtime operations. `store_tests/` mirrors these behaviors.
- Worktree provisioning, maintenance, supervisor recovery, and telemetry tests
  live in their respective responsibility modules under `crates/`.

## Runtime pins and access errors

Initialize runtime context with `session init --session-id <uuid> --workspace
<root>` (MCP: `session_runtime_init`) before pinning entities. Use the same
session UUID and explicit workspace root for initialization and pin operations.
Pins accept canonical `ce://<workspace>/<store>/<entity>` references, not raw
paths or external URLs; a dossier pin uses `ce://default/dossier/<folder>`.

`session_grant_*` operations adjust the graded cost gate's budget only.
Grants never authorize filesystem access or bypass
`PATH_OUTSIDE_SESSION_WORKTREE`. For a path-boundary rejection, use a path
inside the assigned checkout rather than requesting a budget-offset grant.

## Handoff path validation

Handoff `--target-file` and path-shaped `--context-anchor` values are
workspace-relative. Validation uses an active assigned worktree when one
exists; otherwise it uses the workspace that owns the selected session store
(canonical `.workflow-tools/session`, legacy `.session`, or an explicitly
relocated store). Absolute paths and parent-directory escapes are rejected.
