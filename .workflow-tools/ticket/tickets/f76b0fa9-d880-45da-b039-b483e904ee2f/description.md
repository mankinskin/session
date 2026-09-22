# [session-api] Add session-cli and session-mcp for session subcommands

## Purpose
Expose the `session-api` capabilities (check-in, lookup, query, range peeking, and skeleton peeking) through dedicated CLI and MCP surfaces so that agents and users can interact with sessions cleanly.

## Scope
- Create a `session-cli` binary or subcommand family under `tools/cli/`
- Create a `session-mcp` server or tool definitions under `tools/mcp/`
- Expose commands/tools for:
  - `session check-in` (worktree check-in)
  - `session lookup` (worktree lookup)
  - `session query` (query sessions)
  - `session peek-range` (range peeking)
  - `session peek-skeleton` (skeleton peeking)
- Ensure proper serialization and error mapping

## Out of Scope
- Implementing the core store logic (handled by previous tickets)

## Acceptance Criteria
1. A `session-cli` tool is available and supports `check-in`, `lookup`, `query`, `peek-range`, and `peek-skeleton` subcommands.
2. A `session-mcp` server exposes corresponding tools with JSON schema validation.
3. CLI and MCP surfaces return compact TOON or JSON outputs.
4. Focused integration tests cover the CLI and MCP tool invocations.

## Validation Plan
- Run integration tests for `session-cli` and `session-mcp`.
- Verify CLI help output and MCP tool definitions.