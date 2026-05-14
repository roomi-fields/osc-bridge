#!/usr/bin/env python3
"""Build `docs/devices.json` — a slim index consumed by `docs/index.html`
for client-side search/filter on GitHub Pages.

Re-run whenever `devices/` changes (or wire into CI alongside
`regen_supported_devices.py`).
"""
from __future__ import annotations
import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEVICES_DIR = REPO_ROOT / "devices"
OUT = REPO_ROOT / "docs" / "devices.json"


def classify(doc: dict) -> str:
    sources = doc.get("_sources") or []
    types = [s.get("type") for s in sources if isinstance(s, dict)]
    for t, marker in (("hardware-verified", "✅"), ("vendor-doc", "📘"),
                      ("electra-preset", "🎛️"), ("pencilresearch", "📦")):
        if t in types:
            return marker
    return "🚧"


def coverage(doc: dict) -> dict:
    cc = (doc.get("cc_params") or {}).get("entries") or []
    params = (doc.get("params") or {}).get("entries") or []
    cmds = doc.get("commands") or []
    n_14bit = sum(1 for e in cc if e.get("cc_lsb") is not None)
    n_nrpn = sum(1 for e in cc if e.get("nrpn_msb") is not None)
    return {
        "cc": len(cc), "sysex_params": len(params), "commands": len(cmds),
        "cc14": n_14bit, "nrpn": n_nrpn,
        "replies": len(doc.get("replies") or []),
    }


def main():
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
    entries.sort(key=lambda e: (e["vendor"].lower(), e["name"].lower()))
    OUT.write_text(json.dumps({
        "count": len(entries),
        "vendors": sorted({e["vendor"] for e in entries}, key=str.lower),
        "entries": entries,
    }, ensure_ascii=False, indent=2) + "\n")
    print(f"wrote {OUT} — {len(entries)} devices")


if __name__ == "__main__":
    main()
