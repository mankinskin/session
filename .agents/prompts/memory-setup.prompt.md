---
description: "Welcome the user to the workspace, explain the available workflow tools, or help bootstrap a fresh checkout."
name: "memory-setup"
argument-hint: "[fresh-checkout|current-workspace|tools]"
agent: "agent"
---

# Memory Setup

Help the user orient in the current checkout or bootstrap a fresh one without skipping the repository workflow.

Reference [README](../../README.md), [AGENTS](../../AGENTS.md), [session-optimization instructions](../instructions/session/session-optimization.instructions.md), [spec-cli](../../memory-api/tools/cli/spec-cli/README.md), [ticket-cli](../../memory-api/tools/cli/ticket-cli/README.md), [audit-cli](../../workflow-tools/audit/crates/audit-cli/README.md), and [viewer-ctl](../../viewer-api/viewer-ctl/README.md).

## Workflow

1. Read the slash-command text and determine whether the user wants:
- a fresh checkout setup
- current workspace orientation
- a tour of the available workflow tools
2. Inspect the current repository layout and discover the nearest `.ticket` and `.spec` stores before giving setup advice.
3. For a fresh checkout, guide the user through the minimum useful bootstrap commands:
- build or install `spec`, `ticket`, and `audit`
- identify any viewer or browser helpers worth starting
- call out missing prerequisites explicitly instead of assuming they are installed
4. For an existing workspace, summarize the relevant workflow surfaces:
- ticket planning and board state
- spec authoring and health checks
- documentation and log viewers
- focused validation and audit tooling
5. When prior session artifacts matter, treat them as diagnostic evidence for improving future prompt quality:
- start from tickets, specs, handoffs, and validation summaries before reading transcripts
- extract durable findings, blockers, and next actions instead of replaying raw tool chatter
- emphasize upstream tool-result compression and routine-action discipline as the real cost levers
6. Prefer concrete commands and paths over generic setup advice.
7. Ask one concise clarification only when the desired setup mode is still ambiguous after a focused inspection.

## Response

Return a concise setup summary containing:
- detected workspace state
- available workflow tools and what each is for
- missing prerequisites or binaries, if any
- the single best next action for the user
