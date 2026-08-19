//! Minimal fixture MCP server, generation "v1".
//!
//! Deliberately a separate source file from `fake-mcp-v2.rs` (not templated
//! from shared source at runtime) so the two compiled binaries are
//! byte-different by construction, letting shadow-copy/reload tests detect a
//! swap by content hash.

use std::io::{
    self,
    BufRead,
    Write,
};

use serde_json::{
    Value,
    json,
};

const GENERATION: &str = "v1";

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut seen_initialize = false;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();

        let response = match method {
            "initialize" => {
                seen_initialize = true;
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "fake-mcp-v1", "version": GENERATION }
                    }
                }))
            },
            "notifications/initialized" => None,
            "tools/list" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "generation",
                        "description": "Returns this fixture binary's generation string",
                        "inputSchema": { "type": "object", "properties": {} }
                    }]
                }
            })),
            "tools/call" if !seen_initialize => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32000,
                    "message": "tools/call received before initialize (handshake replay ordering violation)"
                }
            })),
            "tools/call" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{ "type": "text", "text": GENERATION }]
                }
            })),
            _ => None,
        };

        if let Some(response) = response {
            let _ = writeln!(stdout, "{response}");
            let _ = stdout.flush();
        }
    }
}
