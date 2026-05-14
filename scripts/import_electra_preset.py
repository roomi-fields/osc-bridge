#!/usr/bin/env python3
"""
Convert an Electra One preset JSON into an osc-bridge device JSON.

Single-file mode:
    python3 scripts/import_electra_preset.py <preset.json> [--out path] [--merge-with devices/...]

Bulk mode (every cached preset):
    python3 scripts/import_electra_preset.py --bulk .cache/electra-presets

The Electra preset format (schemaVersion 2/3 — Editor "tiles" layout) puts
each MIDI binding inline in `tiles[*].values[*].message` with one of these types:

    cc7, cc14, nrpn, sysex, program, note, atpoly, atchannel, pitchbend,
    virtual (UI-only — skipped), none (skipped).

This converter walks all tiles and emits:
  - `cc_params.entries[]` for cc7 / cc14 / nrpn
  - `commands[]` for sysex (with `data` array translated to our placeholder DSL)
  - everything else: warnings logged, skipped

When `--merge-with <existing.json>` is provided, the converter LAYERS the
SysEx commands and CC entries on top of the existing device, preserving any
prior fields (e.g. previously-imported pencilresearch CC/NRPN). The
`_sources[]` array gains a new entry of type "electra-preset".
"""
from __future__ import annotations
import argparse
import json
import re
import sys
from collections import OrderedDict
from datetime import date
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEVICES_DIR = REPO_ROOT / "devices"


# ----- helpers -----

def slug(s: str) -> str:
    s = (s or "").lower().strip()
    s = re.sub(r"\(nrpn\)", "", s)
    s = re.sub(r"[^a-z0-9]+", "_", s).strip("_")
    return s or "unnamed"


def vendor_slug(s: str) -> str:
    s = (s or "").lower().strip()
    s = re.sub(r"[^a-z0-9]+", "-", s).strip("-")
    return s or "unknown"


# Best-effort vendor inference from a free-form preset name.
KNOWN_VENDORS = [
    "Sequential", "Dave Smith Instruments", "DSI", "Moog", "Arturia",
    "Novation", "KORG", "Korg", "Yamaha", "Roland", "Behringer", "Waldorf",
    "Elektron", "Dreadbox", "Erica Synths", "Polyend", "Modal", "Modal Electronics",
    "Bastl", "Make Noise", "Mutable Instruments", "Black Corporation",
    "Expressive E", "Tasty Chips", "Hologram", "Strymon", "Empress",
    "Chase Bliss", "Meris", "Eventide", "OTO Machines", "ASM", "Access",
    "Oberheim", "1010music", "Beetlecrab", "Jomox", "Twisted Electrons",
    "UDO", "UDO Audio", "Synthstrom", "Pioneer", "Zoom", "TC Electronic",
    "Teenage Engineering", "Torso", "Endorphin.es", "Conductive Labs",
    "Audiothingies", "Sherman", "Sonicware", "Studio Electronics",
    "Pearl Drum", "Nord", "Kawai", "MFB", "Ensoniq", "Tubbutec", "Norand",
    "Plinky", "RED Sound", "Rhodes", "Haken", "Spitfire", "Soundtoys",
    "Cherry Audio", "u-he", "Mutable", "Klavis", "Antonus", "Abildgard",
]


_EXISTING_INDEX_CACHE: dict[str, tuple[str, Path]] | None = None  # devname_slug -> (vendor, path)


def _existing_index() -> dict[str, tuple[str, Path]]:
    """Cache of {device-name-slug: (vendor, path)} across our catalogue, used
    to resolve vendor when a preset name doesn't carry a known vendor prefix."""
    global _EXISTING_INDEX_CACHE
    if _EXISTING_INDEX_CACHE is not None:
        return _EXISTING_INDEX_CACHE
    idx: dict[str, tuple[str, Path]] = {}
    if not DEVICES_DIR.is_dir():
        _EXISTING_INDEX_CACHE = idx
        return idx
    for p in DEVICES_DIR.rglob("*.json"):
        if "examples" in p.parts:
            continue
        try: d = json.loads(p.read_text())
        except: continue
        n = slug(d.get("device", {}).get("name", ""))
        v = d.get("device", {}).get("vendor", "")
        if n and v:
            idx.setdefault(n, (v, p))
    _EXISTING_INDEX_CACHE = idx
    return idx


