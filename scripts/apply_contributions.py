#!/usr/bin/env python3
"""Merge the driver files agents contributed through the hosted server.

Run by `.github/workflows/contributions.yml`. For each pending contribution
(from `GET /admin/contributions` on the hosted server), one at a time on a
fresh `main`:

1. the path must be a new `devices/<vendor>/<name>.agent-contributed.fw-<v>.json`
   file, or one an earlier agent contribution created — never another file;
2. the file is written, then must parse and lint (`osc-bridge lint`);
3. the catalogue listings are regenerated (`regen_supported_devices.py`,
   `build_device_index.py`) and the library tests must pass;
4. the change may touch nothing but that file and the two listings;
5. it goes on a branch, as a pull request, merged at once (no review: trust
   comes from usage reports afterwards).

The verdict — merged with the pull request, or rejected with the reason — is
reported back to the server (`POST /admin/contributions/<id>`).

Usage:
    apply_contributions.py PENDING_JSON --binary target/release/osc-bridge
Environment: OSC_BRIDGE_SYNC_TOKEN, MCP_BASE (default the hosted server),
GH_TOKEN for `gh`.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LISTINGS = {"docs/SUPPORTED_DEVICES.md", "docs/devices.json"}
PATH_RE = re.compile(r"^devices/[a-z0-9][a-z0-9-]*/[a-z0-9_]+\.agent-contributed\.fw-[a-z0-9_]+\.json$")
AUTHOR = "roomi-fields <roomi.git@liance.art>"


def sh(*cmd: str, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, cwd=ROOT, check=check, capture_output=True, text=True)


def settle(base: str, token: str, cid: int, state: str, detail: str | None = None, pr: str | None = None) -> None:
    body = json.dumps({"state": state, "detail": detail, "pr": pr}).encode()
    req = urllib.request.Request(f"{base}/admin/contributions/{cid}", data=body, method="POST",
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    urllib.request.urlopen(req, timeout=30).read()


def reset() -> None:
    sh("git", "checkout", "--force", "main")
    sh("git", "reset", "--hard", "origin/main")
    sh("git", "clean", "-fd", "devices", "docs")


def check(c: dict, binary: str) -> str | None:
    """Apply one contribution to the work tree. None if it passes, else why not."""
    path = c["path"]
    if not PATH_RE.match(path) or ".." in path:
        return f"path {path!r} is not an agent-contributed driver file"
    target = ROOT / path
    if target.exists():
        src = (json.loads(target.read_text()).get("_sources") or [{}])[0]
        if src.get("type") != "agent-contributed":
            return f"{path} exists and was not contributed by an agent"
    try:
        json.loads(c["content"])
    except json.JSONDecodeError as e:
        return f"not JSON: {e}"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(c["content"])
    lint = sh(binary, "lint", "--device", path, check=False)
    if lint.returncode != 0:
        return "lint failed: " + (lint.stderr or lint.stdout).strip()[-800:]
    for script in ("scripts/regen_supported_devices.py", "scripts/build_device_index.py"):
        sh(sys.executable, script)
    tests = sh("cargo", "test", "--release", "--lib", check=False)
    if tests.returncode != 0:
        return "tests failed: " + tests.stdout.strip()[-800:]
    changed = {line[3:] for line in sh("git", "status", "--porcelain", "-uall").stdout.splitlines()}
    stray = changed - LISTINGS - {path}
    if stray:
        return "the change touches other files: " + ", ".join(sorted(stray))
    return None


def merge(c: dict) -> str:
    branch = f"agent/contribution-{c['id']}"
    sh("git", "checkout", "-b", branch)
    sh("git", "add", c["path"], *LISTINGS)
    title = f"feat(catalogue): {c['path'].removeprefix('devices/')} (agent contribution #{c['id']})"
    body = (f"Contributed through the hosted MCP server ({c['kind']}).\n\n"
            f"{c.get('note') or ''}\n\nChecked automatically: parses, lints, catalogue regenerated, library tests pass.")
    sh("git", "-c", "user.name=roomi-fields", "-c", "user.email=roomi.git@liance.art",
       "commit", "-m", title, "-m", body, f"--author={AUTHOR}")
    sh("git", "push", "--force", "origin", branch)
    pr = sh("gh", "pr", "create", "--base", "main", "--head", branch, "--title", title, "--body", body).stdout.strip()
    sh("gh", "pr", "merge", pr, "--squash", "--delete-branch")
    return pr


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("pending")
    ap.add_argument("--binary", required=True)
    args = ap.parse_args()
    base = os.environ.get("MCP_BASE", "https://mcp.osc-bridge.roomi-fields.com")
    token = os.environ["OSC_BRIDGE_SYNC_TOKEN"]
    pending = json.loads(Path(args.pending).read_text())["contributions"]
    merged = 0
    for c in pending:
        reset()
        why = check(c, args.binary)
        if why:
            print(f"#{c['id']} {c['path']}: rejected — {why}")
            settle(base, token, c["id"], "rejected", detail=why)
            continue
        try:
            pr = merge(c)
        except subprocess.CalledProcessError as e:
            print(f"#{c['id']} {c['path']}: could not merge — {e.stderr}")
            continue  # stays pending, retried next run
        print(f"#{c['id']} {c['path']}: merged — {pr}")
        settle(base, token, c["id"], "merged", pr=pr)
        merged += 1
    reset()
    if "GITHUB_OUTPUT" in os.environ:
        with open(os.environ["GITHUB_OUTPUT"], "a") as f:
            f.write(f"merged={merged}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
