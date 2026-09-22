# Goal

Collapse the `.session/runtime/workspaces/<workspace_session_id>/` layer so a session **owns** its runtime context and handoffs directly, all sessions share the single `.session/` store, and durable artifacts are git-tracked for feedback/research loops. **Tracker parent — decomposed into 4 subtickets. This track gates all further session-api handoff/schema work.**

# Problem (evidence)

Verified in `memory-api/crates/session-api/src/store.rs`:

- Two decoupled ID spaces: captured sessions live at `.session/sessions/<session_id>/` (`paths_for_session_id`, store.rs:1717); runtime threads live at `.session/runtime/workspaces/<workspace_session_id>/` (`runtime_paths_for_workspace`, store.rs:1754) holding `context.json`, `handoffs/`, `finish.json`.
- `SessionRuntimeContext` (`model.rs:51`) and `SessionRunLineage` carry **no `session_id`**; `SessionLinks` (`model.rs:298`) links to tickets/specs/docs/logs but **not** to a runtime thread. There is no join field between the two trees.
- Relationship is 1 thread : N captured transcripts (resume reuses `workspace_session_id`, appends a `run_id`; each capture is a fresh `session_id`).
- Root `.gitignore` (`.session/*` + only un-ignoring `sessions/`) means `runtime/` — **handoffs, context, finish — is git-ignored**, exactly the artifacts wanted for research loops. `memory-api/.gitignore:5` ignores `.session/` entirely.

# Design (target layout)

```
.session/sessions/<session_id>/        (git-tracked)
    context.json                       pins + workflow + run lineage
    handoffs/<handoff_id>/handoff.json + handoff.md
    finish.json
    runs/<run_id>/{transcript.json, events.json}
.session/local/                        (git-ignored)
    active_session.json                current-thread pointer
    *.lock
```

# Decomposition

1. Unify identity (add `session_id` join) — foundational.
2. Flatten layout + relocate local-only pointers/locks, with back-compat readers.
3. Handoffs as folders (`handoff.json` + rendered `handoff.md`).
4. Git-tracking policy (root + `memory-api` `.gitignore`).

# What must be preserved (verified reasons NOT to over-collapse)

- One directory **per continuity thread** (pins/workflow/lineage/finish are cumulative per thread; multiple threads already coexist).
- Finish immutability per thread (`ensure_workspace_not_finished`, store.rs:717).
- Local-only pointers (`active_workspace_session.json`) and the mutation lock stay git-ignored.

# Acceptance Criteria

1. A session owns its `context.json`, `handoffs/`, and `finish.json` directly under `sessions/<session_id>/`; `runtime/workspaces/` is no longer written.
2. Runtime context and captured transcripts share the `session_id` join.
3. Handoffs persist as folders with JSON + markdown.
4. Durable artifacts are git-tracked; local pointers/locks are ignored.
5. The legacy `runtime/workspaces/` tree is removed and no writer or reader targets it. Code paths must be updated to the new layout.  
Note: Back-compat was deliberately dropped by user decision during the iteration review; its removal and the legacy-path cleanup are tracked in the follow-on ticket created alongside this change.

# Traceability

- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913`.
- Epic: `effba966-f0a8-4d7d-b289-b7feba826cf8`.
- Gates further work: `6431985e`, `96f9ffaa`, `0d3fdba6`, `e731d333`.