Inject session id into rendered instructions via session_runtime_render_instructions + hook.

**Code anchors**: memory-api/tools/mcp/session-mcp/src/server.rs (session_runtime_render_instructions), memory-api/crates/session-api/src/hook.rs

**Acceptance**: Session id appears in rendered instruction text, verifiable in output.