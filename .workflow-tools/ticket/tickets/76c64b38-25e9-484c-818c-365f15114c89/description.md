# Problem (verified)

Root `session.exe init --workspace . --toon` returns the same value for both `session_id` and `workspace_session_id` — a stale slug (`epic-kickoff-8fdfe135`), not a fresh UUID. This is not a display artifact: it traces to real code that never mints a distinct session UUID.

- `memory-api/crates/session-api/src/store/config/worktree_runtime.rs:222-223` initializes runtime `session_id` as `workspace_session_id.clone()` — no distinct UUID is minted at runtime-context creation.
- `memory-api/crates/session-api/src/store/config/persistence.rs` reuses `.session/local/active_workspace_session.json` unconditionally whenever it exists, so a stale slug marker from a prior conversation is silently re-adopted as the current session's identity with no staleness check and no fresh-UUID minting path.
- Slug-shaped values are accepted as a valid `session_id`/`workspace_session_id` by `memory-api/crates/session-api/src/store_routing_types.rs`, so nothing rejects the slug at the type level either.

Consequences observed on this checkout:

- `lookup` and `check-in` require a UUID, but `init` handed back a slug for both fields, so downstream commands cannot use `init`'s own output.
- The already-provisioned worktree `.worktrees/a2659767-3224-48e2-a9a9-72c9582c8515/workspace-policy-refactor` is named for UUID `a2659767-3224-48e2-a9a9-72c9582c8515`, yet no matching session record exists for that UUID — the provisioner's identity and the CLI's identity have drifted apart.
- Running `init` explicitly scoped to that worktree minted a *different* new UUID (`a808ee90-191a-47e2-a609-4a49a967d778`) instead of resolving the provisioner's `a2659767...` UUID, proving the CLI cannot consume the identity the worktree provisioner already established.

## What must NOT be touched by this fix

- `.session/sessions/epic-kickoff-8fdfe135` and `.session/sessions/structured-ticket-entities-iteration` are valid runtime-context-only records (`context.json` + handoffs), not malformed UUID session records. They must be preserved as-is; this ticket is not a migration and must not delete or rewrite them.
- This ticket does not cover the guidance-only protocol in [7be23bd8 Agent session identity, worktree traceability, and prior-session inspection protocol](../.ticket/tickets/7be23bd8-9793-4f86-a96d-403824f8af94/ticket.toml) (in-review, `agent-guidance` component, root ticket store) — that ticket documents the *intended* distinct-UUID model as an instruction/protocol; this ticket is the code defect that makes the documented model false in practice.
- This ticket does not re-open [fc86f42d Unify session identity: link runtime context to session_id and stamp transcripts](fc86f42d-3fc0-4f07-911a-525098248dcf/ticket.toml) or [0a45bedb Flatten session store layout, unify identity, and git-track durable artifacts](0a45bedb-6dfe-466e-893f-fddfd225f1f6/ticket.toml) (both `done`). Those tickets intentionally introduced the `session_id` alias and the flattened runtime/session layout; they did not scope or claim to guarantee UUID-vs-slug distinctness, which is the specific gap this ticket closes.

# Scope Boundaries

In scope:
- Decouple the Copilot-conversation session UUID from the workspace-local runtime context identity so they are never silently equal by construction.
- Make `init` (and any equivalent capture/MCP entry point) mint or resolve a real session UUID distinctly from the workspace runtime context slug/marker.
- Make the worktree bootstrap path consume the same UUID the provisioner already established, instead of minting a second, unrelated UUID.
- Make UUID-only commands (`lookup`, `check-in`, etc.) fail fast and descriptively when handed a slug, instead of silently accepting it or failing with an unrelated storage error.
- Add regression coverage that pins both failure modes reproduced here: a stale marker file being reused across conversations, and a provisioned worktree UUID not matching what `init` resolves against that worktree.

Out of scope:
- Any change to `.agents/instructions/**` guidance content (owned by ticket 7be23bd8).
- Any deletion, rewrite, or migration of existing `.session/sessions/*` records, including the two runtime-context-only records named above.
- Broader session-store layout changes (owned by the completed `fc86f42d`/`0a45bedb` track).
- Fixing unrelated `ticket-mcp` main-checkout-mutation guard behavior encountered while investigating this ticket (that guard behaved as designed and is not part of this defect).

# Acceptance Criteria

1. A fresh Copilot session (captured via the capture hook or an equivalent MCP entry point) is assigned a session UUID that is minted or sourced from the capture/MCP context itself, not derived by cloning the workspace runtime context's slug.
2. The workspace-local runtime context (the persisted marker plus its slug-shaped `workspace_session_id`) remains a distinct value from the session UUID; the two are never the same value by construction, only by an explicit, intentional join field if one is later added.
3. `session.exe init` (and any MCP equivalent) emits both identifiers in its output with explicit, distinguishable semantics — e.g. `session_id` (UUID) versus `workspace_session_id` (slug) — and the two fields are not permitted to silently collapse to the same string.
4. The worktree bootstrap/provisioning path consumes the same session UUID that `init`/capture already established for the active conversation, rather than minting an unrelated second UUID when scoped to an already-provisioned worktree.
5. Any command that requires a UUID (`lookup`, `check-in`, and equivalents) fails early with a descriptive, actionable error when given a slug-shaped value, instead of either silently accepting it or surfacing an unrelated storage error.
6. Reuse of the active marker file (`.session/local/active_workspace_session.json`) cannot leak one conversation's session identity into a different, later conversation — reuse must be scoped/validated (e.g., freshness or an explicit binding check) rather than unconditional.
7. Regression tests cover: (a) a stale slug marker present at startup does not get adopted as a fresh session's UUID, and (b) `init` scoped to a worktree that a provisioner already assigned a UUID to resolves that same UUID rather than minting a new one.
8. No existing valid `.session/sessions/*` record (in particular `epic-kickoff-8fdfe135` and `structured-ticket-entities-iteration`) is deleted, migrated, or restructured by this change.

