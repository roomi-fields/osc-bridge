#!/usr/bin/env python3
"""Write the catalogue's size into every sentence that states it.

The driver files are the source (scripts/regen_supported_devices.py counts
them: devices, vendors, driver files). Each file below states those numbers
in prose — README, the npm page, the registry and plugin manifests, the MCP
guide; this rewrites the number inside each phrase and leaves the phrase
alone. Directories copy these descriptions verbatim, so a stale number
travels far: CI runs this with --check and fails on any drift, including the
GitHub repository's description (--description). Run without --check, it
also rewrites that description itself, through the GitHub CLI.

Usage:
    python3 scripts/sync_counts.py                 # rewrite the files and the repository's description
    python3 scripts/sync_counts.py --check [--description "<repo description>"]
"""
from __future__ import annotations
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from regen_supported_devices import REPO_ROOT, counts, load_devices  # noqa: E402

FILES = ["README.md", "npm/README.md", "npm/package.json", "server.json", ".claude-plugin/plugin.json", "src/mcp.rs",
         "mcpb/manifest.json", "docs/MCP.md", "docs/MCP_DIRECTORIES.md"]

# A number, then (maybe across bold markers and a line break) what it counts.
PATTERNS = {
    "devices": re.compile(r"\b\d{3,5}(?=(?:\*\*)?(?:\s+|-)(?:hardware |music )?(?:synthesizers|synths|devices?)\b)"),
    "vendors": re.compile(r"\b\d{2,4}(?=(?:\*\*)?\s+vendors\b)"),
    "drivers": re.compile(r"\b\d{3,5}(?=\s+driver files\b)"),
}


def rewrite(text: str, n: dict[str, int]) -> str:
    for key, pattern in PATTERNS.items():
        text = pattern.sub(str(n[key]), text)
    return text


def sync_description(n: dict[str, int]) -> list[str]:
    """Rewrite the GitHub repository's description with the GitHub CLI."""
    try:
        description = subprocess.run(["gh", "repo", "view", "--json", "description", "-q", ".description"],
                                     cwd=REPO_ROOT, capture_output=True, text=True, check=True).stdout.strip()
        new = rewrite(description, n)
        if new == description:
            return []
        subprocess.run(["gh", "repo", "edit", "--description", new], cwd=REPO_ROOT, capture_output=True, check=True)
        return ["the GitHub description"]
    except (OSError, subprocess.CalledProcessError) as e:
        print(f"the GitHub description was not checked ({e}): update it with gh repo edit --description", file=sys.stderr)
        return []


def main() -> int:
    check = "--check" in sys.argv
    n = counts(load_devices())
    stale = []
    for name in FILES:
        path = REPO_ROOT / name
        text = path.read_text(encoding="utf-8")
        new = rewrite(text, n)
        if new != text:
            stale.append(name)
            if not check:
                path.write_text(new, encoding="utf-8")
    if not check:
        stale += sync_description(n)
    if "--description" in sys.argv:
        description = sys.argv[sys.argv.index("--description") + 1]
        if rewrite(description, n) != description:
            stale.append(f"the GitHub description (gh repo edit --description, with {n['devices']} devices)")
    if check and stale:
        print(f"catalogue counts out of date ({n['devices']} devices, {n['vendors']} vendors, {n['drivers']} driver files): "
              + ", ".join(stale) + " — run scripts/sync_counts.py", file=sys.stderr)
        return 1
    print(f"{n['devices']} devices, {n['vendors']} vendors, {n['drivers']} driver files"
          + (f"; rewrote {', '.join(stale)}" if stale and not check else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
