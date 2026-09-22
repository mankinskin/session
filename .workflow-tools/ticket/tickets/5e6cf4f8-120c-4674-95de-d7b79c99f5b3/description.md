## Objective

Replace `tools/worktree/worktree.sh` with a Rust binary that drives git through
a library rather than shelling out, and give worktrees a lifecycle: locked
while a session is active, reclaimed when it completes and the worktree holds
nothing worth keeping.

## Why a rewrite

`worktree.sh` is a 500-line bash script encoding subtle git invariants —
offline submodule population via per-submodule linked worktrees, refusal to
fall back to a non-fast-forward merge, never touching `origin`. Its test suite
is 16 shell scripts, 10 of which fail. Those 10 are not flaky; they describe
lifecycle behavior the script never implemented. Rather than grow the script to
meet them, the contract moves to Rust with tests in Rust.

The existing shell suite is prior art, not a spec to satisfy in place. Its
intent carries over; the scripts and their `common.sh` harness retire with the
script they exercise.

## Lifecycle rules

A worktree is locked while its session is active and is never purged or
repurposed during that time.

On session completion, evaluate the worktree:

| State | Outcome |
|---|---|
| Uncommitted changes | Preserved. Never reclaimed automatically. |
| Commits not reachable from `main` | Preserved. Never reclaimed automatically. |
| Clean, and not ahead of `main` | Reclaimed for reuse. |

Reclamation is **rename/move in place**, re-pointing the branch, performed
automatically by the hook with no human in the loop. Moving in place preserves
the built `target/` directory and the rebuilt entity-store indexes; remove and
re-create would discard both and pay a full workspace rebuild (~2m26s measured)
plus a re-scan of ~1500 tickets.

A dirty worktree is never repurposed automatically. Repurposing one requires an
explicit agent decision, framed as re-topicing that worktree.

## Acceptance Criteria

1. A Rust binary replaces `worktree.sh` and covers its current subcommands:
   `new`, `list`, `rebase`, `merge`, `remove`, `doctor`.
2. Git is driven through a library, not by shelling out to the `git` binary.
3. Tests are written in Rust and cover the intent of the retiring shell suite,
   including the ten currently-failing contracts.
4. The `UserPromptSubmit` hook creates the session's worktree if absent, and is
   idempotent across repeated prompts within one session.
5. A worktree belonging to an active session is never purged or repurposed.
6. On session completion, a worktree with uncommitted changes is preserved.
7. On session completion, a worktree holding commits not reachable from `main`
   is preserved.
8. On session completion, a clean worktree not ahead of `main` is renamed/moved
   in place and its branch re-pointed, automatically.
9. Repurposing a dirty worktree requires an explicit agent decision and never
   happens automatically.
10. A recycled worktree is brought up to date with `main` before reassignment.
11. A newly created or recycled worktree has its machine-local entity-store
    indexes rebuilt before first use. A fresh worktree carries tracked
    `.ticket`, `.spec` and `.test` payloads but no `tickets.db` or
    `search_index/`, and the store refuses to open until those exist.
12. The retiring shell suite and its `common.sh` harness are removed in the
    same change that lands the Rust equivalent, not left behind red.

## Notes

Blocked on nothing, but sequenced after session-worktree discovery, which
supplies the resolution half of the same feature.

Separately tracked: `.worktrees/` currently holds nine directories while git
registers only three or four. Eager per-session creation will amplify this
orphan accumulation.



## 2026-08-07 Scope Update

The narrow capture-hook fix moved out of this ticket to ticket `a1b911ab-9394-4ba8-9134-1b2687e96ccd` and is already implemented there. This ticket now covers only eager worktree creation and the `worktree.sh`-to-Rust rewrite.



## Design decisions (2026-07-27, orchestrator session 70abae1b)

### Acceptance criterion 2 amended: hybrid git access

Criterion 2 originally read "Git is driven through a library, not by shelling out to the `git` binary." **That is not satisfiable.** A feasibility probe against the cached `git2` 0.20.4 / `libgit2-sys` 0.18.7 sources established that libgit2 cannot express the three write operations this feature depends on most:

| Operation | git2 | gix | Notes |
|---|---|---|---|
| worktree add, new branch from local `main` | partial | no | `WorktreeAddOptions::reference` needs an existing ref |
| **detached submodule worktree at an arbitrary gitlink SHA, offline** | **no** | **no** | No API accepts a detached target commit. This is the exact mechanism documented in `tools/worktree/worktree.sh` lines 121-134 as the one that must never be re-broken. |
| worktree remove (`--force`) | partial | no | No `git_worktree_remove` symbol in libgit2-sys 0.18.7; only `prune` |
| **worktree move / rename in place** | **no** | **no** | No `git_worktree_move` symbol at all. Required by both the rename-when-ticket-known naming strategy and by reclaim. |
| worktree list | yes | no | |
| branch create / delete / exists | yes | partial | |
| ff-only merge that fails rather than merging | partial | partial | composable from `merge_analysis` |
| rebase onto upstream | yes | no | |
| dirty-tree detection | yes | yes | |
| ahead/behind commit count | yes | yes | |
| read gitlink SHA from a tree | yes | yes | |

**Amended criterion 2:** git is accessed through `git2` for all read-side queries — worktree enumeration, branch existence, dirty-tree status, ahead/behind counts, and gitlink resolution from a tree — and through `std::process::Command` only for the write operations libgit2 cannot express: `worktree add` (superproject and detached submodule), `worktree remove`/`prune`, and `worktree move`. Every subprocess invocation must be justified by an entry in the table above; no read-side query may shell out.

Criteria 1, 3, and 12 are unaffected.

### Naming strategy

A worktree is created with a fixed session-derived slug (`<short-id>-session`) at provisioning time, when no ticket is yet known. When a ticket is later associated with the session, the worktree is **renamed in place** (`git worktree move` + branch re-point), never removed and recreated.

### Reclaim and cap

- No pre-provisioning. Each new session first attempts to **claim an eligible existing worktree**: not owned by an active session, clean working tree, and not ahead of `main`. Only if no candidate is eligible does the session create a new worktree and absorb the ~19s stall.
- Reclaim is a rename in place. Remove-and-recreate is rejected: it discards `target/` and the machine-local entity-store indexes, costing a ~2m26s rebuild plus a re-scan of roughly 1500 tickets.
- Total worktree count is capped. Past the cap, provisioning **fails loudly** rather than degrading silently, but a reclaim attempt runs first.

### Measured costs

| Operation | Cost |
|---|---|
| `worktree.sh new` (5 submodules) | 6.433 s, 193 MB |
| `ticket.exe init` | 5.075 s |
| `ticket.exe scan` | 7.220 s |
| **Full eager provision** | **≈ 19 s** |
| `worktree.sh remove` | 3.176 s |

A freshly created worktree carries the tracked `.ticket` and `.spec` payloads including `index.toon`, but has no `tickets.db`, `entities.db`, or `search_index/` until `init` and `scan` run. Criterion 11 covers rebuilding these before first use.

### Delivery order

Implementation lands in two slices on branch `agent/5e6cf4f8-worktree-rust-rewrite`:

1. **Slice 1 — eager creation.** New Rust crate, the git abstraction layer, the `new` subcommand, reclaim-eligibility and cap logic, index rebuild, and `UserPromptSubmit` hook wiring. Satisfies criteria 4, 5, 8, 9, 10, 11 and lets the next session start in its own worktree.
2. **Slice 2 — full rewrite.** The remaining subcommands (`list`, `rebase`, `merge`, `remove`, `doctor`), completion-time preservation semantics (criteria 6, 7), the Rust test suite covering all sixteen shell contracts, and retirement of the shell suite plus `common.sh`. Satisfies criteria 1, 3, 6, 7, 12.



## Slice 1 delivered and installed (2026-07-27)

Eager per-session worktree provisioning is merged to `main` and the release hook binary is installed. Superproject `main` = `34bac2df`, `memory-api` `main` = `c91aeb98`, gitlink consistent. All four merges were fast-forwards.

### What landed

New crate `memory-api/crates/session-worktree-provision` (registered in the root workspace):
- Git access layer — `git2` for every read (worktree enumeration, branch existence, dirty status, ahead/behind, gitlink resolution from a tree); `git` subprocess only for operations libgit2 cannot express.
- Offline submodule population — each submodule gets its own linked worktree at the recorded gitlink SHA, never `git submodule update`, which would repoint the shared `core.worktree` and empty the main checkout's submodule tree.
- Provisioning policy — reuse, else reclaim, else create, else fail loudly at the cap.
- `rebuild_entity_indexes` via direct library calls: `ticket_api::storage::TicketStore::init(...).scan(true)` and `spec_api::SpecStore::init(...).scan(true)`. No binary dependency, so a fresh worktree needs no pre-built executables.

Hook wiring in `memory-api/crates/session-capture-hook/src/main.rs`, on the `UserPromptSubmit` trigger — the only registered trigger that fires before any tool call. `PreToolUse` and `SessionStart` are not registered for this hook.

`.github/hooks/hooks.json` `UserPromptSubmit` timeout raised 120s → 300s.

