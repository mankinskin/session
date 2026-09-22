# [session-api] Fix: Resolve session workspace relative to tool execution

## Purpose
Fix a bug where the `.memory-api` folder is created inside the nested `memory-viewers/memory-api` folder even when the ticket tool is run from the `context-engine` root. The session workspace should be resolved relative to the tool execution root, not hardcoded to the nested crate's directory.

## Scope
- Modify `session-api` store configuration or workspace resolution to dynamically locate the active workspace root
- Ensure that when run from the `context-engine` root, the session store root resolves to `memory-api/.memory-api` relative to that root, or respects the current working directory dynamically
- Align with `memory_api::workspace::resolve_workspace` or similar workspace resolution helpers to find the correct `.memory-api` directory

## Out of Scope
- Changing the internal layout of the `.memory-api` directory (sessions, transcripts, etc.)

## Acceptance Criteria
1. The session store root is resolved dynamically based on the current tool execution context or workspace root.
2. Running the tool from the repository root (`context-engine`) correctly writes session files to `memory-api/.memory-api` (or the active workspace's `.memory-api` directory) instead of creating a duplicate nested folder structure.
3. Workspace resolution is robust across Windows, WSL, and Linux paths.
4. Focused unit or integration tests verify that the resolved store root matches the expected workspace layout.

## Validation Plan
- Run `cargo test -p session-api` to verify all unit tests pass.
- Verify that running a simulated session capture from the root directory writes to the correct `.memory-api` path.