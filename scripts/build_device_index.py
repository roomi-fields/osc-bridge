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


# The `device` keys that say which firmware a file is, not how it speaks MIDI.
IDENTITY = ("name", "vendor", "revision", "osc_prefix", "kind")


# Route lists and tables (`entries`) a `midi_of` file may add to its target's.
OWN_LISTS = ("commands", "custom_commands")
OWN_TABLES = ("params", "cc_params", "triggers")
# The most `midi_of` links followed from one file (MIDI_OF_DEPTH).
DEPTH = 8


def resolve(path: Path, doc: dict, chain: tuple = ()) -> dict:
    """A driver file as its firmware runs it, as osc-bridge loads it
    (src/device.rs, resolve_midi_of): a file naming another with `midi_of`
    (`m-vave/fm-1.vendor.fw-15`, a firmware sharing that MIDI interface)
    keeps its `_` keys and the `device` keys of IDENTITY, and takes the rest
    from the target, a driver of the same device in the same maker folder,
    itself resolved first when it names another (up to DEPTH links, never in
    a circle). Routes of its own (OWN_LISTS, the entries of
    OWN_TABLES) are appended to the target's, its own winning on the same OSC
    address. ValueError when the link is broken."""
    target = doc.get("midi_of")
    if target is None:
        return doc
    fail = lambda why: ValueError(f"{path}: midi_of {target!r}: {why}")
    parts = target.split("/") if isinstance(target, str) else []
    if len(parts) != 2 or any(not c or c.startswith(".") for c in parts):
        raise fail("not a catalogue id")
    if parts[0] != path.parent.name:
        raise fail("names another maker's driver")
    other = [k for k in doc if not k.startswith("_") and k not in ("device", "midi_of", *OWN_LISTS, *OWN_TABLES)]
    if other:
        raise fail(f"{other[0]} is the target's (a midi_of file adds routes only)")

    def entries(d: dict, key: str) -> list:
        v = d.get(key)
        return list((v if key in OWN_LISTS else (v or {}).get("entries")) or [])

    for key in (*OWN_LISTS, *OWN_TABLES):
        seen = set()
        for e in entries(doc, key):
            if not isinstance(e, dict) or "osc" not in e:
                raise fail(f"a {key} entry has no osc address")
            if e["osc"] in seen:
                raise fail(f"{e['osc']} is twice in its own {key}")
            seen.add(e["osc"])
    tpath = path.parent / f"{parts[1]}.json"
    try:
        out = json.loads(tpath.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        raise fail(e) from e
    chain = (*chain, path)
    if tpath in chain:
        raise fail("the links come back to this file (a circle)")
    if len(chain) > DEPTH:
        raise fail(f"more than {DEPTH} links")
    out = resolve(tpath, out, chain)
    if (doc.get("device") or {}).get("name") not in (None, (out.get("device") or {}).get("name")):
        raise fail("the target is another device")

    def merge(base: list, own: list) -> list:
        mine = {e["osc"] for e in own}
        return [e for e in base if not (isinstance(e, dict) and e.get("osc") in mine)] + own

    out = {k: v for k, v in out.items() if not k.startswith("_")}
    device = dict(out.pop("device", None) or {})
    for k, v in doc.items():
        if k == "device":
            for key in IDENTITY:
                if key in v:
                    device[key] = v[key]
                else:
                    device.pop(key, None)
        elif k in OWN_LISTS:
            out[k] = merge(list(out.get(k) or []), entries(doc, k))
        elif k in OWN_TABLES:
            if isinstance(out.get(k), dict):
                extra = [x for x in v if x != "entries"]
                if extra:
                    raise fail(f"{k}.{extra[0]} is the target's (a midi_of file adds entries only)")
                out[k] = {**out[k], "entries": merge(list(out[k].get("entries") or []), entries(doc, k))}
            else:
                out[k] = v
        else:
            out[k] = v
    out["device"] = device
    return out


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
            d = resolve(p, json.loads(p.read_text()))
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
        if d.get("midi_of"):
            entries[-1]["midi_of"] = d["midi_of"]
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
