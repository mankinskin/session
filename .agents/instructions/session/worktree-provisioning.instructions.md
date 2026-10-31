---
description: "Use when explicitly requested worktree isolation needs creation or registration. The capture hook never creates worktrees."
---

## Default Execution Context

Resolve the target repository before considering a worktree. The active VS Code
workspace root supplies the initial scope; a nested repository is selected only
by an explicit task reference resolved from that workspace root and verified
with `git -C <candidate> rev-parse --show-toplevel`. Never infer the target
repository from the location of this instruction file, prompt absolute paths, a
pasted artifact path, or an inherited current directory. “Main checkout” in
this file means the selected target repository main checkout.

The capture hook never provisions worktrees, on `SessionStart` or any later
event. A leftover directory, branch name, captured metadata, or agent-authored
roadmap does not select worktree execution or authorize its creation.

## When To Create A Worktree

Create or adopt a worktree only when the current requester explicitly asks for
worktree isolation. Ownership overlap, dirty files, risk, a submodule, task
size, or an agent-proposed waypoint do not supply that permission. Coordinate
or stop on conflicting ownership; do not silently create a checkout as a
workaround. Otherwise work in the selected main checkout without
`session_check_in`.

The agent that will perform the isolated task creates the worktree from the selected main checkout:

```bash
./target/debug/worktree-ctl.exe bootstrap <full-session-uuid> <topic-slug>
```

Use `new` instead of `bootstrap` only when repository initialization must be deferred. The slug is lowercase kebab-case, describes the task, and is 40 characters or fewer. The resulting path is `.worktrees/<full-session-uuid>/<topic-slug>` and the branch is `agent/<full-session-uuid>/<topic-slug>`.

## Register And Route

Immediately after creation, register the assignment with the session tool before editing implementation files:

```text
session_check_in {
  workspace: "<main checkout>/.worktrees/<full-session-uuid>/<topic-slug>",
  session_id: "<full-session-uuid>",
  owner_id: "<agent id>",
  ticket_id: "<full ticket uuid>",
  worktree_path: "<main checkout>/.worktrees/<full-session-uuid>/<topic-slug>",
  branch: "agent/<full-session-uuid>/<topic-slug>"
}
```

Then claim the task's files on the board. Registered sessions use their
assigned worktree; unregistered sessions use the main checkout.
The authoritative assignment is the session API's explicit check-in registry
under the main session store's `local/worktrees/`, not the branch-only session
manifest. All assignment consumers use `SessionStoreConfig::registered_worktree`;
MCP and capture routing use `SessionWorkspaceResolver` to validate it. Missing
registrations use the main checkout; broken, malformed, or inactive registrations
must not silently fall back there. Do not cache assignments across lifecycle
changes or rediscover them by scanning `.worktrees`.

Treat any nonzero bootstrap result as failure, even if a directory was created.
Do not register or use a partially initialized checkout. A checkout starts from
committed Git state, not the main checkout's uncommitted Ticket/Spec/Test updates;
verify the selected entity authority and revisions before any execution.

## Capture Hook

VS Code loads .github/hooks/hooks.json (`../../../../../context-engine/.github/hooks/hooks.json`) through .vscode/settings.json (`../../../../../context-engine/.vscode/settings.json`). `session-capture-hook --from-hook-stdin` accepts the Copilot hook payload and emits `{}` on success or intentional skip. It records session events and transcript capture without changing Git worktrees.

The hook uses `MCP_MAIN_CHECKOUT` when set, otherwise its current directory, to locate the main checkout. An explicit `--store-root` is honored. Without one, the resolver selects the registered worktree store when available and otherwise the main checkout canonical `.workflow-tools/session` store. Bare `.session` remains a legacy compatibility layout.

For hook diagnostics, inspect the tracing log at `$TMPDIR/session-capture-hook/session-capture-hook.log` on Unix or `%TEMP%/session-capture-hook/session-capture-hook.log` on Windows. `SESSION_HOOK_LOG_DIR` changes the log directory and `SESSION_HOOK_LOG` or `RUST_LOG` changes its filter.

## Worktree Lifecycle

[worktree-workflow.instructions.md](../worktree/worktree-workflow.instructions.md) owns branch creation, check-in, rebase, merge, and teardown. `worktree-ctl` supports `new`, `bootstrap`, `list`, `rebase`, `merge`, `sync`, `rename`, `finish`, `remove`, and `doctor`; run lifecycle mutations from the main checkout.
