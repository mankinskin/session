# Decision: events.json Tracking Policy

**Date:** 2026-07-28  
**Context:** Ticket 4817a5cc (Git-tracking policy for session artifacts)  
**Decided by:** Implementation Agent (per AC4 requirement)

## Decision

`events.json` files under `.session/sessions/` are **ignored** (not tracked by git).

## Rationale

1. **Size:** Existing committed events.json files total ~99MB across 76 files. Event payloads grow unbounded with tool usage and are not deduplicated.

2. **Durability vs bloat:** Transcripts and handoffs are small, stable, and essential for feedback loops. Events are verbose machine telemetry useful for debugging but not required for session reproduction.

3. **Prerequisite:** Event deduplication (ticket `67d7c279`) must land before bulk-tracking events to prevent repository bloat.

4. **Current state:** Already-committed events.json files are tracked separately by ticket `580019e8` (will be untracked with `git rm --cached`, no history rewrite).

## Implementation

The following patterns in `.gitignore` exclude events:

```
.session/sessions/**/events.json
.session/sessions/**/runs/**/events.json
```

Transcripts (`transcript.json`, `runs/**/transcript.json`) remain tracked as they are lightweight and essential for session replay.

## Future

If event deduplication reduces size by 10x or more, this policy may be revisited to track events for richer debugging context.