def infer_vendor(preset_name: str) -> tuple[str, str]:
    """Return (vendor, device_name) for a free-form preset title.

    Resolution order:
      1. Exact match against our existing catalogue → reuse that vendor
         (so e.g. preset "Nymphes" picks up vendor "Dreadbox" automatically).
      2. Known-vendor prefix in the title → split.
      3. Otherwise → ("Electra Community", preset_name) so the device lands
         in a clearly-tagged bucket instead of fabricating a vendor folder.
    """
    n = (preset_name or "").strip()
    s = slug(n)
    if s and s in _existing_index():
        v, _ = _existing_index()[s]
        return v, n
    for v in sorted(KNOWN_VENDORS, key=len, reverse=True):
        if n.lower().startswith(v.lower() + " "):
            return v, n[len(v) + 1:].strip()
    return "Electra Community", n or "unnamed"


def coerce_byte(b) -> int | None:
    """`'XX'` hex string → int. Other types: None (handled as placeholder)."""
    if isinstance(b, str):
        try:
            return int(b, 16)
        except ValueError:
            return None
    if isinstance(b, int):
        return b
    return None


# ----- per-message translation -----

def translate_sysex_data(data: list, value_id: str = "val") -> list:
    """Convert Electra SysEx `data` array to our FrameToken format.

    `data` is a mix of:
      - hex strings: "F0", "00", … → integers
      - dicts: {"type":"value"} or {"type":"value","rules":[…]} → placeholders
        We map them to "{val}" — for now a single u7 value substitution.
        14-bit / NRPN-as-SysEx with rules is not yet handled (logged as warn).

    Note: our schema's `frame[]` is the body BETWEEN the device's
    `sysex.header` and `sysex.footer`. The Electra `data[]` includes the F0/F7,
    so we strip them here.
    """
    out = []
    n = len(data)
    for i, b in enumerate(data):
        if isinstance(b, str):
            v = coerce_byte(b)
            if v is None: continue
            if i == 0 and v == 0xF0: continue
            if i == n - 1 and v == 0xF7: continue
            out.append(v)
        elif isinstance(b, dict):
            t = b.get("type", "value")
            if t == "value":
                out.append(f"{{{value_id}}}")
            elif t == "checksum":
                out.append(f"{{checksum}}")
            else:
                # Unknown placeholder type — keep verbatim
                out.append(f"{{{t}}}")
    return out


def make_osc_path(name: str, section: str | None) -> str:
    n = slug(name)
    if section:
        s = slug(section)
        if s and s != n:
            return f"/{s}/{n}"
    return f"/{n}"


# ----- preset → osc-bridge device dict -----

