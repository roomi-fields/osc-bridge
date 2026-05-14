#!/usr/bin/env python3
"""Drive bulk-import sources declared in `sources.toml`.

Usage:
    python3 scripts/sync_sources.py [sources.toml] [--source NAME]

For each source it calls the right driver module (scripts.sources.<type>.fetch)
and caches the result under `.cache/<name>/`. The caching is incremental
when the source driver supports it.

Converters (turning cached data into device JSONs) are intentionally kept
separate — run them after `sync_sources.py` has populated the cache:
    python3 scripts/import_pencilresearch.py   --bulk .cache/pencilresearch-midi
    python3 scripts/import_electra_preset.py   --bulk .cache/electra-presets
"""
from __future__ import annotations
import argparse
import sys
import tomllib
from importlib import import_module
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


def load_config(path: Path) -> dict:
    with path.open("rb") as f:
        return tomllib.load(f)


def load_local_sources() -> list[dict]:
    """Optional local source-declaration overrides from `.local/sources.toml`.

    `.local/` is git-ignored — a place for machine-local config that isn't
    part of the project. Declaring a `[[source]]` block there lets you point
    the bulk-import tooling at a local-only source without editing the
    committed `sources.toml`. Returns an empty list when the file is absent —
    the normal case.
    """
    local = REPO_ROOT / ".local" / "sources.toml"
    if not local.is_file():
        return []
    with local.open("rb") as f:
        extra = tomllib.load(f).get("source", [])
    if extra:
        print(f"  (+{len(extra)} source(s) from .local/sources.toml)")
    return extra


def fetch_one(src: dict) -> dict:
    t = src.get("type", "")
    try:
        mod = import_module(f"scripts.sources.{t}")
    except ImportError as e:
        return {"fetched": 0, "skipped": 0, "errors": [f"no driver for type '{t}': {e}"], "cache_dir": None}
    return mod.fetch(src, REPO_ROOT)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("config", nargs="?", default="sources.toml")
    ap.add_argument("--source", help="restrict to a single source by name")
    args = ap.parse_args()

    cfg_path = (REPO_ROOT / args.config).resolve()
    if not cfg_path.is_file():
        print(f"error: config not found: {cfg_path}", file=sys.stderr); sys.exit(2)
    cfg = load_config(cfg_path)
    sources = cfg.get("source", []) + load_local_sources()
    if not sources:
        print("no [[source]] blocks declared", file=sys.stderr); sys.exit(2)

    total = {"fetched": 0, "skipped": 0, "errors": 0}
    for src in sources:
        if args.source and src["name"] != args.source:
            continue
        print(f"\n=== source: {src['name']} ({src['type']}) ===")
        report = fetch_one(src)
        total["fetched"] += report["fetched"]
        total["skipped"] += report["skipped"]
        total["errors"] += len(report.get("errors", []))
        for err in report.get("errors", [])[:5]:
            print(f"  ! {err}")
        if report.get("cache_dir"):
            print(f"  cache: {report['cache_dir'].relative_to(REPO_ROOT) if report['cache_dir'].is_relative_to(REPO_ROOT) else report['cache_dir']}")

    print(f"\n=== total: {total['fetched']} fetched, {total['skipped']} skipped, {total['errors']} errors ===")
    sys.exit(1 if total["errors"] > 0 else 0)


if __name__ == "__main__":
    # Make scripts.* importable when run directly
    sys.path.insert(0, str(REPO_ROOT))
    main()
