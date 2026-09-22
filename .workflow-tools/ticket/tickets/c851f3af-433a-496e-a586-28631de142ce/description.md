# Summary
Raw session events include high-volume empty assistant-message records during tool-heavy phases, inflating artifacts and obscuring meaningful narration.

## Reproduction context
Session: `a0228f9f-bbac-4c82-b1e6-8a628aa91ec1`
- Count query: `jq '[.events[] | select(.event_type=="assistant.message" and (.data_json.content==""))] | length'` => `24`.
- These events often only carry tool request envelopes with blank content.

## Expected
Assistant-message records should default to semantic content; tool-only placeholder turns should be collapsed or separately typed to reduce noise.

## Actual
Many zero-content assistant-message events are emitted, adding low-value records to downstream session analytics and handoff reconstruction.

## Suggested fixes
1. Event model split:
- represent tool-only emissions as `assistant.tool_plan` (or equivalent) instead of `assistant.message` with empty content.
2. Compaction pass:
- during capture finalize, collapse consecutive empty assistant messages that have no unique semantic text.
3. Analytics hygiene:
- exclude empty assistant messages from default session summaries.
4. Validation:
- add session fixture asserting empty assistant-message count stays below threshold.

## Related specs
- `f5e0df47-d0ec-4456-b268-689f2a41ecd7`
- `9e04ff58-9160-4766-b307-74c0fb32a92c`
