"""Source drivers for sync_sources.py.

Each module exposes a `fetch(source_cfg: dict, repo_root: Path) -> dict` function
that returns a report like:
    {"fetched": int, "skipped": int, "errors": list[str], "cache_dir": Path}
"""