def convert_preset(preset: dict) -> dict | None:
    """Build a fresh device dict from an Electra preset."""
    schema = preset.get("schemaVersion")
    name = preset.get("name") or "unnamed"
    devices = preset.get("devices") or []
    if not devices:
        return None

    # Use first device's channel as default
    primary_dev = devices[0]
    default_channel = max(0, int(primary_dev.get("channel", 1)) - 1)  # Electra is 1-indexed

    cache_meta = preset.get("_cache_meta", {})
    pid = cache_meta.get("id", "")
    revision = cache_meta.get("revision", 0)
    preset_url = f"https://app.electra.one/preset/{pid}" if pid else None

    cc_entries: list[dict] = []
    commands: list[dict] = []
    seen_cc: set[tuple] = set()
    seen_cmd: set[str] = set()

    # Use category labels (when present) to enrich OSC paths.
    # Electra has `categories[]` referenced by tile.categoryId.
    categories = {c.get("id"): c.get("name") for c in preset.get("categories", []) if c.get("id")}

    skip_count = {"virtual": 0, "none": 0, "unknown": 0, "no_message": 0}

    for tile in preset.get("tiles", []):
        ttype = tile.get("type", "")
        # Skip pure UI tiles (label, separator, …)
        if ttype in ("label", "separator", "page", "group"):
            continue
        tname = tile.get("name", "").strip()
        if not tname:
            continue
        section = categories.get(tile.get("categoryId"))
        for v in tile.get("values", []):
            msg = v.get("message", {}) or {}
            mtype = msg.get("type", "")
            if not mtype:
                skip_count["no_message"] += 1
                continue
            if mtype in ("virtual", "none"):
                skip_count[mtype] += 1
                continue

            # Range from value object (display range), fallback to message min/max,
            # then to 0..127 if either side is missing/None.
            def _i(x, default):
                try:
                    return int(x) if x is not None else default
                except (TypeError, ValueError):
                    return default
            rmin = _i(v.get("min", msg.get("min")), 0)
            rmax = _i(v.get("max", msg.get("max")), 127)

            if mtype == "cc7":
                cc_num = msg.get("parameterNumber")
                if cc_num is None: continue
                key = ("cc7", msg.get("deviceId", 1), cc_num)
                if key in seen_cc: continue
                seen_cc.add(key)
                cc_entries.append({
                    "osc": make_osc_path(tname, section),
                    "cc": int(cc_num) & 0x7F,
                    "range": [int(rmin), int(rmax)],
                    "section": section or "",
                })
            elif mtype == "cc14":
                cc_num = msg.get("parameterNumber")
                if cc_num is None: continue
                # Electra cc14 = MSB on cc_num, LSB on cc_num+32 (standard)
                key = ("cc14", msg.get("deviceId", 1), cc_num)
                if key in seen_cc: continue
                seen_cc.add(key)
                cc_entries.append({
                    "osc": make_osc_path(tname, section),
                    "cc": int(cc_num) & 0x7F,
                    "cc_lsb": (int(cc_num) + 32) & 0x7F,
                    "range": [int(rmin), int(rmax)],
                    "section": section or "",
                })
            elif mtype == "nrpn":
                nrpn = msg.get("parameterNumber")
                if nrpn is None: continue
                nrpn_msb = (int(nrpn) >> 7) & 0x7F
                nrpn_lsb = int(nrpn) & 0x7F
                key = ("nrpn", msg.get("deviceId", 1), nrpn)
                if key in seen_cc: continue
                seen_cc.add(key)
                cc_entries.append({
                    "osc": make_osc_path(tname, section),
                    "nrpn_msb": nrpn_msb,
                    "nrpn_lsb": nrpn_lsb,
                    "range": [int(rmin), int(rmax)],
                    "section": section or "",
                })
            elif mtype == "sysex":
                data = msg.get("data") or []
                frame = translate_sysex_data(data)
                if not frame:
                    continue
                osc = make_osc_path(tname, section)
                if osc in seen_cmd:
                    continue
                seen_cmd.add(osc)
                cmd = {
                    "osc": osc,
                    "args": [{"name": "val", "type": "u7", "range": [int(rmin), int(rmax)]}],
                    "frame": frame,
                }
                if section:
                    cmd["section"] = section
                commands.append(cmd)
            elif mtype == "program":
                # Single-shot program change — expose as a command
                osc = "/program_change_set" if not tname else make_osc_path(tname, section)
                if osc in seen_cmd: continue
                seen_cmd.add(osc)
                # A bare PC is sent without sysex; until our schema models PC commands
                # explicitly we represent it as a CC at parameterNumber so it's at
                # least recorded. Better mapping = TODO.
                # Skipping for now to avoid emitting wrong frames.
                continue
            else:
                skip_count.setdefault(mtype, 0)
                skip_count[mtype] += 1

    # Electra device sysex header: typically nothing common (each cmd is full F0…F7
    # in our `data`). Set header/footer empty so commands carry their own opening F0.
    # Wait — our build_frame ALWAYS prepends sysex.header and appends sysex.footer.
    # We strip F0/F7 from `data[]` above. So set header=[F0], footer=[F7].
    # However, per-tile devices may share the manufacturer ID prefix — we keep the
    # full byte sequence inside the command's frame, header/footer minimal.
    sysex_header = [0xF0] if commands else []
    sysex_footer = [0xF7] if commands else []

    # Try to extract manufacturer prefix common to ALL sysex frames (longest common).
    if commands:
        all_byte_prefixes = []
        for c in commands:
            bytes_only = [b for b in c["frame"] if isinstance(b, int)]
            all_byte_prefixes.append(bytes_only)
        # Longest common prefix among all
        if all_byte_prefixes:
            prefix = list(all_byte_prefixes[0])
            for bp in all_byte_prefixes[1:]:
                # truncate prefix to common
                new = []
                for a, b in zip(prefix, bp):
                    if a == b: new.append(a)
                    else: break
                prefix = new
                if not prefix: break
            # Only factor if we save bytes (>=3 common bytes) and commands all have integers at start
            if len(prefix) >= 3:
                sysex_header = [0xF0] + prefix
                # strip from each command frame (only the leading int prefix)
                for c in commands:
                    n = 0
                    for tok in c["frame"]:
                        if isinstance(tok, int) and n < len(prefix) and tok == prefix[n]:
                            n += 1
                        else:
                            break
                    c["frame"] = c["frame"][n:]

    vendor, device_name = infer_vendor(name)

    src = {
        "type": "electra-preset",
        "url": preset_url or "https://app.electra.one/presets/",
        "preset_name": name,
        "imported_at": date.today().isoformat(),
    }
    if pid:
        src["preset_id"] = pid
    if revision:
        src["revision"] = revision

    doc = {
        "_sources": [src],
        "_limitations": (
            "Imported from an Electra One community preset. The mapping reflects "
            "the preset author's editorial choices: parameter naming, ranges, and "
            "which params are exposed. Untested on hardware in this repo."
        ),
        "_coverage": {
            "entries_total": len(cc_entries) + len(commands),
            "cc_entries": len(cc_entries),
            "sysex_commands": len(commands),
        },
        "device": {
            "name": device_name,
            "vendor": vendor,
            "revision": "per Electra preset",
            "osc_prefix": "/" + slug(device_name),
            "manufacturer_id": [],
            "device_id": [],
            "rate_limit_hz": 1000,
        },
        "sysex": {"header": sysex_header, "footer": sysex_footer},
        "commands": commands,
        "cc_params": {"channel": default_channel, "entries": cc_entries} if cc_entries else None,
        "midi_out": {"default_channel": default_channel, "note_offset": 0},
        "midi_in": {
            "note_on":   "/note/on {note} {velocity} {channel}",
            "note_off":  "/note/off {note} {velocity} {channel}",
            "cc":        "/cc/{num} {value} {channel}",
            "pitchbend": "/pitchbend {value_u14} {channel}",
            "aftertouch":"/aftertouch {value} {channel}",
        },
        "replies": [],
    }
    if doc["cc_params"] is None:
        del doc["cc_params"]
    return doc


