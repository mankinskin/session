## Overview
Implement the core data model and computation primitives for empirical tool token metrics derived from session transcripts.

## Key Design Points
- **Data model**: ToolTokenStats (per tool: call_count, success_count, size quartiles), ToolMetricsReport (global rollup), pluggable tokenizer trait with default chars÷4 estimator
- **Per-session summary**: compute tool-metrics.json (sizes/counts ONLY, no content) at capture time, persist beside session.json/transcript.json/events.json
- **Lazy backfill**: scan existing sessions missing tool-metrics.json on first aggregation pass
- **Global aggregation**: window-scoped (last 30 days AND ≤100 most recent sessions, whichever is fewer), reads per-session summaries across ALL discoverable .session stores
- **Failure handling**: exclude failed tool calls from size percentiles; count separately (call_count vs success_count)
- **Privacy**: per-session summaries store sizes/counts only, never transcript content

## Acceptance Criteria
- [ ] ToolTokenStats + ToolMetricsReport structs with serde
- [ ] Tokenizer trait with chars÷4 default impl
- [ ] Per-session tool-metrics.json compute + persist
- [ ] Lazy backfill for sessions missing tool-metrics.json
- [ ] Window-scoped aggregation: 30d AND ≤100 most recent enforced
- [ ] Failed calls excluded from percentiles, counted separately
- [ ] Unit tests: percentile correctness, window logic, failure exclusion, no content leakage
- [ ] No transcript content persisted in tool-metrics files

## References
- Transcript data model: SessionTurn in memory-api/crates/session-api/src/model.rs (~L205)
- Existing pattern: memory-api/crates/session-api/src/audit.rs
- Store layout: store_root/sessions/{session_id}/{session.json, transcript.json, events.json, tool-metrics.json}
