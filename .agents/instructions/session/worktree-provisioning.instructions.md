---
description: "Use when deciding whether to create a worktree or when registering one for a large implementation task. The capture hook never creates worktrees."
applyTo: "**"
---

## Default Execution Context

Resolve the target repository before considering a worktree. The active VS Code
workspace root supplies the initial scope; a nested repository is selected only
by an explicit task reference resolved from that workspace root and verified
with `git -C <candidate> rev-parse --show-toplevel`. Never infer the target
repository from the location of this instruction file, prompt absolute paths, a
pasted artifact path, or an inherited current directory. “Main checkout” in
this file means the selected target repository main checkout.

The capture hook can provision a session worktree before the session's first tool call. Provisioning makes an isolated checkout available; it does not select the execution mode for the session. An agent uses a worktree only when the task needs explicit isolation under [AGENTS.md](../../../AGENTS.md#task-routing). VS Code loads [.github/hooks/hooks.json](../../../.github/hooks/hooks.json) through the `.chat.hookFilesLocations` setting in [.vscode/settings.json](../../../.vscode/settings.json). The registered binary is `session-capture-hook`, installed on `PATH` at `~/.cargo/bin/session-capture-hook`.

`SessionStart` is the event eager provisioning is primarily attached to, so a provisioned worktree may exist before the first prompt. If `SessionStart` was missed for a session (e.g. hooks were reconfigured mid-session, or the event never fired), the hook lazily provisions instead on the first later event that carries a session id and isn't `Stop` (`UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `SubagentStart`, `SubagentStop`) — `Stop` intentionally never provisions, so a session that never began capturing does not spring a fresh worktree into existence only at its end. The `UserPromptSubmit` timeout is 300 seconds to allow a cold provision.

## When To Create A Worktree

Create a worktree only for worktree-backed implementation work under [AGENTS.md](../../../AGENTS.md#task-routing): overlapping active file ownership, requester-required branch isolation, or a planned Git operation requiring an independent branch. A multi-file change, a submodule, risk, or work expected to span sessions alone does not require a worktree. Small, self-contained work stays in the selected main checkout and does not call `session_check_in` or `board_check_in`.

The agent that will perform the isolated task creates the worktree from the selected main checkout:

```bash
./target/debug/worktree-ctl.exe bootstrap <full-session-uuid> <topic-slug>
```

Use `new` instead of `bootstrap` only when repository initialization must be deferred. The slug is lowercase kebab-case, describes the task, and is 40 characters or fewer. The resulting path is `.worktrees/<full-session-uuid>/<topic-slug>` and the branch is `agent/<full-session-uuid>/<topic-slug>`.

## Register And Route

Immediately after creation, register the assignment with the session tool before editing implementation files:

```text
session_check_in {
  workspace: "<main checkout>",
  session_id: "<full-session-uuid>",
  owner_id: "<agent id>",
  ticket_id: "<full ticket uuid>",
  worktree_path: "<main checkout>/.worktrees/<full-session-uuid>/<topic-slug>",
  branch: "agent/<full-session-uuid>/<topic-slug>"
}
```

Then claim the task's files on the board. The session record is authoritative for routing: registered sessions use the worktree; unregistered sessions use the main checkout. Do not infer or create a worktree from a hook event, a current directory, or a missing assignment.

## Capture Hook

VS Code loads [.github/hooks/hooks.json](../../../.github/hooks/hooks.json) through [.vscode/settings.json](../../../.vscode/settings.json). `session-capture-hook --from-hook-stdin` accepts the Copilot hook payload and emits `{}` on success or intentional skip. It records session events and transcript capture without changing Git worktrees.

The hook uses `MCP_MAIN_CHECKOUT` when set, otherwise its current directory, to locate the main checkout. An explicit `--store-root` is honored. Without one, the resolver selects the registered worktree store when available and otherwise the main checkout `.session` store.

For hook diagnostics, inspect the tracing log at `$TMPDIR/session-capture-hook/session-capture-hook.log` on Unix or `%TEMP%/session-capture-hook/session-capture-hook.log` on Windows. `SESSION_HOOK_LOG_DIR` changes the log directory and `SESSION_HOOK_LOG` or `RUST_LOG` changes its filter.

## Worktree Lifecycle

[worktree-workflow.instructions.md](../commit/worktree-workflow.instructions.md) owns branch creation, check-in, rebase, merge, and teardown. `worktree-ctl` supports `new`, `bootstrap`, `list`, `rebase`, `merge`, `sync`, `rename`, `finish`, `remove`, and `doctor`; run lifecycle mutations from the main checkout.