def merge_into_existing(existing: dict, new_doc: dict) -> dict:
    """Layer SysEx commands and (extra) CC entries from new_doc onto existing.

    Preservation rules:
      - existing.commands: extended with new_doc.commands not already present (by `osc`).
      - existing.cc_params: extended with new entries whose `osc` isn't already present.
      - existing._sources: prepend the electra source if not already there.
      - existing.sysex.header/footer: kept verbatim if non-empty; else taken from new.
      - everything else (device, midi_out, replies): kept.
    """
    # _sources
    sources = existing.setdefault("_sources", [])
    new_src = new_doc["_sources"][0]
    sources = [s for s in sources if s.get("type") != "electra-preset"]
    sources.insert(0, new_src)
    existing["_sources"] = sources

    # commands
    existing_commands = existing.setdefault("commands", [])
    seen = {c.get("osc") for c in existing_commands}
    for c in new_doc.get("commands", []):
        if c["osc"] not in seen:
            existing_commands.append(c)

    # sysex header/footer — only fill if existing is empty (don't overwrite manual work)
    if not existing.get("sysex", {}).get("header") and new_doc["sysex"]["header"]:
        existing.setdefault("sysex", {})["header"] = new_doc["sysex"]["header"]
        existing["sysex"]["footer"] = new_doc["sysex"]["footer"]

    # cc_params — append new oscs only
    if "cc_params" not in existing and "cc_params" in new_doc:
        existing["cc_params"] = new_doc["cc_params"]
    elif "cc_params" in new_doc:
        existing_entries = existing["cc_params"].setdefault("entries", [])
        seen_osc = {e.get("osc") for e in existing_entries}
        for e in new_doc["cc_params"].get("entries", []):
            if e["osc"] not in seen_osc:
                existing_entries.append(e)

    # coverage
    existing["_coverage"] = {
        "entries_total": len(existing.get("commands", [])) + len(existing.get("cc_params", {}).get("entries", [])),
        "cc_entries": len(existing.get("cc_params", {}).get("entries", [])),
        "sysex_commands": len(existing.get("commands", [])),
    }
    return existing


