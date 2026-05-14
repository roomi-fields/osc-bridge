#!/usr/bin/env python3
"""Curate + import Electra One presets from a local cache into osc-bridge.

Maintainer-only bulk-import helper. Reads a local cache directory of Electra
preset JSON files and selects the richest one per device.

Input:   .cache/electra-import/*.json  (local cache, not tracked)
Output:  new devices/<vendor>/<slug>.json entries.

Selection pipeline (conservative):
  1. Drop presets without any MIDI messages (Lua-only, empty, unfinished).
  2. Drop presets whose declared device name is a generic placeholder
     (Generic MIDI, MIDI Device 1, Current Channel, Test device, …).
  3. Drop preset-instance labels (Part 1, Ch 3, Scene …) — these are
     sub-devices of a multi-patch setup, not a device in their own right.
  4. Collapse near-duplicates by a canonical key (lowercased, alnum-only).
  5. For each remaining device name, pick the preset with the most
     messages as the canonical import.
  6. Skip names we already cover (case-insensitive slug match against
     existing devices[]/device.name).
  7. Require ≥50 messages (lower = probably a stub or WIP).

Then delegate to `scripts/import_electra_preset.py --new` for the actual
conversion.

Usage:
    python3 scripts/import_electra_cache.py            # full run
    python3 scripts/import_electra_cache.py --dry-run  # just list
    python3 scripts/import_electra_cache.py --limit 20 # first N
"""
from __future__ import annotations
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
D = REPO_ROOT / ".cache" / "electra-import"
DEVICES_DIR = REPO_ROOT / "devices"
IMPORTER = REPO_ROOT / "scripts" / "import_electra_preset.py"


def slug(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", (s or "").lower().strip()).strip("-")


def canon(s: str) -> str:
    """Collapse spacing/punct + fix common misspellings to dedupe near-duplicates."""
    s = s.lower()
    s = s.replace("rolamd", "roland")
    s = re.sub(r"[^a-z0-9]+", "", s)
    return s


REJECT_RE = re.compile(
    r"^(part|channel|ch|scene|layer|slot|bank|rack|hdhp|fh|fs|main|rcv|to |from |"
    r"s|t|u|tr|hd|dev\.?|fl |1\.|2\.|3\.|4\.|test|template|wip|demo|default|"
    r"generic|general|current|my |synth |device |untitled|unnamed|no\s?name|"
    r"midi\s+device|midi\s+keyboard|controllers?\b)",
    re.I,
)
NUMERIC_RE = re.compile(r"^[\d\s\-_\.]+$")
MULTI_PART_RE = re.compile(r"\b(part|ch|slot|layer|voice|scene|snd|s)\s*[0-9]+\b", re.I)


def count_msgs(preset: dict) -> int:
    n = 0
    for tile in preset.get("tiles") or []:
        if not isinstance(tile, dict):
            continue
        for v in tile.get("values") or []:
            if isinstance(v, dict) and (v.get("message") or {}).get("type"):
                n += 1
    return n


def existing_device_slugs() -> set[str]:
    out: set[str] = set()
    for p in DEVICES_DIR.rglob("*.json"):
        try:
            d = json.loads(p.read_text())
        except Exception:
            continue
        n = (d.get("device") or {}).get("name", "")
        if n:
            out.add(slug(n))
            out.add(canon(n))
    return out


def select() -> list[dict]:
    existing = existing_device_slugs()
    by_device: dict[str, list[tuple[int, Path, str]]] = defaultdict(list)
    for p in D.glob("*.json"):
        try:
            d = json.loads(p.read_text())
        except Exception:
            continue
        if not isinstance(d, dict):
            continue
        devs = d.get("devices") or []
        if not devs or not isinstance(devs[0], dict):
            continue
        name = (devs[0].get("name") or "").strip()
        if len(name) < 3 or REJECT_RE.match(name) or NUMERIC_RE.match(name) or MULTI_PART_RE.search(name):
            continue
        n = count_msgs(d)
        if n < 50:
            continue
        by_device[canon(name)].append((n, p, name))

    candidates: list[dict] = []
    for ckey, lst in by_device.items():
        if ckey in existing:
            continue
        # Pick the richest preset in the group
        best = max(lst, key=lambda t: t[0])
        display = best[2]
        if slug(display) in existing:
            continue
        candidates.append({
            "name": display,
            "msgs": best[0],
            "variants": len(lst),
            "path": str(best[1]),
        })
    candidates.sort(key=lambda c: -c["msgs"])
    return candidates


def main():
    dry = "--dry-run" in sys.argv
    limit = None
    if "--limit" in sys.argv:
        limit = int(sys.argv[sys.argv.index("--limit") + 1])

    if not D.is_dir():
        print(f"no {D}"); return
    cands = select()
    if limit:
        cands = cands[:limit]
    print(f"candidates: {len(cands)}")
    if dry:
        for c in cands:
            print(f"  msgs={c['msgs']:4d}  v={c['variants']:3d}   {c['name']}")
        return

    ok, fail = 0, 0
    for c in cands:
        try:
            r = subprocess.run(
                ["python3", str(IMPORTER), "--new", c["path"],
                 "--source-type", "electra-preset"],
                capture_output=True, text=True, timeout=60,
            )
            if r.returncode == 0:
                ok += 1
            else:
                fail += 1
                print(f"  FAIL {c['name']}: {r.stderr.strip()[:180]}")
        except Exception as e:
            fail += 1
            print(f"  FAIL {c['name']}: {e}")
        if (ok + fail) % 25 == 0:
            print(f"  {ok+fail}/{len(cands)} processed (ok={ok})")
    print(f"\nimported {ok} / failed {fail} / total {len(cands)}")


if __name__ == "__main__":
    main()
