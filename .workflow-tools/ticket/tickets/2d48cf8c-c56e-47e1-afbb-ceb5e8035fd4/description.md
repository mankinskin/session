## Problem

The repository consistently emits `hookSpecificOutput.additionalContext`, but no repository evidence proves that VS Code injects the field into the model prompt for `UserPromptSubmit`, and no repository documentation states the runtime hook stdout contract. Building a worktree announcement on an unverified contract risks shipping a no-op. The current bootstrap proposal is ticket `3d535b2c-7361-4f08-bfb4-63b0b3174afc`, which plans to emit the assigned worktree through `hookSpecificOutput.additionalContext`.

## Scope

- Run a controlled live experiment that emits distinguishable `hookSpecificOutput.additionalContext` values on `UserPromptSubmit` and records whether the agent receives the value.
- Run the corresponding controlled live experiment for `PostToolUse` and record whether the agent receives the value.
- Record empirical evidence from both experiments; code inspection, a simulated invocation, or hook stdout alone is insufficient.
- Document the confirmed runtime contract in `.agents/instructions/session/worktree-provisioning.instructions.md`, whose lines 15-18 currently state only that capture-hook stdout is `{}` and diagnostics go to stderr.

## Acceptance Criteria

- [ ] A controlled `UserPromptSubmit` experiment records the emitted `additionalContext` marker and independently records whether the marker is visible to the agent.
- [ ] A separate controlled `PostToolUse` experiment records the emitted `additionalContext` marker and independently records whether the marker is visible to the agent.
- [ ] The stored evidence identifies the VS Code version, hook event, emitted marker, observation method, and result for each experiment.
- [ ] `.agents/instructions/session/worktree-provisioning.instructions.md` is updated only with the experimentally confirmed runtime contract and its limitations.
- [ ] Ticket `3d535b2c-7361-4f08-bfb4-63b0b3174afc` remains blocked until the `UserPromptSubmit` result establishes whether its `additionalContext` mechanism can reach the agent.