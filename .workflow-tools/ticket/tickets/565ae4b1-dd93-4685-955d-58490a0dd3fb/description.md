## Problem

`copilot-capture-hook` exits 0 and emits `{}` for successful provisioning, reuse, skip, and failure alike. Diagnostics go only to stderr, which VS Code discards, leaving silent success and silent failure indistinguishable after the fact. The ambiguity already prevented one real diagnosis.

## Scope

- Centralize the three `println!("{}")` sites in `memory-api/crates/session-capture-hook/src/main.rs` lines 81, 90, and 136 behind one JSON emitter.
- Thread the provisioning outcome and resolved worktree path from `initialize_session_routing`, which currently returns `()` at lines 152-156, through to the emission site. The local `worktree = workspace.target_root()` at lines 197-218 must survive long enough to report the routing result.
- Emit a durable diagnostic record for the provisioning outcome.
- Preserve valid `{}` output when no routing outcome exists.
- Update the `{}` sentinel assertion in `memory-api/crates/session-capture-hook/tests/copilot_capture_hook_e2e.rs` lines 345-352.

## Coordination Constraint

This ticket and ticket `3d535b2c-7361-4f08-bfb4-63b0b3174afc` modify the same emission sites in `memory-api/crates/session-capture-hook/src/main.rs`. Implement the work on the same branch in this order: `3d535b2c` first, then this ticket, to avoid a conflict.

## Acceptance Criteria

- [ ] One JSON-emission helper replaces the three direct `println!("{}")` calls at `memory-api/crates/session-capture-hook/src/main.rs` lines 81, 90, and 136.
- [ ] `initialize_session_routing` exposes a provisioning outcome and resolved worktree path from the routing flow at lines 157-232 to the JSON emitter.
- [ ] Successful provisioning, reuse, skip, and failure each produce distinguishable durable diagnostic evidence while retaining a valid `{}` payload when no outcome exists.
- [ ] `memory-api/crates/session-capture-hook/tests/copilot_capture_hook_e2e.rs` lines 345-352 is updated to cover the retained sentinel behavior and the observable outcome payload.
- [ ] The implementation sequence is documented and followed: ticket `3d535b2c-7361-4f08-bfb4-63b0b3174afc` precedes this ticket on the same branch.