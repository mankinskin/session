Problem:
The rendered handoff markdown document only summarizes the workflow snapshot as counts. Current output in .session/sessions/78a67ddb-1d98-4f9f-8d41-4d938bb07b35/handoffs/087da925-e599-4e95-a96f-69a86433b2ed/handoff.md is:

## Workflow
- **Nodes**: 16
- **Edges**: 24
- **Not Done**: 13

A reader cannot see the workflow shape without running a separate render tool.

Goal / Acceptance Criteria:
1. The handoff markdown renderer `render_handoff_record_markdown` in `memory-api/crates/session-api/src/store.rs` emits a fenced ```mermaid flowchart of the handoff record's workflow graph inside the `## Workflow` section, in addition to the existing counts.
2. The mermaid source is produced by a pure function that renders a `SessionWorkflowGraph` (`memory-api/crates/session-api/src/model/workflow.rs`, `SessionWorkflowGraph` and `SessionWorkflowSnapshot`) plus node resolutions into mermaid text, refactored out of the existing store-scoped `workflow_render_mermaid` in `memory-api/crates/session-api/src/store/config/workflow.rs` so both call sites share one implementation. Existing behavior of `workflow_render_mermaid` (bare mermaid source, no fence) must be preserved for its CLI/MCP callers.
3. Empty workflow graph renders no mermaid block (no empty fence).
4. Test coverage added in `memory-api/crates/session-api/tests/handoff_folder_storage.rs` asserting the handoff markdown contains a ```mermaid fence and expected node/edge lines.
5. The handoff requirements are updated: the Handoff Package Schema spec `.spec/specs/5e52039d-aabc-434d-bdf3-eca63e312476/body.md` and the agent contract `.agents/agents/handoff.agent.md` require the rendered mermaid workflow graph in the markdown handoff.
6. The existing handoff document for handoff `087da925-e599-4e95-a96f-69a86433b2ed` is updated in place to contain the rendered mermaid graph derived from its own `handoff.json` workflow snapshot.

Affected files:
- memory-api/crates/session-api/src/store.rs
- memory-api/crates/session-api/src/store/config/workflow.rs
- memory-api/crates/session-api/src/model/workflow.rs
- memory-api/crates/session-api/tests/handoff_folder_storage.rs
- .spec/specs/5e52039d-aabc-434d-bdf3-eca63e312476/body.md
- .agents/agents/handoff.agent.md
- .session/sessions/78a67ddb-1d98-4f9f-8d41-4d938bb07b35/handoffs/087da925-e599-4e95-a96f-69a86433b2ed/handoff.md

Validation:
- Run `cargo test -p session-api` and ensure tests pass for the updated behavior.

## Status
- Changed: memory-api/crates/session-api/src/store.rs (new pure `render_workflow_mermaid`; `render_handoff_record_markdown` emits a fenced ```mermaid block in `## Workflow` when the graph has nodes), memory-api/crates/session-api/src/store/config/workflow.rs (`workflow_render_mermaid` now delegates to the shared pure renderer, signature and bare-source output unchanged), memory-api/crates/session-api/tests/handoff_folder_storage.rs (new tests `handoff_markdown_includes_workflow_mermaid_diagram_when_nodes_exist`, `handoff_markdown_omits_mermaid_diagram_when_workflow_empty`).
- Requirements updated: .spec/specs/5e52039d-aabc-434d-bdf3-eca63e312476/body.md (new "Rendered markdown" section requiring the mermaid flowchart), spec.toml ticket_ids now links this ticket, .agents/agents/handoff.agent.md (Output Format "Workflow graph" bullet + constraint), .agents/prompts/handoff.prompt.md (response requirement, mermaid fence nests inside the ~~~text fence).
- Handoff 087da925-e599-4e95-a96f-69a86433b2ed regenerated at .session/sessions/78a67ddb-1d98-4f9f-8d41-4d938bb07b35/handoffs/087da925-e599-4e95-a96f-69a86433b2ed/handoff.md — 93 -> 156 lines, `## Workflow` now contains a `flowchart TD` mermaid diagram with 16 nodes, 24 edges, and 10 diagnostic nodes; all 11 section headings preserved.
- Validation: `cargo test -p session-api` — all suites pass, 0 failed.
- Limitation to note: the rendered diagram surfaces 10 `ticket-state-unavailable` diagnostics caused by cross-workspace ticket routing (URN workspace `memory-api` vs session workspace `default`); this is pre-existing session-workflow behavior, not introduced by this ticket.