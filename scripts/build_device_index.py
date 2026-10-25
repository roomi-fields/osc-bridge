#!/usr/bin/env python3
"""Build `docs/devices.json` — a slim index consumed by `docs/index.html`
for client-side search/filter on GitHub Pages.

Re-run whenever `devices/` changes (or wire into CI alongside
`regen_supported_devices.py`).

Each entry records when its driver file was `added` and last `updated`
(YYYY-MM-DD, from git history), for the pages SynthHub renders, which have
no history to read. A file not committed yet is dated today; outside a git
checkout an entry keeps the dates the previous docs/devices.json gave it.
"""
from __future__ import annotations
import json
import subprocess
from datetime import date
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEVICES_DIR = REPO_ROOT / "devices"
OUT = REPO_ROOT / "docs" / "devices.json"


def classify(doc: dict) -> str:
    sources = doc.get("_sources") or []
    types = [s.get("type") for s in sources if isinstance(s, dict)]
    for t, marker in (("hardware-verified", "✅"), ("vendor-doc", "📘"),
                      ("electra-preset", "🎛️"), ("pencilresearch", "📦"), ("knobkraft", "💾"),
                      ("agent-contributed", "🤖"), ("test-fixture", "🧪")):
        if t in types:
            return marker
    return "🚧"


def coverage(doc: dict) -> dict:
    cc = (doc.get("cc_params") or {}).get("entries") or []
    params = (doc.get("params") or {}).get("entries") or []
    cmds = doc.get("commands") or []
    n_14bit = sum(1 for e in cc if e.get("cc_lsb") is not None)
    n_nrpn = sum(1 for e in cc if e.get("nrpn_msb") is not None)
    # Count surface entries whose `expects` is annotated beyond the default
    # "continuous" — informative only, lets the Pages UI surface typed controls.
    typed = {}
    for e in (*cc, *params):
        nature = e.get("expects")
        if nature and nature != "continuous":
            typed[nature] = typed.get(nature, 0) + 1
    cov = {
        "cc": len(cc), "sysex_params": len(params), "commands": len(cmds),
        "cc14": n_14bit, "nrpn": n_nrpn,
        "replies": len(doc.get("replies") or []),
        "triggers": len((doc.get("triggers") or {}).get("entries") or []),
    }
    if typed:
        cov["expects"] = dict(sorted(typed.items()))
    return cov


def dates() -> dict[str, tuple[str, str]]:
    """(added, updated) of every file under devices/, from one `git log`
    pass, files changed since the last commit updated today; empty outside a
    git checkout."""
    try:
        out = subprocess.run(["git", "log", "--format=@%cs", "--name-only", "--", "devices"],
                             cwd=REPO_ROOT, capture_output=True, text=True, check=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return {}
    found: dict[str, tuple[str, str]] = {}
    when = None
    for line in out.splitlines():
        if line.startswith("@"):
            when = line[1:]
        elif line and when:
            _, updated = found.get(line, (when, when))
            found[line] = (when, updated)   # newest first: the last seen is the first commit
    try:
        dirty = subprocess.run(["git", "status", "--porcelain", "--", "devices"],
                               cwd=REPO_ROOT, capture_output=True, text=True, check=True).stdout
    except (OSError, subprocess.CalledProcessError):
        dirty = ""
    today = date.today().isoformat()
    for line in dirty.splitlines():   # changed since the last commit: updated today
        path = line[3:].strip().strip('"')
        found[path] = (found.get(path, (today, today))[0], today)
    return found


def main():
    try:
        before = {e["path"]: e for e in json.loads(OUT.read_text()).get("entries", [])}
    except (OSError, ValueError):
        before = {}
    history = dates()
    entries = []
    for p in sorted(DEVICES_DIR.rglob("*.json")):
        try:
            d = json.loads(p.read_text())
        except Exception:
            continue
        dev = d.get("device") or {}
        vendor = dev.get("vendor") or p.parent.name
        name = dev.get("name") or p.stem
        sources = d.get("_sources") or []
        authors = sorted({s.get("author") for s in sources
                          if isinstance(s, dict) and s.get("author")})
        urls = [s.get("url") for s in sources if isinstance(s, dict) and s.get("url")]
        entries.append({
            "vendor": vendor,
            "name": name,
            "path": str(p.relative_to(REPO_ROOT)).replace("\\", "/"),
            "marker": classify(d),
            "source_types": sorted({s.get("type") for s in sources
                                    if isinstance(s, dict) and s.get("type")}),
            "authors": authors,
            "urls": urls[:2],
            "coverage": coverage(d),
        })
        path = entries[-1]["path"]
        today = date.today().isoformat()
        added, updated = history.get(path) or ((today, today) if history else
                                               (before.get(path, {}).get("added"), before.get(path, {}).get("updated")))
        if added:
            entries[-1]["added"] = added
        if updated:
            entries[-1]["updated"] = updated
    entries.sort(key=lambda e: (e["vendor"].lower(), e["name"].lower()))
    OUT.write_text(json.dumps({
        "count": len(entries),
        "vendors": sorted({e["vendor"] for e in entries}, key=lambda v: (v.lower(), v)),
        "entries": entries,
    }, ensure_ascii=False, indent=2) + "\n")
    print(f"wrote {OUT} — {len(entries)} devices")


if __name__ == "__main__":
    main()