# Proposed Validation Commands

- `cargo test -p session-api` — unit/integration coverage for the identity/runtime-context split and the new regression tests.
- `./target/debug/session.exe init --workspace . --toon` — confirm `session_id` and `workspace_session_id` are no longer identical for a stale-marker case, and that both carry explicit semantics.
- `./target/debug/session.exe init --workspace .worktrees/a2659767-3224-48e2-a9a9-72c9582c8515/workspace-policy-refactor --toon` — confirm this resolves the provisioner's existing UUID (`a2659767-3224-48e2-a9a9-72c9582c8515`) rather than minting a new one.
- `./target/debug/session.exe lookup --session-id <slug> --workspace . --toon` — confirm a slug-shaped input now fails fast with a descriptive error rather than an unrelated storage error.
- Manual repro check: verify `.session/sessions/epic-kickoff-8fdfe135` and `.session/sessions/structured-ticket-entities-iteration` are unchanged (mtimes/content) after the fix.

# Traceability

- Guidance-only sibling: [7be23bd8 Agent session identity, worktree traceability, and prior-session inspection protocol](7be23bd8-9793-4f86-a96d-403824f8af94/ticket.toml) (in-review, root ticket store, `agent-guidance`).
- Completed foundation: [fc86f42d Unify session identity: link runtime context to session_id and stamp transcripts](fc86f42d-3fc0-4f07-911a-525098248dcf/ticket.toml) (done).
- Completed foundation: [0a45bedb Flatten session store layout, unify identity, and git-track durable artifacts](0a45bedb-6dfe-466e-893f-fddfd225f1f6/ticket.toml) (done).
- Owning spec (parent, draft): [8c880efc Dynamic session bootstrapping and just-in-time context routing](../.spec/specs/8c880efc-7083-4e1d-bf06-96b8254be913/spec.toml) (root spec store, `session-api` component).

Incident: identity aliasing. `init_runtime_context` assigned `session_id` = `workspace_session_id`. The active marker at `.session/local/active_workspace_session.json` then persistently reused that value, including stale slugs such as `epic-kickoff-8fdfe135` (current on-disk content: {"workspace_session_id":"epic-kickoff-8fdfe135","updated_at":"2026-08-12T10:35:58.180321300Z"}). So `session init` returned a stale slug as `session_id`. Worktree provisioning used the real UUID a2659767-3224-48e2-a9a9-72c9582c8515, while init later minted a DIFFERENT UUID a808ee90-191a-47e2-a609-4a49a967d778 inside that same worktree. Check-in requires the provisioning UUID, so the bootstrap chain was incompatible with itself.
Design decision (user-made): `session_id` is the SINGLE session identity handle and its value is the capture/provisioning UUID. `workspace_session_id` is eliminated as a distinct identity; the slug survives only as a non-identity display label.
Defect anchors: memory-api/crates/session-api/src/store/config/persistence.rs ~line 208 reuses the active marker UNCONDITIONALLY; memory-api/crates/session-api/src/store/helpers/storage.rs ~line 267 validates session_id only as a safe path segment with no UUID shape check. Runtime context already persists to `.session/sessions/<key>/context.json` (persistence.rs ~line 146), the same directory family as session.json (~line 108) — so storing runtime context under the session is a KEYING fix, not a storage migration.
Scope note: this ticket covers the CORRECTNESS FIX only (distinct UUID minting, marker reuse validation, UUID shape validation, worktree parity, regressions). Mechanical removal of the `workspace_session_id` identifier is split into the follow-up ticket.
Related spec: 709f067a-21b6-41b6-8879-3cacef4bacaf (Runtime session-context model).
## Completion Record

Completed on local `main`.

- Superproject `main`: `c98340463c34f78ad2aeffaf27790fe73f973184` (`c9834046`).
- `memory-api` submodule `main`: `7ac740bc2fd140a6353458d4256eb2bce40cde93` (`7ac740bc`); the superproject gitlink was verified to match.
- Validation: 6/6 packages green, 0 failures.
- Regression coverage includes `validate_session_id_rejects_slug_shaped_id` and `handoff_record_deserializes_legacy_workspace_session_id_alias`.

Residual items (reported, not changed by this ticket closure):

- The main checkout retains inert untracked/gitignored leftovers: one untracked `context.json`, `.session/sessions/epic-kickoff-8fdfe135/`, and `.session/local/active_workspace_session.json`. No code reads these paths.
- `tools/agent-hooks/hook-capture-e2e.sh` remains locally blocked because the Copilot CLI did not have the repository in `trustedFolders`; the repository was added but the check is unverified.