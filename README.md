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
