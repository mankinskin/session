---
description: "Use when optimizing model-bound Copilot context, tool-result compression, or session bootstrap workflow quality. Covers upstream request shaping, routine-action discipline, and diagnostic transcript analysis."
---


## Session Optimization Guidance

This guidance focuses on reducing what GitHub Copilot sends to the model API before tokens are spent.

### Scope

Apply this guidance when:
- refining agent workflow prompts or instructions
- reviewing session bootstrap or handoff behavior
- designing tool wrappers or context packing helpers
- analyzing captured transcripts to improve future request quality

### Upstream Boundary

`session-api` capture hooks run after Copilot has already sent tokens to the model API. They are diagnostic only.

Use captured transcripts to:
- identify repeated boilerplate and low-value tool chatter
- verify whether prompt and tool changes improved future sessions
- distinguish durable findings from operational noise

Do not treat transcript capture as the mechanism that reduces the cost of the current request.

### High-Confidence Reductions

The highest-confidence ways to reduce model-bound context are:
- compress tool results before any follow-up reasoning step
- avoid routine-action reasoning when the next direct tool call is obvious
- suppress repeated unchanged state checks unless a write or external change happened
- keep long outputs as artifact pointers plus extracted findings
- collapse retry chains into one-line outcomes rather than narrating each failed attempt

### Model Routing Workflow

> **Capability gate:** model routing requires a subagent-capable surface (a `runSubagent` or equivalent tool with a selectable `model`). When no such tool is loadable in the current session, this workflow is inert — the router/subagent pattern below cannot run, so apply the high-confidence reductions above inline instead and do not narrate a delegation plan you cannot execute.

When subagents are available, prefer a workflow where a large, smart model opens the session and acts as a router: it plans and reasons at a high level, then delegates routine subtasks — command batches, summarization of large tool outputs, and research/summarization across many large files or artifacts — to smaller, cheaper models via subagents. Reserve the expensive model for large-scope planning, high-level reasoning, and review of dense content or individual artifacts.

To make routing observable, the `session-api` transcript records the active model responding to each turn (`SessionTurn.model`), inheriting the session-level model when a turn does not specify one. Use this per-turn model signal when analyzing whether expensive-model turns were spent on work that a cheaper model could have handled.

### Tool Result Guarding

Before the model reasons over a tool result, reduce it to the smallest useful shape.

Use this normalized tuple whenever possible:

```text
scope | command | result | blocker | pointer
```

Rules:
- prefer extracted findings over raw output bodies
- use bounded grep, targeted reads, or field selection before exposing large payloads
- drop duplicated tool arguments, lifecycle wrappers, and unchanged status echoes
- keep only the fields needed for the next decision

### Routine-Action Discipline

Do not spend reasoning budget on routine steps that are already implied by the current local hypothesis.

Examples:
- run the single relevant test instead of explaining why it is probably relevant
- rerun a command from the correct directory instead of speculating about cwd drift
- call the already-known tool instead of searching for it again

### Session Artifacts as Evidence

When session files are needed, treat them as evidence and not as default prompt input.

Rules:
- start from tickets, specs, handoffs, and validation summaries before reading raw transcripts
- read the smallest artifact slice that resolves the open question
- preserve durable findings and blockers, not raw event streams
- use transcript content to tune upstream prompt and tool behavior for the next session

### Review Checklist For Remaining Feature Work

When reviewing remaining implementation work for session optimization, confirm that the planned feature:
- reduces model-bound context before Copilot sends the request
- improves tool-result compression or duplicate suppression upstream
- preserves access to raw artifacts through pointers and bounded retrieval
- is validated against representative captured sessions rather than only theoretical examples
