CLI commands matching MCP track query and rollup surface.

**Code anchors**: memory-api/tools/cli/session-cli/src/lib.rs

**Acceptance**: CLI can query by track_id and return rollup metrics, parity with MCP.

**Batch 1 validation bar** (shared with 938a7ae9, 185a00a2): End-to-end session lifecycle test covering check-in → handoff write → finish → reload, asserting on-disk layout and track fields, PLUS MCP and CLI round-trip through new track query surface. Base commands: `cargo test -p session-api && cargo test -p session-mcp`, plus memory-kernel board tests.