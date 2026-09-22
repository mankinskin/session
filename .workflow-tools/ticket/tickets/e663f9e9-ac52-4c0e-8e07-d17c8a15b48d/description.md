Implement the first external integration slice for session capture by wiring VS Code GitHub Copilot chat hooks to the existing `session-api` persistence path.

# Implemented Slice
- registered the repo hook file in workspace settings with `chat.hookFilesLocations` so Copilot can load `.github/hooks/docs-validation.json` in this checkout
- extended the root hook config with a non-blocking `Stop` hook that runs `session-capture-stop.sh` alongside the existing `PostToolUse` reminder hooks and the existing docs validation stop hook
- added transcript normalization helpers in `session-api` so a Copilot transcript JSONL file can be converted into the existing `CopilotHookPayload` model while preserving append-only store behavior
- added `capture_copilot_transcript` plus a thin `copilot-stop-hook` binary so the hook script can persist sessions without duplicating store logic in Bash
- added a repo-local `.memory-api/` ignore rule in the `memory-api` git root so hook writes do not create tracked repo dirt

# Validation
- ValidationSpec: focused `session-api` transcript tests, a real stop-hook simulation against the current Copilot transcript, and a full `session-api` crate test run
- ValidationExecution: passed `./target/debug/spec.exe health 09f96d83-4795-4f19-9259-64ad0d452387 --workspace-root . --json`
- ValidationExecution: passed `cargo test -p session-api transcript -- --nocapture`
- ValidationExecution: passed `git -C memory-viewers/memory-api check-ignore -v .memory-api/test/session.json`
- ValidationExecution: passed `printf '{"transcript_path":"C:/Users/linus_behrbohm/AppData/Roaming/Code/User/workspaceStorage/2a3d14caf3f9407a57d20e903c05a6f8/GitHub.copilot-chat/transcripts/2b90cd39-cf55-4840-9b3e-ce38cde2f7b3.jsonl","stop_hook_active":false}' | bash .github/hooks/session-capture-stop.sh && test -f memory-api/.memory-api/sessions/context-engine/2b90cd39-cf55-4840-9b3e-ce38cde2f7b3/session.json && test -f memory-api/.memory-api/sessions/context-engine/2b90cd39-cf55-4840-9b3e-ce38cde2f7b3/transcript.json`
- ValidationExecution: passed `cargo test -p session-api`

# Evidence Trail
- Spec: `09f96d83-4795-4f19-9259-64ad0d452387`
- DocEvidenceRecord candidates: `.vscode/settings.json`, `.github/hooks/docs-validation.json`, `.github/hooks/session-capture-stop.sh`, `memory-api/.gitignore`, `memory-api/crates/session-api/src/hook.rs`, `memory-api/crates/session-api/src/store.rs`, and `memory-api/crates/session-api/src/bin/copilot-stop-hook.rs`
- ValidationLogCapture / ValidationLogRetrieval: the commands above and the resulting persisted session files under `memory-api/.memory-api/sessions/context-engine/2b90cd39-cf55-4840-9b3e-ce38cde2f7b3/`

# Remaining Work
- decide whether a separate `Session End` hook is useful once the `Stop` hook path has proven stable in normal use
- add richer indexing or query surfaces if the append-only filesystem store grows beyond the current simple scan model