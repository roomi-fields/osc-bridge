"""Git-repo source: clone or pull into a local cache directory."""
from __future__ import annotations
import subprocess
from pathlib import Path


def fetch(cfg: dict, repo_root: Path) -> dict:
    cache = (repo_root / cfg["cache_dir"]).resolve()
    repo = cfg["repo"]
    if not cache.exists():
        cache.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "clone", "--depth", "1", repo, str(cache)], check=True)
        return {"fetched": 1, "skipped": 0, "errors": [], "cache_dir": cache}
    # Existing clone: pull
    try:
        subprocess.run(["git", "-C", str(cache), "pull", "--ff-only"], check=True)
        return {"fetched": 1, "skipped": 0, "errors": [], "cache_dir": cache}
    except subprocess.CalledProcessError as e:
        return {"fetched": 0, "skipped": 0, "errors": [str(e)], "cache_dir": cache}
