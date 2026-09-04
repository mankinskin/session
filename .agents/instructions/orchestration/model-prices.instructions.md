---
description: "Use when resolving model prices, syncing the model cost table, or wiring cost-aware routing to real numbers. Covers workflow-tools/session/crates/model-prices layout, the sync script, offline queries, the cost gate, and staleness policy."
applyTo: "**/*.json,**/*.toml"
---

## The Model Cost Table

`workflow-tools/session/crates/model-prices/` is the single source of truth for model pricing in this repository. Every routing, delegation, and cost-gate decision resolves against it. **Never hardcode a price** into instructions, agent templates, or tooling — read it from the table.

| Path | Role |
|---|---|
| `workflow-tools/session/crates/model-prices/model_prices.json` | The generated price table. Committed artifact; do not hand-edit. |
| `workflow-tools/session/crates/model-prices/sync_model_prices.py` | Fetches upstream prices and regenerates the table. Also serves offline queries. |
| `workflow-tools/session/crates/mcp-toolmon` | The cost gate: MCP middleware that resolves `caller_model` to an allow/delegate decision. Rust crate; there is no Python gate. |

Upstream sources are [pydantic/genai-prices](https://github.com/pydantic/genai-prices) (`prices/data_slim.json`, MIT) and GitHub Copilot's published pricing table (`https://raw.githubusercontent.com/github/docs/main/data/tables/copilot/models-and-pricing.yml` from the github/docs repo). The script is stdlib-only — no PyYAML dependency, no virtualenv needed.

### Source Precedence

The two upstreams occupy **disjoint provider namespaces**, so neither can overwrite the other:

- All genai-prices rows retain their original `provider_id` (e.g. `anthropic`, `openai`, `google`).
- All GitHub Copilot rows use `provider_id: "github-copilot"` and `provider_name: "GitHub Copilot"`, with `model_id` set to the model's display name **verbatim** as it appears in the `runSubagent` model surface (e.g. `MAI-Code-1-Flash`, `Claude Opus 5`).

Consequence: the same underlying model can legitimately appear **twice** in the table — once under its vendor's genai-prices slug and once under `github-copilot`. For routing and cost-gate decisions about a Copilot dispatch, use the `github-copilot` row.

Within the GitHub Copilot source, models may appear multiple times with different pricing tiers/thresholds. The sync keeps only the `Default` tier row (or the row with no threshold specified), ensuring exactly one row per model_id.

### Table shape

```jsonc
{
  "_meta": {
    "source": "pydantic/genai-prices",
    "source_url": "...data_slim.json",
    "source_sha256": "...",       // composite digest over both upstreams; drives change detection
    "synced_at": "...",           // UTC ISO timestamp of last successful sync
    "model_count": 1285,
    "price_unit": "USD per 1,000,000 tokens",
    "sources": [                  // per-source detail; source/source_url retained for compatibility
      {"name": "pydantic/genai-prices", "url": "...", "sha256": "...", "model_count": 1200},
      {"name": "github-copilot", "url": "...", "sha256": "...", "model_count": 85}
    ]
  },
  "models": [
    {
      "provider_id": "anthropic",
      "provider_name": "Anthropic",
      "model_id": "claude-sonnet-5",
      "input_mtok": 2,
      "output_mtok": 10,
      "cache_read_mtok": 0.2,
      "cache_write_mtok": 2.5,
      "context_window": 1000000,
      "deprecated": false
    }
  ]
}
```

- All prices are **USD per 1M tokens**. A `null` (rendered `-` in table/CSV output) means upstream did not publish that field, not that it is free.
- Rows are sorted by `(provider_id, model_id)`, so a regenerated table diffs cleanly.
- The same model often appears under several `provider_id`s (`anthropic`, `aws`, `google`, …) at different prices. Match the provider you actually bill through; when unsure, use the model vendor's own row.
- Prices are **indicative estimates**, not authoritative billing data. Use them for relative routing decisions, not for invoicing.

## Reading Prices (Offline)

`--query` and `--list` never hit the network — they read the committed table. Use them freely.

```bash
cd workflow-tools/session/crates/model-prices

# One model or family, human-readable
python sync_model_prices.py --query claude-sonnet-5

# Compare a family across providers
python sync_model_prices.py --query gpt-5.6 --format csv

# Whole table, machine-readable
python sync_model_prices.py --list --format json
```

Practical patterns:

```bash
# Cheapest models under $6/M output, sorted
python sync_model_prices.py --list --format csv \
  | awk -F, '$4 != "-" && $4+0 <= 6 && $4+0 > 0' | sort -t, -k4 -n

# Rank cheap-tier candidates by INPUT price (the metric that matters for bulk units)
python sync_model_prices.py --list --format csv \
  | grep -E '^(anthropic|openai|google),' | sort -t, -k3 -n | head -20
```

CSV columns are `provider,model,in$/M,out$/M,cread$/M,cwrite$/M,ctx` — note that `--format csv` uses these short headers while the JSON uses the full `*_mtok` field names.

## Syncing (Network)

```bash
cd workflow-tools/session/crates/model-prices

# Is the local table stale? Exit 1 = out of date. Writes nothing.
python sync_model_prices.py --check

# Sync. No-ops when the upstream hash is unchanged.
python sync_model_prices.py

# Force a rewrite even when unchanged (refreshes synced_at)
python sync_model_prices.py --force

# Richer upstream dataset, or a pinned/mirrored source
python sync_model_prices.py --full
python sync_model_prices.py --source-url <url> --timeout 60
```

Sync rules:

- Run `--check` before trusting the table for a cost-sensitive decision, and sync when it reports stale.
- The script rewrites `model_prices.json` **only** when `source_sha256` changes, so a routine sync is a no-op and produces no diff.
- Treat `model_prices.json` as generated: commit the regenerated file, never hand-edit it. Fix pricing problems upstream or in the script.
- Sync is a network operation. If it fails (offline, upstream down), fall back to the committed table, note that prices may be stale, and continue — do not block work on a failed sync.
- A sync can **change tier assignments**. After a sync that produces a diff, re-check the canonical ladder in [model-routing.instructions.md](model-routing.instructions.md) against the new numbers. [orchestrator-delegation.instructions.md](orchestrator-delegation.instructions.md) references that ladder rather than duplicating it — keep it that way.
- The table is a **vendor catalogue**, not the roster of models the current surface offers. Many priced models will be refused by `runSubagent`. Confirm availability before routing to a model you found here; see "Roster is not the catalogue" in [model-routing.instructions.md](model-routing.instructions.md).

## The Cost Gate

The gate is a Rust MCP middleware, [workflow-tools/session/crates/mcp-toolmon](../../../workflow-tools/session/crates/mcp-toolmon). There is **no** `cost_gate.py`; earlier revisions of this file documented one that never shipped.

```bash
mcp-toolmon -- <real-server-command> [server args...]
```

- It fronts a real MCP stdio server: on `tools/list` it injects a required `caller_model` argument into every advertised tool schema; on `tools/call` it reads `arguments.caller_model`, decides allow/delegate, and strips the field before forwarding.
- The decision is a **graded budget**, not a ban: `base_budget = round((1 − output_mtok / budget_zero_price) × scale_max)`, clamped to the scale, where `budget_zero_price` defaults to 60.0 and `scale_max` to 100. Tool cost comes from the empirical rollup; the call is allowed when `tool_cost <= base_budget + grant_offset`.
- **No model is ever denied outright.** A pricier model keeps full access to cheap tools and is asked to delegate only the token-heavy ones. An unmeasured tool resolves to cost 0 and is always allowed, and a grant offset (`COST_GATE_GRANTS_DIR`, optionally model-scoped and expiring) raises the ceiling.
- Illustrative budgets at the default calibration: Claude Opus 5 (`out 25`) → 58, GPT-5.6 Terra and Claude Sonnet 4.5 (`out 15`) → 75, Claude Sonnet 5 (`out 10`) → 83, GPT-5.6 Luna (`out 6`) → 90, GPT-5 mini (`out 2`) → 97. Note that the gate reads `output_mtok` **only**, so two models at the same output price receive identical budgets regardless of how they compare on input, cache read, or context window — that comparison is routing judgement, not gate arithmetic.
- `caller_model` must be a real `model_id` key from the table (e.g. `claude-sonnet-5`, `gpt-5.3-codex`), not a vendor or product label like `copilot` or `anthropic`. An unrecognized id is **rejected**, so price awareness is never silently bypassed. A delegated sub-agent passes **its own** id, not the orchestrator's.
- Configure via `COST_GATE_TABLE` (required for enforcement), `COST_GATE_TOOL_METRICS`, `COST_GATE_GRANTS_DIR`, `COST_GATE_SCALE_MAX`, `COST_GATE_BUDGET_ZERO_PRICE`.
- The gate **fails open** when the price table is missing or unreadable — it becomes a transparent passthrough. A silently permissive gate looks identical to a correctly permissive one; verify `COST_GATE_TABLE` resolves before concluding that routing is unrestricted.
- **The gate does not see `runSubagent`.** It only intercepts MCP `tools/call` traffic, so it governs *which tools a model may call*, not *which model receives a delegated unit*. Dispatch-target selection is routing judgement — see the tier ladder in [model-routing.instructions.md](model-routing.instructions.md).
- After changing gate logic, run `cargo test -p mcp-toolmon` before relying on it.

## When Prices Move

Model pricing changes often, and a stale table quietly degrades every routing decision. When a sync produces a real diff:

1. Re-resolve the models named in the tier tables; confirm each still sits in its assigned band.
2. Confirm each still exists on the current surface — a price change is a good moment to re-verify the roster, since vendors retire models and the catalogue keeps pricing them.
3. Check whether a newer generation now dominates the current default on **every** axis (input, output, cache read, context window). If so, promote it and mark the old default as dominated rather than deleting it — existing sessions may still reference it, and a dominated model left unmarked is exactly how a stale name gets dispatched from habit.
4. Check whether the `X = 15` gate threshold still splits the catalogue sensibly. Remember it governs **whether to orchestrate**, not which model to dispatch to — dispatch targets come from the tier ladder.
5. Update the tier tables and record the price basis, so a later reader can tell whether a routing rule was reasoned or inherited.
