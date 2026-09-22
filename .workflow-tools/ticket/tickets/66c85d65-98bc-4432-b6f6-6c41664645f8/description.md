## Overview
Expose tool_metrics computation via CLI and MCP surfaces, and wire automatic rollup refresh on session persist.

## Key Design Points
- **CLI surface**: `session tool-metrics` command with --toon/--json/--export, window flags (--days, --max-sessions)
- **MCP surface**: `session_tool_metrics` tool matching CLI contract
- **Rollup writer**: `--export` writes schema-versioned `tool-metrics-rollup.json` to session store root
- **Capture hook integration**: refresh rollup after each session persist (copilot capture hook invokes aggregation + export)

## Acceptance Criteria
- [ ] `session tool-metrics` CLI command with --toon, --json, --export
- [ ] Window flags: --days (default 30), --max-sessions (default 100)
- [ ] `session_tool_metrics` MCP tool with same parameters
- [ ] `--export` writes tool-metrics-rollup.json with schema_version field
- [ ] Capture hook calls aggregation + export on session persist
- [ ] Tests: CLI/MCP output parity, rollup schema stability, hook invocation
- [ ] Rollup location: <store_root>/tool-metrics-rollup.json

## References
- Existing pattern: memory-api/tools/cli/session-cli/src/lib.rs, memory-api/tools/mcp/session-mcp/src/server.rs
- Store method: session_audit in store impl
