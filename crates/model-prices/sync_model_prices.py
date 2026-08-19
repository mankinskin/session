#!/usr/bin/env python3
"""Extract and synchronize an LLM model price table from multiple sources.

Sources:
1. pydantic/genai-prices (MIT) — https://github.com/pydantic/genai-prices
2. GitHub Copilot pricing — https://github.com/github/docs (models-and-pricing.yml)

The two sources occupy disjoint provider namespaces (genai-prices provider ids
vs. ``github-copilot``), so rows never collide. Final table is sorted by
(provider_id, model_id).

Change detection uses a composite sha256 over both sources' content hashes,
so the output is rewritten when either source changes.

Stdlib only. No third-party dependencies.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

RAW_BASE = "https://raw.githubusercontent.com/pydantic/genai-prices/main/prices"
SLIM_URL = f"{RAW_BASE}/data_slim.json"
FULL_URL = f"{RAW_BASE}/data.json"
GITHUB_COPILOT_URL = "https://raw.githubusercontent.com/github/docs/main/data/tables/copilot/models-and-pricing.yml"

# Per-million-token price fields we surface in the flattened table.
PRICE_FIELDS = (
    "input_mtok",
    "output_mtok",
    "cache_read_mtok",
    "cache_write_mtok",
)


def fetch(url: str, timeout: float) -> bytes:
    """Download ``url`` and return the raw bytes."""
    request = urllib.request.Request(
        url, headers={"User-Agent": "context-engine-model-price-sync/1.0"}
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.read()


def parse_mini_yaml(raw: bytes) -> list[dict[str, Any]]:
    """Parse a minimal YAML subset: a sequence of mappings.
    
    Expected shape:
    - key: value
      key2: value2
    - key: value
    
    Strips surrounding quotes, skips blank lines and # comments, strips inline
    trailing comments from unquoted values. Raises ValueError on malformed input.
    """
    lines = raw.decode("utf-8").splitlines()
    entries: list[dict[str, Any]] = []
    current: dict[str, Any] | None = None
    
    for line_num, line in enumerate(lines, start=1):
        stripped = line.lstrip()
        if not stripped or stripped.startswith("#"):
            continue
        
        # New entry starts with "- key: value"
        if stripped.startswith("- "):
            if current is not None:
                entries.append(current)
            current = {}
            rest = stripped[2:]
            if ":" not in rest:
                raise ValueError(f"Line {line_num}: expected 'key: value', got '{line}'")
            key, _, val = rest.partition(":")
            current[key.strip()] = _parse_yaml_value(val.strip())
        elif ":" in stripped and current is not None:
            # Continuation: "  key: value"
            key, _, val = stripped.partition(":")
            current[key.strip()] = _parse_yaml_value(val.strip())
        else:
            raise ValueError(f"Line {line_num}: unexpected format '{line}'")
    
    if current is not None:
        entries.append(current)
    return entries


def _parse_yaml_value(val: str) -> str:
    """Strip quotes and inline comments from a YAML scalar value."""
    if not val:
        return ""
    # Strip surrounding quotes
    if (val.startswith('"') and val.endswith('"')) or (val.startswith("'") and val.endswith("'")):
        return val[1:-1]
    # Strip inline trailing comment if no quotes
    if "#" in val:
        val = val.split("#", 1)[0].rstrip()
    return val


def scalar_price(value: Any) -> float | None:
    """Reduce a price field to a single USD/mtok number.

    Accepts a plain number or a tiered ``{base, tiers}`` object (base rate is
    used). Returns ``None`` when the field is absent or not understood.
    """
    if value is None:
        return None
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, dict) and "base" in value:
        base = value["base"]
        if isinstance(base, (int, float)):
            return float(base)
    return None


def resolve_prices(prices: Any) -> dict[str, Any]:
    """Resolve a model's ``prices`` (ModelPrice or conditional list) to one map.

    For a conditional list, prefer the last entry with no constraint (the
    always-valid price); otherwise fall back to the first entry.
    """
    if isinstance(prices, dict):
        return prices
    if isinstance(prices, list) and prices:
        chosen = prices[0]
        for entry in prices:
            if isinstance(entry, dict) and entry.get("constraint") is None:
                chosen = entry
        if isinstance(chosen, dict):
            return chosen.get("prices", {}) or {}
    return {}


def flatten(providers: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Flatten providers/models into a list of price rows sorted by id."""
    rows: list[dict[str, Any]] = []
    for provider in providers:
        provider_id = provider.get("id", "")
        provider_name = provider.get("name", "")
        for model in provider.get("models", []):
            price_map = resolve_prices(model.get("prices"))
            row: dict[str, Any] = {
                "provider_id": provider_id,
                "provider_name": provider_name,
                "model_id": model.get("id", ""),
                "context_window": model.get("context_window"),
                "deprecated": bool(model.get("deprecated", False)),
            }
            for field in PRICE_FIELDS:
                row[field] = scalar_price(price_map.get(field))
            rows.append(row)
    rows.sort(key=lambda r: (r["provider_id"], r["model_id"]))
    return rows


