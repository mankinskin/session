# Goal

Validate pinned entity URNs at init/resume and surface or prune unresolved pins instead of silently failing instruction rendering and carrying dead pins forward.

# Problem (evidence)

In session `beca8ec5`, `render_instructions` failed at resume because pinned rule `84fa9769-...` no longer resolves. The dead pin was preserved through the entire run and re-emitted unchanged in handoff `816f0807`; both new specs even hard-coded that unresolved rule id as their governing rule. Spec `8c880efc` contract covers "semantic search never silently auto-pins" but nothing about validating existing pins.

# Code touchpoints (verified)

- `render_pinned_rule_instructions` (`memory-api/crates/session-api/src/store.rs:926`) iterates pinned `Rule` entities and calls `rule_store.get(&parsed.entity_id)`; the first unresolved pin returns `SessionError::InvalidHookInput`, aborting the whole render.
- `view_runtime_context` (`store.rs:901`) maps pins straight to `SessionPinnedEntityHeader` with no resolution attempt; `SessionRuntimeView` (`model.rs:120`) has no unresolved-pin channel.
- `init_runtime_context` (`store.rs:608`) restores pins from the predecessor context verbatim.
- Audit sink already exists (`memory-api/crates/session-api/src/audit.rs`) for auditable mutations.

# Solution Design

1. Add a private helper `resolve_pins(&self, context) -> Vec<PinResolution>` where `PinResolution { header: SessionPinnedEntityHeader, resolved: bool, error: Option<String> }`. Resolve per `SessionPinnedEntityKind` against its owning store (`RuleStore` / `TicketStore` / `SpecStore`) via `sibling_store_root`.
2. `render_pinned_rule_instructions`: collect resolvable rules, and for each unresolved pin push a diagnostic instead of early-returning. Change the return to a struct `{ render: String, unresolved: Vec<PinResolution> }` (or keep the string and add a sibling method) and thread `unresolved` through session-mcp `render_instructions` and session-cli output.
3. Extend `SessionRuntimeView` with `#[serde(default, skip_serializing_if = "Vec::is_empty")] unresolved_pins: Vec<...>` populated by `view_runtime_context` so `runtime_view` shows the classification without erroring.
4. Add an explicit prune/repair path `runtime_prune_pins(strategy: Flag | Drop)` that writes an `audit.rs` entry for every dropped/flagged pin; never drop silently and never on the read path.

# Non-goals

- Auto-repinning a replacement rule.
- Changing pin creation semantics.

# Acceptance Criteria

1. Resume with one unresolved pin returns a partial render of the resolvable pins plus an explicit unresolved-pin list — not a hard `Err`.
2. `runtime_view` reports each pin's resolved/unresolved classification.
3. The prune/flag path emits an audit entry per affected pin and only mutates on explicit request.
4. Focused test covers a dead-URN rule pin through both `render_instructions` and `runtime_view`.

# Traceability

- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913` (contract: entity references / pinning).
- Epic: `effba966-f0a8-4d7d-b289-b7feba826cf8`.