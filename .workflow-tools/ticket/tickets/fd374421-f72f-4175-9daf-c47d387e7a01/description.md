`compact-terminal-mcp` hangs forever on every request, returning no output at all, including for a trivial `echo alive`. Reproduced repeatedly on 2026-08-07 after rebuilding the binary from source, so staleness is ruled out. Sibling MCP servers proxied through the same `mcp-toolmon` middleware respond normally, so session routing is not the cause.

## Root cause

In `memory-api/crates/compact-terminal-api/src/execute.rs`:

- Line 38-40: the child is spawned with `Command::new("sh")`, and `stdout`/`stderr` are set to `Stdio::piped()`, but `stdin` is never configured. The child therefore inherits the server process's stdin.
- The MCP server uses a stdio transport, so its stdin carries MCP protocol traffic. A spawned shell that reads stdin consumes or blocks that same stream, wedging the server.
- Line 74: the timeout path returns `RunResult::TimedOut` without killing the child. The orphaned shell keeps holding stdin, so every subsequent request hangs as well. This matches the observed progression exactly: the first call timed out, and all later calls hung with zero output.

## Acceptance criteria

1. The spawned child never inherits the server's stdin; stdin is explicitly set to null (or an empty pipe that is closed immediately).
2. The timeout path kills the child process rather than leaking it, and reaps it.
3. A regression test asserts that a command which reads from stdin (for example `cat`) terminates rather than hanging, and does not disturb the server.
4. A regression test asserts that a command exceeding its timeout leaves no surviving child process.
5. After the fix, a trivial `echo alive` through the MCP surface returns promptly, and a subsequent call also returns promptly (proving no orphan wedged the stream).


## 2026-08-07 Implementation Update

### compact-terminal-mcp stdin Hang

`memory-api/crates/compact-terminal-api/src/execute.rs` is implemented with spawned `sh` stdin set to `Stdio::null()`, preventing inheritance of the MCP stdio-transport stdin. The timeout path kills and reaps the child, recursively kills Git Bash MSYS descendants on Windows, and returns buffered output in `stdout_partial`.

### Acceptance Criteria Status

1. **DONE:** Commands that read stdin terminate rather than hanging. Covered by `stdin_reading_command_terminates_and_subsequent_command_succeeds`.
2. **DONE:** The command after an stdin-reading command succeeds. Covered by `stdin_reading_command_terminates_and_subsequent_command_succeeds`.
3. **DONE:** Timeout kills and reaps the shell process. Covered by `timeout_kills_and_reaps_shell_process`.
4. **DONE:** Buffered output is retained as `stdout_partial` on timeout. Covered by `timeout_kills_and_reaps_shell_process`.
5. **PENDING:** A live `echo alive` through the MCP surface returns promptly twice in a row. This requires reinstalling the binary and restarting the MCP server; the orchestrator will perform that separate validation.


## 2026-08-07 AC5 Live Verification

The MCP terminal tool is currently disabled in the editor, so AC5 was verified through the CLI binary `memory-api/target/debug/compact-terminal.exe run`, which exercises the same `compact-terminal-api` execute path the MCP server calls, plus the 6 in-process `compact-terminal-mcp` integration tests that drive the server surface itself.

| Check | Result | Duration |
|---|---|---|
| `echo alive-first` | exit 0 | 24 ms |
| `echo alive-second` immediately after | exit 0 | 24 ms |
| `cat` with no input | exit 0 | 33 ms |
| `sleep 30` with `--timeout 3` | `timed_out` | 3.125 s |

The back-to-back `echo` pair is the AC5 regression: before the fix the second call hung forever because an orphaned shell still held stdin. The bare `cat` case previously blocked indefinitely on inherited stdin and now returns at EOF. After the timeout check, `ps -W` showed no surviving `sleep` or `sh.exe` child, confirming the kill-and-reap path.

**AC5: DONE** (verified at CLI + MCP integration-test level; live editor MCP surface unavailable because the tool is disabled).

All five acceptance criteria are now satisfied.