### Three defects found by live verification, all fixed

Unit tests alone would not have caught any of these.

1. **`git worktree move` is unusable.** Git refuses outright: `fatal: working trees containing submodules cannot be moved or removed`. Every worktree here has 5 submodules, so both reclaim and rename were dead on arrival. Replaced with filesystem move followed by `git worktree repair` on the superproject and each nested submodule worktree, with rollback to the original path on any failure.
2. **Reclaim selected an actively-used worktree.** `.worktrees/5e6cf4f8-worktree-rust-rewrite` qualified while an agent was working in it, because the agent had never written a `.session` check-in record and the rule treated "unclaimed" as "abandoned". Reclaim now additionally requires the candidate to have been idle past a threshold (24h default, `WORKTREE_IDLE_SECS`), never reclaims the worktree containing the current directory, and fails closed when activity cannot be determined. Idle time is derived from cheap signals only — the worktree root, `.git/worktrees/<name>/`, and its `index` — never a recursive scan of ~5650 files.
3. **A failed reclaim aborted provisioning.** The session ended up with no worktree at all. A failed reclaim now logs to stderr, tries the next candidate, and falls through to creating a fresh worktree. `CapReached` still fails hard.

### A containment defect found while cleaning up

Both hook end-to-end suites build isolated `TempDir` fixtures and assert the real `.session` store is untouched, but `anchor_checkout()` falls back to `current_dir()` when `MCP_MAIN_CHECKOUT` is unset — so provisioning escaped the fixture and created a real `.worktrees/session--session` worktree plus eight `.session/sessions/session-workspace-fixture-*` directories in the developer's actual checkout. Fixed on both sides: the fixture harness now pins `MCP_MAIN_CHECKOUT` per spawned command (never `std::env::set_var`, which is unsafe under concurrent tests), and provisioning now runs only when the resolved session store lies inside the anchor checkout. Debris removed; leak verified closed by comparing the worktree list and fixture count before and after a full test run.

### Verification

53 tests: 24 in `session-worktree-provision`, 29 in `session-capture-hook`. `cargo fmt --check` clean. Clippy is blocked only by a pre-existing `unnecessary_sort_by` in `feedback-api`, newly pulled into the graph by the `ticket-api`/`spec-api` additions.

Live end-to-end against the real repository, release build, all assertions passing:

| Check | Result |
|---|---|
| Cold provision (create + 5 submodules + index rebuild) | 53.0 s |
| Warm / already-provisioned | 0.04 s |
| Hook stdout | exactly `{}` |
| Exit status | 0 |
| Submodules populated | 5/5 |
| Indexes rebuilt | `tickets.db`, `entities.db`, `search_index/` present in both stores |
| Kill switch `WORKTREE_EAGER_PROVISION=0` | honored, nothing created |
| Non-`UserPromptSubmit` trigger | nothing created |
| Live session recognized as already provisioned | yes, no duplicate short-id worktree |
| Pre-existing worktrees moved, renamed, or removed | none |

Cold cost is 53s against a 300s timeout — comfortable headroom, and the debug build's 80.4s was what motivated raising the timeout.

### Acceptance criteria status

Satisfied: 2 (as amended to the hybrid contract), 4, 5, 8, 9, 11.
Partially satisfied: 6 and 7 — a worktree holding uncommitted changes or unmerged commits is never reclaimed, but the completion-time preservation path is slice 2.
Not yet started: 1, 3, 10, 12 — the `worktree-ctl` binary, the `list`/`rebase`/`merge`/`remove`/`doctor` subcommands, bringing a reclaimed worktree up to date with `main` before reassignment, the Rust test suite covering all sixteen shell contracts, and retiring the shell suite plus `common.sh`.

### Note for slice 2

Criterion 10 has a `TODO` seam marked in the policy module. Criterion 3's specification is the existing shell suite under `tools/worktree/tests/`, of which 6 pass and 10 encode never-implemented lifecycle behavior.

Scope expanded 2026-08-08: this ticket now also absorbs the retirement of tools/worktree/worktree.sh, rescoped out of ticket 4ef88dbc. Branch agent/5e6cf4f8-worktree-rust-rewrite is currently zero commits ahead of main with no Rust source; the 16 shell tests under tools/worktree/tests/ (10 currently failing) are the behavioral specification for worktree-ctl: rename, finish, --dry-run, dirty acknowledgement, --preserve-main-changes, --allow-additional, session reuse, and remove-refuses-dirty.

Landed: worktree-ctl migrated from Bash to Rust; 28 tests pass and 16 shell contracts were retired.