def find_existing_match(new_doc: dict) -> Path | None:
    """Look in devices/ for an existing device that matches the new one.

    Matching rules (strict, to avoid merging into an unrelated device):
      - must have BOTH the same vendor (slug match) AND a matching device-name slug;
      - device-name match = exact equality, OR one slug is a prefix of the
        other AND the prefix is at least 4 chars AND ends on a `_` boundary
        (so e.g. "matriarch" matches "matriarch_globals" but "example" does
        NOT match "scripted_example").
    Generic vendors like "Electra Community" / "Unknown" never match (we never
    want to merge a generic-vendor preset into a real-vendor device).
    """
    new_vendor = vendor_slug(new_doc["device"].get("vendor", ""))
    new_dev = slug(new_doc["device"].get("name", ""))
    if not new_dev or not new_vendor or new_vendor in {"electra-community", "unknown"}:
        return None
    if len(new_dev) < 4:
        return None  # too generic to match safely
    for p in DEVICES_DIR.rglob("*.json"):
        if p.parts and "examples" in p.parts:
            continue  # never merge into example/demo devices
        try: d = json.loads(p.read_text())
        except: continue
        ev = vendor_slug(d.get("device", {}).get("vendor", ""))
        edev = slug(d.get("device", {}).get("name", ""))
        if ev != new_vendor:
            continue
        if edev == new_dev:
            return p
        # Word-boundary prefix match (length floor 4 already enforced)
        if new_dev.startswith(edev + "_") or edev.startswith(new_dev + "_"):
            return p
    return None


def output_path_for_new(new_doc: dict) -> Path:
    v = vendor_slug(new_doc["device"]["vendor"])
    s = slug(new_doc["device"]["name"])
    return DEVICES_DIR / v / f"{s}.json"


def import_one(preset_path: Path, *, force_new: bool = False, merge_with: Path | None = None,
               out: Path | None = None, source_type: str | None = None) -> tuple[Path | None, str]:
    preset = json.loads(preset_path.read_text())
    new_doc = convert_preset(preset)
    if new_doc is None:
        return None, f"skip (no devices): {preset_path.name}"
    if not new_doc["commands"] and not new_doc.get("cc_params"):
        return None, f"skip (no useful tiles): {preset_path.name}"
    if source_type:
        for s in new_doc.get("_sources") or []:
            if isinstance(s, dict) and s.get("type") == "electra-preset":
                s["type"] = source_type

    target = merge_with or (None if force_new else find_existing_match(new_doc))
    if target and target.is_file():
        existing = json.loads(target.read_text())
        merged = merge_into_existing(existing, new_doc)
        target.write_text(json.dumps(merged, indent=2) + "\n")
        return target, f"MERGED {target.relative_to(REPO_ROOT)}  +{len(new_doc.get('commands',[]))} cmds, +{len(new_doc.get('cc_params',{}).get('entries',[]))} cc"
    out = out or output_path_for_new(new_doc)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(new_doc, indent=2) + "\n")
    label = out.relative_to(REPO_ROOT) if out.is_relative_to(REPO_ROOT) else out
    return out, f"NEW {label}  {len(new_doc.get('commands',[]))} cmds, {len(new_doc.get('cc_params',{}).get('entries',[]))} cc"


def bulk(cache_dir: Path):
    cnt = {"merged": 0, "new": 0, "skipped": 0, "errors": 0}
    for f in sorted(cache_dir.glob("*.json")):
        try:
            out, msg = import_one(f)
            print("  " + msg)
            if msg.startswith("MERGED"): cnt["merged"] += 1
            elif msg.startswith("NEW"): cnt["new"] += 1
            else: cnt["skipped"] += 1
        except Exception as e:
            cnt["errors"] += 1
            print(f"  FAIL {f.name}: {e}")
    print(f"\n=== bulk: {cnt['merged']} merged, {cnt['new']} new, {cnt['skipped']} skipped, {cnt['errors']} errors ===")


def main():
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--bulk", metavar="DIR", help="convert every .json in DIR")
    g.add_argument("--merge-with", metavar="JSON", help="layer onto an existing device")
    ap.add_argument("--new", action="store_true", help="always create a new file (don't auto-merge)")
    ap.add_argument("--source-type", default=None,
                    help="override the _sources[].type tag (default: 'electra-preset')")
    ap.add_argument("--out", help="output path (single mode)")
    ap.add_argument("preset", nargs="?", help="preset JSON to import")
    args = ap.parse_args()

    if args.bulk:
        bulk(Path(args.bulk))
        return
    if not args.preset:
        ap.print_help(); sys.exit(2)
    out, msg = import_one(
        Path(args.preset),
        force_new=args.new,
        merge_with=Path(args.merge_with) if args.merge_with else None,
        out=Path(args.out) if args.out else None,
        source_type=args.source_type,
    )
    print(msg)


if __name__ == "__main__":
    main()
