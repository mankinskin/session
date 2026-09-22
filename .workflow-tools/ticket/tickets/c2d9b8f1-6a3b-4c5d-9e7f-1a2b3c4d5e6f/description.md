# Problem

`DefaultTicketStateResolver` rejects any entity URN whose workspace slug differs from the session's own workspace slug. The rejection occurs in memory-api/crates/session-api/src/store.rs where code returns an error when `parsed.workspace_slug != self.workspace_slug`. Spec URNs are rejected symmetrically. Each rejection becomes a `SessionWorkflowDiagnostic { code: "ticket-state-unavailable" }` and blocks `create_handoff_record`.

# Impact

Ticket `d1b3a6c9-5f2e-4f6b-9b3c-8fa1e2d3c4b5` made `create_handoff_record` BLOCK on unresolved diagnostics (`SessionError::WorkflowDiagnosticsUnresolved`). Any epic whose tickets live in the nested `memory-api/.ticket` store cannot produce a handoff from a session whose workspace is `default`. Previously generated handoff 087da925 carried 10 such diagnostics, blocking epic `bbb4bce9` (Structured Ticket Entities).

# Root cause

The resolver holds a single `TicketStore` opened at `SessionStoreConfig::ticket_store_root()` (sibling `.ticket`) and does not map workspace slugs to nested store roots. There is no slug -> store-root registry; MCP resolves `workspace` as either the literal `default` or a filesystem path.

# Goal / Acceptance Criteria

1. Add a workspace-slug -> ticket/spec store-root resolution step so `ce://memory-api/ticket/<uuid>` resolves against `memory-api/.ticket` while `ce://default/ticket/<uuid>` continues to resolve against the root `.ticket`. Convention: slug `default` -> `<workspace_root>/.ticket`; slug `S` -> `<workspace_root>/S/.ticket`.
2. Slug validation: reject empty, `.`, `..`, path separators, absolute paths; reuse `SessionError::InvalidWorkspaceSlug` semantics if present.
3. MUST NOT create stores as a side effect. If the resolved store root does not exist, return an `Err` (surfaces as diagnostic) instead of creating directories.
4. Apply resolution symmetrically to `resolve_ticket_state` and `resolve_spec_state`.
5. Preserve same-workspace semantics for `ce://default/...`.
6. Cache opened stores per slug (interior mutability, e.g., `Mutex<HashMap<String, TicketStore>>`) to avoid repeated opens.
7. Tests: nested-memory-api ticket URNs resolve live; unknown/nonexistent slug produces diagnostic and does not create directory; path-traversal slug rejected; `ce://default/...` continues to work.
8. After the fix, `create_handoff_record` for an epic whose tickets live in `memory-api/.ticket` succeeds with zero diagnostics.

# Affected files

- memory-api/crates/session-api/src/store.rs
- memory-api/crates/session-api/src/store/config/persistence.rs
- memory-api/crates/session-api/src/store_routing_types.rs
- memory-api/crates/session-api/src/store/config/workflow.rs
- memory-api/crates/session-api/tests/ (new/updated resolver tests)

# Validation

Run: `cargo test -p session-api`

# Specs

- Durable session workflow graph and handoff continuity: `c677182e-90da-4ac3-8b94-9e2e97c825cf` (linked)


## Implementation complete

- Replaced equality-based cross-workspace rejection in DefaultTicketStateResolver (session-api/src/store.rs) with slug -> store-root resolution via resolve_slug_store_root(session_store_root, session_workspace_slug, slug, sibling_store_dir). Literal `default` and the session's own workspace slug both resolve to the existing sibling root (byte-identical to prior behavior); any other slug resolves to <base>/<slug>/<sibling_store_dir>.
- Slug validated via existing validate_segment(slug, true) (store/helpers/storage.rs), extended to reject `.` and `..` in addition to existing empty/`/`/`\\`/`:` checks, before any path is built.
- Non-own-workspace stores never created: explicit root.exists() check runs before TicketStore::open/SpecStore::open; missing stores return a descriptive Err naming the slug and expected path.
- Opened stores cached per resolved store-root path in Mutex<BTreeMap<PathBuf, TicketStore|SpecStore>> on the resolver, so repeated resolutions within one workflow_snapshot/finish_workflow call reuse the same handle.

Changed files:
- memory-api/crates/session-api/src/store.rs
- memory-api/crates/session-api/src/store/config/persistence.rs
- memory-api/crates/session-api/src/store/helpers/storage.rs
- memory-api/crates/session-api/src/store_tests/finish/ticket_enforcement.rs (new tests)

New tests: workflow_finish_resolves_ticket_from_nested_workspace_store, workflow_finish_rejects_unknown_workspace_slug_without_creating_store, workflow_finish_rejects_path_traversal_workspace_slug.

Validation: cargo test -p session-api -> 184 passed, 0 failed (lib) + all integration binaries passed.