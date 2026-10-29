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

## Quickstart

```bash
cargo build -p session --bin session --features cli
./target/debug/session --help
./target/debug/session lookup --session-id <uuid> --workspace . --toon
```

See [crates/session-api](crates/session-api) for the underlying model and
[crates/worktree-ctl](crates/worktree-ctl) for worktree lifecycle commands.

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
