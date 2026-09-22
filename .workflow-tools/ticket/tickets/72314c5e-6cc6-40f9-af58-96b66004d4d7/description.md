## Objective

Provide a safe, agent-facing `rename <old-name> <new-name>` command for topic-slug worktree renames.

## Scope

Implement the verified move, top-level repair, and branch-rename sequence in the Rust worktree tool. Preserve five populated submodules, enforce location and dirty-tree safety checks, and support dry runs.

## Done

The acceptance criteria in the `requirements` part pass, including the currently-red rename coverage in `tools/worktree/tests/run.sh`.
Landed: worktree-ctl now exposes a rename subcommand for topic-slug renaming.