def parse_price_string(s: str) -> float | None:
    """Parse a price string like '$1,234.50' to float. Returns None for non-numeric."""
    if not s or s.strip().lower() in ("not applicable", "n/a", "-"):
        return None
    cleaned = s.strip().replace("$", "").replace(",", "")
    try:
        return float(cleaned)
    except ValueError:
        return None


def flatten_github_copilot(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Flatten GitHub Copilot YAML entries into price rows.
    
    Deduplicates models by keeping the 'Default' tier or first occurrence.
    Strips markdown footnote markers from model names.
    """
    # Group by model name to handle duplicates
    model_groups: dict[str, list[dict[str, Any]]] = {}
    for entry in entries:
        model_name = entry.get("model", "")
        # Strip markdown footnote markers like [^sonnet-5-promo]
        import re
        model_name = re.sub(r"\[\^[^\]]+\]$", "", model_name).strip()
        if not model_name:
            continue
        if model_name not in model_groups:
            model_groups[model_name] = []
        model_groups[model_name].append(entry)
    
    rows: list[dict[str, Any]] = []
    for model_name, group in model_groups.items():
        # Pick one entry: prefer tier='Default', else threshold absent/not applicable, else first
        chosen = group[0]
        for entry in group:
            tier = entry.get("tier", "")
            threshold = entry.get("threshold", "")
            if tier == "Default":
                chosen = entry
                break
            if not threshold or threshold.lower() in ("not applicable", "n/a"):
                chosen = entry
        
        row: dict[str, Any] = {
            "provider_id": "github-copilot",
            "provider_name": "GitHub Copilot",
            "model_id": model_name,
            "context_window": None,
            "deprecated": False,
            "input_mtok": parse_price_string(chosen.get("input", "")),
            "output_mtok": parse_price_string(chosen.get("output", "")),
            "cache_read_mtok": parse_price_string(chosen.get("cached_input", "")),
            "cache_write_mtok": parse_price_string(chosen.get("cache_write", "")),
        }
        rows.append(row)
    
    rows.sort(key=lambda r: r["model_id"])
    return rows


def build_document(
    genai_url: str,
    genai_sha: str,
    genai_count: int,
    github_sha: str,
    github_count: int,
    rows: list[dict[str, Any]],
) -> dict[str, Any]:
    """Build the final document with composite metadata from both sources.
    
    The composite source_sha256 is sha256(genai_sha + github_sha) for change detection.
    sources array carries per-source details.
    """
    composite_sha = hashlib.sha256((genai_sha + github_sha).encode("utf-8")).hexdigest()
    return {
        "_meta": {
            "source": "pydantic/genai-prices",
            "source_url": genai_url,
            "source_sha256": composite_sha,
            "synced_at": _dt.datetime.now(_dt.timezone.utc).isoformat(),
            "model_count": len(rows),
            "price_unit": "USD per 1,000,000 tokens",
            "note": "Multi-source aggregate: genai-prices + GitHub Copilot. Disjoint provider namespaces.",
            "sources": [
                {
                    "name": "pydantic/genai-prices",
                    "url": genai_url,
                    "sha256": genai_sha,
                    "model_count": genai_count,
                },
                {
                    "name": "github-copilot",
                    "url": GITHUB_COPILOT_URL,
                    "sha256": github_sha,
                    "model_count": github_count,
                },
            ],
        },
        "models": rows,
    }


def load_existing_sha(output_path: Path) -> str | None:
    if not output_path.exists():
        return None
    try:
        existing = json.loads(output_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return None
    if isinstance(existing, dict):
        return existing.get("_meta", {}).get("source_sha256")
    return None


def _fmt(value: Any) -> str:
    if value is None:
        return "-"
    if isinstance(value, float):
        return f"{value:g}"
    return str(value)


def query_table(output_path: Path, needle: str | None, fmt: str) -> int:
    """Print matching rows from the local price table without any network I/O.

    ``needle`` is a case-insensitive substring matched against ``provider_id``
    and ``model_id``; ``None`` lists everything.
    """
    if not output_path.exists():
        print(
            f"error: {output_path} not found; run a sync first (see --help).",
            file=sys.stderr,
        )
        return 2
    try:
        document = json.loads(output_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as exc:
        print(f"error: cannot read {output_path}: {exc}", file=sys.stderr)
        return 2

    rows = document.get("models", []) if isinstance(document, dict) else []
    if needle:
        low = needle.lower()
        rows = [
            r
            for r in rows
            if low in r.get("provider_id", "").lower()
            or low in r.get("model_id", "").lower()
        ]

    if not rows:
        print("no matching models" if needle else "no models in table", file=sys.stderr)
        return 1

    if fmt == "json":
        print(json.dumps(rows, indent=2, ensure_ascii=False))
        return 0

    columns = [
        ("provider_id", "provider"),
        ("model_id", "model"),
        ("input_mtok", "in$/M"),
        ("output_mtok", "out$/M"),
        ("cache_read_mtok", "cread$/M"),
        ("cache_write_mtok", "cwrite$/M"),
        ("context_window", "ctx"),
    ]
    if fmt == "csv":
        print(",".join(header for _, header in columns))
        for row in rows:
            print(",".join(_fmt(row.get(key)) for key, _ in columns))
        return 0

    # Aligned text table (default).
    cells = [[header for _, header in columns]]
    cells += [[_fmt(row.get(key)) for key, _ in columns] for row in rows]
    widths = [max(len(r[i]) for r in cells) for i in range(len(columns))]
    for i, row in enumerate(cells):
        print("  ".join(cell.ljust(widths[j]) for j, cell in enumerate(row)))
        if i == 0:
            print("  ".join("-" * widths[j] for j in range(len(columns))))
    print(f"\n{len(rows)} model(s)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).with_name("model_prices.json"),
        help="Path to the local price table JSON (default: model_prices.json next to this script).",
    )
    parser.add_argument(
        "--full",
        action="store_true",
        help="Use the full data.json instead of the slimmed data_slim.json.",
    )
    parser.add_argument(
        "--source-url",
        default=None,
        help="Override the remote source URL entirely.",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Rewrite the output even when the upstream content is unchanged.",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=30.0,
        help="HTTP timeout in seconds (default: 30).",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Exit 1 if the local table is out of date; do not write anything.",
    )
    parser.add_argument(
        "--query",
        metavar="TEXT",
        default=None,
        help="Offline: print rows whose provider or model id contains TEXT (no sync).",
    )
    parser.add_argument(
        "--list",
        dest="list_all",
        action="store_true",
        help="Offline: print every model in the local table (no sync).",
    )
    parser.add_argument(
        "--format",
        choices=("table", "csv", "json"),
        default="table",
        help="Output format for --query/--list (default: table).",
    )
    args = parser.parse_args(argv)

    if args.query is not None or args.list_all:
        return query_table(args.output, args.query, args.format)

    genai_url = args.source_url or (FULL_URL if args.full else SLIM_URL)

    # Fetch genai-prices
    try:
        genai_raw = fetch(genai_url, timeout=args.timeout)
    except (urllib.error.URLError, TimeoutError) as exc:
        print(f"error: failed to fetch {genai_url}: {exc}", file=sys.stderr)
        return 2
    genai_sha = hashlib.sha256(genai_raw).hexdigest()

    # Fetch GitHub Copilot pricing
    try:
        github_raw = fetch(GITHUB_COPILOT_URL, timeout=args.timeout)
    except (urllib.error.URLError, TimeoutError) as exc:
        print(f"error: failed to fetch {GITHUB_COPILOT_URL}: {exc}", file=sys.stderr)
        return 2
    github_sha = hashlib.sha256(github_raw).hexdigest()

    # Composite hash for change detection
    composite_sha = hashlib.sha256((genai_sha + github_sha).encode("utf-8")).hexdigest()
    local_sha = load_existing_sha(args.output)
    up_to_date = local_sha == composite_sha

    if args.check:
        if up_to_date:
            print(f"up to date ({args.output.name}, composite_sha256={composite_sha[:12]})")
            return 0
        print(f"out of date: {args.output.name} (local={local_sha}, remote={composite_sha[:12]})")
        return 1

    if up_to_date and not args.force:
        print(f"up to date, no changes written ({args.output.name}, composite_sha256={composite_sha[:12]})")
        return 0

    # Parse genai-prices
    try:
        providers = json.loads(genai_raw)
    except json.JSONDecodeError as exc:
        print(f"error: genai-prices JSON is invalid: {exc}", file=sys.stderr)
        return 2
    if not isinstance(providers, list):
        print("error: genai-prices JSON root is not an array", file=sys.stderr)
        return 2
    genai_rows = flatten(providers)

    # Parse GitHub Copilot pricing
    try:
        github_entries = parse_mini_yaml(github_raw)
    except ValueError as exc:
        print(f"error: GitHub Copilot YAML parse failed: {exc}", file=sys.stderr)
        return 2
    github_rows = flatten_github_copilot(github_entries)

    # Combine rows (disjoint provider namespaces, so no collision)
    all_rows = genai_rows + github_rows
    all_rows.sort(key=lambda r: (r["provider_id"], r["model_id"]))

    document = build_document(
        genai_url, genai_sha, len(genai_rows), github_sha, len(github_rows), all_rows
    )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    action = "updated" if local_sha else "created"
    print(
        f"{action} {args.output} ({len(genai_rows)} genai + {len(github_rows)} github-copilot = {len(all_rows)} total, composite_sha256={composite_sha[:12]})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
