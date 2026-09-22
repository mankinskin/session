## Finding

Review of ticket 3c6da958-f494-408f-b7dd-cc43997b8ead found AC1 incomplete: `WorkflowAddNodeInput.category` explains the custom-label redirect in prose but does not show the literal copy-ready field combination.

## Required change

Add `kind="task", category="<your-label>"` to the MCP category schema description and update the relevant schema/error-copy assertion.

## Acceptance criteria

1. Generated MCP schema contains the literal example.
2. Focused session-mcp tests pass.