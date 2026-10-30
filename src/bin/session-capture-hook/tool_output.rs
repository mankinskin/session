#[allow(unused_imports)]
use super::*;

/// Build the layered output-size override for the tool call that triggered
/// this hook invocation (ticket 44119807 T2).
///
/// The hook stdin's `tool_use_id` is the full on-disk spill entry name,
/// `<bare_id>__vscode-<epoch>`, while the transcript's own `toolCallId` is
/// the bare id without that suffix. The two must be split apart: the bare id
/// is what `apply_tool_response_override` matches against transcript events,
/// while the full suffixed id is the literal spill directory name.
///
/// Layer 1 (`hook_payload`): the hook stdin's `tool_response` string, used
/// only when non-empty (observed to be populated for some tool types, e.g.
/// `run_in_terminal`, and empty for others, e.g. `read_file`).
///
/// Layer 2 (`spill_file`): VS Code Copilot Chat spills large tool outputs to
/// `<workspaceStorage>/<hash>/GitHub.copilot-chat/chat-session-resources/
/// <session_id>/<tool_use_id>/content.txt` (or `content.json`), derived here
/// from the hook stdin's own `transcript_path` (its
/// `GitHub.copilot-chat/transcripts/<session>.jsonl` layout shares the same
/// `GitHub.copilot-chat` root) plus `session_id` and the full `tool_use_id`.
pub(super) fn build_tool_response_override(
    tool_use_id: Option<&str>,
    tool_response_chars: Option<u64>,
    session_id: Option<&str>,
    transcript_path: &Path,
) -> Option<ToolResponseOverride> {
    let tool_use_id = tool_use_id?;
    let bare_tool_call_id = tool_use_id.split("__vscode-").next().unwrap_or(tool_use_id);

    if let Some(output_chars) = tool_response_chars.filter(|chars| *chars > 0) {
        return Some(ToolResponseOverride {
            tool_call_id: bare_tool_call_id.to_string(),
            output_chars,
            output_source: "hook_payload".to_string(),
        });
    }

    let session_id = session_id?;
    let output_chars = stat_spill_output_chars(transcript_path, session_id, tool_use_id)?;
    Some(ToolResponseOverride {
        tool_call_id: bare_tool_call_id.to_string(),
        output_chars,
        output_source: "spill_file".to_string(),
    })
}

/// Stat the `chat-session-resources/<session_id>/<tool_use_id>` spill entry
/// relative to the hook stdin's `transcript_path`
/// (`.../GitHub.copilot-chat/transcripts/<session>.jsonl`). `tool_use_id` is
/// the full suffixed id (`<bare_id>__vscode-<epoch>`), matching the literal
/// on-disk directory name. Returns `None` (unmeasured, never a fabricated
/// zero) when the root can't be derived or no spill file is found.
///
/// VS Code writes the spill file asynchronously after invoking the
/// PostToolUse hook, so the file can be briefly absent at hook-fire time;
/// this retries a few times with a short backoff before giving up.
pub(super) fn stat_spill_output_chars(
    transcript_path: &Path,
    session_id: &str,
    tool_use_id: &str,
) -> Option<u64> {
    let chat_root = transcript_path.parent()?.parent()?;
    let entry_dir = chat_root
        .join("chat-session-resources")
        .join(session_id)
        .join(tool_use_id);

    const MAX_ATTEMPTS: u32 = 5;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);
    for attempt in 0..MAX_ATTEMPTS {
        if let Some(candidate) = ["content.txt", "content.json"]
            .iter()
            .map(|name| entry_dir.join(name))
            .find(|path| path.is_file())
        {
            let bytes = std::fs::read(&candidate).ok()?;
            return Some(String::from_utf8_lossy(&bytes).chars().count() as u64);
        }
        if attempt + 1 < MAX_ATTEMPTS {
            std::thread::sleep(RETRY_DELAY);
        }
    }
    None
}
