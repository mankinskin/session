## Objective
Preserve session-anchored MCP routing without persisting a root-store dependency on another worktree's path or contents.

## Required behavior
- Direct CLI remains CWD/explicit-selector based and does not use session routing.
- MCP proxy resolves only the calling session's assigned worktree.
- A deleted assignment cannot poison unrelated session or main-checkout operations.
- Session records do not retain foreign-worktree absolute paths as active dependencies; legacy records are reconciled safely.
- Worktree reclaim does not inspect another worktree's stores or artifacts.

## Owning paths
- `memory-api/crates/session-api/src/store/config/worktree_runtime.rs`
- `memory-api/crates/session-api/src/store/config/worktree_capture_inference.rs`
- `memory-api/crates/session-workspace-resolver/src/lib.rs`
- `memory-api/tools/mcp/mcp-toolmon/src/proxy.rs`
- `memory-api/crates/session-worktree-provision/src/policy.rs`

## Validation
Add resolver and session-store tests covering deletion of an assigned worktree, unrelated session continuity, and the CLI/MCP routing boundary.

## Parent
Implements one slice of ticket `3a624bf6`.
Obsolete: registry-backed session-to-worktree assignments no longer exist; discovery is positional as of ticket 99e040a2.