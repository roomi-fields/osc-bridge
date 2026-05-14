#!/usr/bin/env python3
"""Propagate the version from Cargo.toml (the source of truth) to every mirror.

Mirrors kept in sync:
  - npm/package.json   →  "version"

Future phases will add `.claude-plugin/plugin.json` (version + the npx pin in
mcpServers.args) and `server.json` here.

Each mirror is patched with a targeted regex replace, never parse-then-dump —
re-serialising a JSON file reflows it and produces phantom diffs.

Usage:
    python3 scripts/sync_version.py            # write the mirrors
    python3 scripts/sync_version.py --check    # CI gate: exit 1 on any drift
"""
from __future__ import annotations
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def cargo_version() -> str:
    txt = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    # The [package] version is the first line-anchored `version = "..."`.
    # Dependency versions are mid-line (`crate = { version = "..." }`), so the
    # anchor keeps us on the package version.
    m = re.search(r'(?m)^version\s*=\s*"([^"]+)"', txt)
    if not m:
        sys.exit("error: no [package] version in Cargo.toml")
    return m.group(1)


# (path, compiled regex with 3 groups: prefix / version / suffix, label)
MIRRORS = [
    (
        ROOT / "npm" / "package.json",
        re.compile(r'("version"\s*:\s*")([^"]+)(")'),
        "npm/package.json",
    ),
]


def main() -> None:
    check = "--check" in sys.argv[1:]
    want = cargo_version()
    drift: list[str] = []

    for path, pat, label in MIRRORS:
        if not path.is_file():
            # A mirror that doesn't exist yet (earlier phase) is not drift.
            continue
        txt = path.read_text(encoding="utf-8")
        m = pat.search(txt)
        if not m:
            sys.exit(f"error: version field not found in {label}")
        have = m.group(2)
        if have == want:
            continue
        if check:
            drift.append(f"  {label}: {have} != {want}")
        else:
            path.write_text(
                pat.sub(rf"\g<1>{want}\g<3>", txt, count=1), encoding="utf-8"
            )
            print(f"  {label}: {have} -> {want}")

    if check and drift:
        print(f"version drift (Cargo.toml = {want}):", file=sys.stderr)
        print("\n".join(drift), file=sys.stderr)
        print(
            "::error::version mirrors out of sync — run "
            "scripts/sync_version.py and commit.",
            file=sys.stderr,
        )
        sys.exit(1)

    print(f"version: {want} — all mirrors in sync")


if __name__ == "__main__":
    main()
