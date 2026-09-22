# [session-api] Record active model per transcript turn

Session metadata already carries a single `model` field, but a session may route across multiple models (a large model delegating subtasks to cheaper ones). To make model routing observable, capture the active model at the turn level in the session transcript.

## Implementation
- Added optional `model: Option<String>` to `SessionTurn` (serialized as `model`, omitted when `None`).
- In the capture hook mapping, model-produced turns (`SessionRole::Assistant`) record the session/payload model; user and tool turns leave `model` as `None` and inherit the session-level `SessionMetadata.model`.
- Backward compatible: `#[serde(default, skip_serializing_if = "Option::is_none")]`, so existing transcripts deserialize unchanged.

## Acceptance Criteria
1. `SessionTurn` has an optional `model: Option<String>` serialized as `model`, omitted when `None`. — done
2. Capture hook records the active model on assistant turns; user/tool turns inherit the session-level model. — done
3. session-api tests pass, including per-turn model round-trip and hook-mapping assertions. — done (`cargo test -p session-api`)