# session

Copilot session capture, worktree assignment, and durable workflow tracking:
a `session` CLI, a `session-mcp` server, and the `session-capture-hook` that
records session events as they happen. Primary use case: look up which
worktree/branch a session owns, inspect a prior session's transcript, or
record a handoff package between sessions.

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
