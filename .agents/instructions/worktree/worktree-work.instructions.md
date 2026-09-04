---
description: "Use while performing edits, builds, and tests inside a worktree-backed task. Covers working-directory discipline, board file-list upkeep, and explicit entity-store targeting for ticket/spec/session writes."
---

## 3. Work

- For a worktree-backed task, every read, edit, build, and test runs with the worktree as the working directory. A command run from the repository root is a bug — it touches the wrong checkout.
- Before the first edit, run the recursive submodule branch check in [worktree-submodule-branch-check.instructions.md](worktree-submodule-branch-check.instructions.md). Do not edit any file — top level or inside a submodule — until every populated submodule prints the expected feature branch.
- Never run `git checkout`, `git switch`, or `git stash` in the repository root from inside an implementation session.
- Keep the claimed file list current with `board_update_files` when scope shifts.
- Refresh `board_heartbeat` before the TTL elapses on long tasks.

## Entity store targeting is explicit

The active-session marker no longer exists. The assigned worktree's
`.session/sessions/<session-uuid>/session.json` manifest carries runtime state,
and agents supply the Copilot session UUID explicitly from the hook payload.

The handoff package MUST separately declare `entity_store_root`. Do not assume
that the code worktree, main checkout, or current directory owns the canonical
ticket, spec, test, or session store. Every state-store command passes the
declared root explicitly. After a write, read the entity back through the same
transport and the same root; success against a different discovered or shadow
store is not evidence that the intended mutation occurred.

`.session`, `.ticket`, and `.spec` are version-controlled, so every worktree carries its own copy. The active copy is the one **inside the session's worktree**. The main checkout's copies are a merge target: they become current only when a branch merges, never by direct edit.

- A session writes entity records only into its own worktree's stores. Writing an active store in the main checkout from an implementation session is forbidden — it forks authority between the store the agent can see and the store it actually wrote.
- Pass the worktree explicitly on every entity CLI call, e.g. `ticket.exe --workspace <worktree> …`. Omitting it falls back to process working directory, which for a long-lived server or a shell started at the repository root is the main checkout.
- The MCP servers are started once by the editor and keep the main checkout as their working directory for the whole session. Until session-anchored resolution lands (ticket `fa2ba34b-59ec-4321-a4ce-a3c3a9295ea3`), a bare `workspace: "default"` MCP write resolves to the **main** store and is therefore unsafe from a worktree. Prefer the CLI with an explicit `--workspace`, and never verify a worktree write by reading it back through MCP — that read resolves to the other store and will confirm the wrong file.
- After any batch of entity writes, confirm the main checkout stayed clean:

```bash
git -C <repo-root> status --porcelain -- .ticket .spec .session
```

  Non-empty output means the write went to the wrong store. Stop and relocate it before continuing.
