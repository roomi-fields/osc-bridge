#!/usr/bin/env python3
"""Propagate the version from Cargo.toml (the source of truth) to every mirror.

Mirrors kept in sync:
  - npm/package.json              "version"
  - .claude-plugin/plugin.json    "version" + the npx pin in mcpServers.args
  - server.json                   "version" (top-level) + packages[].version

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


# Each mirror: (relative path, [compiled patterns]). Every pattern has 3
# groups — prefix / version / suffix — and EVERY match in the file is rewritten
# (a file may legitimately repeat the version, e.g. server.json's top-level
# field and packages[].version).
_VERSION_FIELD = re.compile(r'("version"\s*:\s*")([^"]+)(")')
_NPX_PIN = re.compile(r"(@roomi-fields/osc-bridge@)([^\"]+)(\")")

MIRRORS = [
    ("npm/package.json", [_VERSION_FIELD]),
    (".claude-plugin/plugin.json", [_VERSION_FIELD, _NPX_PIN]),
    ("server.json", [_VERSION_FIELD]),
]


def main() -> None:
    check = "--check" in sys.argv[1:]
    want = cargo_version()
    drift: list[str] = []

    for rel, patterns in MIRRORS:
        path = ROOT / rel
        if not path.is_file():
            # A mirror that doesn't exist yet (earlier phase) is not drift.
            continue
        txt = path.read_text(encoding="utf-8")
        new_txt = txt
        for pat in patterns:
            matches = list(pat.finditer(new_txt))
            if not matches:
                sys.exit(f"error: pattern {pat.pattern!r} not found in {rel}")
            for m in matches:
                if m.group(2) != want and check:
                    drift.append(f"  {rel}: {m.group(2)} != {want}")
            if not check:
                new_txt = pat.sub(rf"\g<1>{want}\g<3>", new_txt)
        if not check and new_txt != txt:
            path.write_text(new_txt, encoding="utf-8")
            print(f"  {rel}: synced to {want}")

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
