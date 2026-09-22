# Goal

Relocate runtime context/handoffs/finish to be owned directly by `sessions/<session_id>/`, move local-only pointers/locks to `.session/local/`, and remove the `runtime/workspaces/` nesting (subticket 2 of the flattening tracker). Depends on identity unification.

# Problem (verified)

- `runtime_paths_for_workspace` (`memory-api/crates/session-api/src/store.rs:1754`) builds `runtime/workspaces/<id>/{context.json,handoffs,finish.json}`.
- `runtime_root` (store.rs:1743) and `active_workspace_session_path` (store.rs:1751) place a machine-local "current thread" pointer in the same ignored tree.
- Captured transcripts live at `sessions/<session_id>/` (`paths_for_session_id`, store.rs:1717).

# Target layout

```
.session/sessions/<session_id>/
    context.json
    handoffs/…            (folder form in subticket 3)
    finish.json
    runs/<run_id>/{transcript.json, events.json}
.session/local/
    active_session.json
    *.lock
```

# Solution Design

1. Rework the path builders so `context.json`/`handoffs/`/`finish.json` resolve under `sessions/<session_id>/`; move captured `transcript.json`/`events.json` under `runs/<run_id>/`.
2. Move the active-thread pointer and mutation lock to `.session/local/` (never git-tracked).
3. Add a back-compat read fallback: if `sessions/<id>/context.json` is absent, read the legacy `runtime/workspaces/<id>/` location. Provide an opt-in migration that relocates legacy threads.
4. Update all readers/writers (`persist_runtime_context`, finish, handoff, capture) and CLI/MCP path outputs.

# Acceptance Criteria

1. New writes land under `sessions/<session_id>/`; nothing writes to `runtime/workspaces/`.
2. Local pointer + lock live under `.session/local/`.
3. Legacy `runtime/workspaces/` records still load via fallback; migration relocates them.
4. Finish immutability semantics unchanged (`ensure_workspace_not_finished`).
5. Focused tests cover new-layout round-trip + legacy fallback.

# Traceability

- Parent: flattening tracker. Depends on identity subticket.