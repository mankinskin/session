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

## Handoff path validation

Handoff `--target-file` and path-shaped `--context-anchor` values are
workspace-relative. Validation uses an active assigned worktree when one
exists; otherwise it uses the workspace that owns the selected session store
(canonical `.workflow-tools/session`, legacy `.session`, or an explicitly
relocated store). Absolute paths and parent-directory escapes are rejected.
