## Overview
Implement durable escalation record and workflow for sub-agents to request capability/offset upward, with async resolution queue.

## Key Design Points
- **Escalation record** (session-api): durable storage
  - Fields: escalation_id, blocking_decision, context, requested_capability/offset, options_considered, resolution_outcome, created_at, resolved_at
- **Sub-agent convention**: return marker `ESCALATION:<record-id>` when blocked
- **Orchestrator resolution actions**: handle it / grant offset / escalate to user / spawn session
- **Resolution outcomes**: recorded in escalation record (resolution_outcome field)
- **Async-capable queue**: escalations can be picked up and resolved later if not handled in-turn
- **Storage**: memory-api/crates/session-api

## Acceptance Criteria
- [ ] Escalation record schema: escalation_id, blocking_decision, context, requested_capability, options_considered, resolution_outcome, created_at, resolved_at
- [ ] create_escalation() operation
- [ ] list_escalations(status: pending|resolved|all) operation
- [ ] resolve_escalation(escalation_id, outcome) operation
- [ ] Marker convention `ESCALATION:<record-id>` documented
- [ ] Resolution outcome enum: handled, granted_offset(grant_id), escalated_to_user, spawned_session(session_id)
- [ ] Async queue: pending escalations can be retrieved and resolved later
- [ ] CLI: session-cli escalation create|list|resolve subcommands
- [ ] MCP: session-mcp escalation tools
- [ ] Tests: CRUD, status filtering, async pickup, outcome recording

## References
- Depends on: T5 (grant records for granted_offset outcome)
- Target: memory-api/crates/session-api, memory-api/tools/cli/session-cli, memory-api/tools/mcp/session-mcp
