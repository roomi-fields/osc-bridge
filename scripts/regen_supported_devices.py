#!/usr/bin/env python3
"""
Regenerate `docs/SUPPORTED_DEVICES.md` from every `devices/**/*.json`.

The JSONs are the single source of truth. Running this script should be
idempotent — CI runs it and fails if the Markdown would have been
changed, forcing contributors to keep the listing up to date.

Usage:
    python3 scripts/regen_supported_devices.py
"""
from __future__ import annotations
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEVICES_DIR = REPO_ROOT / "devices"
OUTPUT = REPO_ROOT / "docs" / "SUPPORTED_DEVICES.md"


def classify_status(doc: dict, path: Path) -> tuple[str, str]:
    """Return (marker, kind).

    marker ∈ {✅, 📘, 🎛️, 📡, 📦, 🚧, 📝}
    kind   ∈ {hardware-verified, vendor-doc, vendor-osc-api, electra-preset,
              third-party-osc, pencilresearch, wip, stub}

    Order of priority across the `_sources[]` array:
      1. ✅ if any source is `hardware-verified` → most authoritative.
      2. 📘 if any source is `vendor-doc` (manufacturer reference).
      3. 📡 if any source is `vendor-osc-api` (officially documented OSC).
      4. 🎛️ if any source is `electra-preset` (community SysEx + CC).
      5. 📡 if any source is `third-party-osc` (community OSC shim — DAW
         remote-script style, e.g. AbletonOSC).
      6. 📦 if the primary source is `pencilresearch` (CC/NRPN only).
      7. 🚧 / 📝 fallbacks.

    Layered devices (e.g. pencilresearch + electra-preset) get the more
    informative marker. The 📡 satellite-dish marker is shared by both
    `vendor-osc-api` and `third-party-osc` — the distinction is captured
    in `kind` for the source summary, not in the visible icon.
    """
    sources = doc.get("_sources")
    if isinstance(sources, list) and sources:
        types = [s.get("type") for s in sources if isinstance(s, dict)]
        if "hardware-verified" in types or "hardware-verified-partial" in types:
            return ("✅", "hardware-verified")
        if "software-verified" in types:
            return ("✅", "software-verified")
        if "vendor-doc" in types:
            return ("📘", "vendor-doc")
        if "vendor-osc-api" in types:
            return ("📡", "vendor-osc-api")
        if "electra-preset" in types:
            return ("🎛️", "electra-preset")
        if "third-party-osc" in types:
            return ("📡", "third-party-osc")
        if "pencilresearch" in types:
            return ("📦", "pencilresearch")
        # Unknown source type — best-effort
        if types:
            return ("📘", types[0])
    if doc.get("_source"):  # legacy unstructured source = treat as vendor-doc best guess
        return ("📘", "vendor-doc")
    if any(str(doc.get(k, "")).lower().find("stub") >= 0 for k in ("_note", "_comment")):
        return ("📝", "stub")
    return ("🚧", "wip")


def summarise_coverage(doc: dict) -> str:
    """One-liner like "31 SysEx cmds + 12 replies" / "82 CC (5 14-bit)" / "— stub —"."""
    bits = []
    n_cmds = len(doc.get("commands", []))
    transport = (doc.get("device") or {}).get("transport") or {}
    is_osc = transport.get("kind") == "osc"
    cmd_label = "OSC forward" if is_osc else "SysEx command"
    if n_cmds: bits.append(f"{n_cmds} {cmd_label}{'s' if n_cmds != 1 else ''}")
    params = doc.get("params", {})
    if params and params.get("entries"):
        bits.append(f"{len(params['entries'])} SysEx param{'s' if len(params['entries']) != 1 else ''}")
    cc = doc.get("cc_params", {})
    if cc and cc.get("entries"):
        n = len(cc["entries"])
        n_14bit = sum(1 for e in cc["entries"] if e.get("cc_lsb") is not None)
        n_nrpn = sum(1 for e in cc["entries"] if e.get("nrpn_msb") is not None)
        extras = []
        if n_14bit: extras.append(f"{n_14bit} 14-bit")
        if n_nrpn:  extras.append(f"{n_nrpn} NRPN")
        suffix = f" ({', '.join(extras)})" if extras else ""
        bits.append(f"{n} CC{suffix}")
    replies = doc.get("replies", [])
    if replies: bits.append(f"{len(replies)} repl{'ies' if len(replies) != 1 else 'y'}")
    mo = doc.get("midi_out")
    if mo is not None: bits.append("midi_out")
    return " · ".join(bits) if bits else "—"


def source_summary(doc: dict) -> tuple[str, str | None]:
    """Return a short label + optional source URL for the main source."""
    sources = doc.get("_sources")
    if isinstance(sources, list) and sources:
        s = sources[0]
        if isinstance(s, dict):
            t = s.get("type", "doc")
            url = s.get("url")
            if t == "pencilresearch":
                return "pencilresearch", url
            if t == "vendor-doc":
                return "vendor spec", url
            if t == "vendor-osc-api":
                return "vendor OSC API", url
            if t == "third-party-osc":
                return "third-party OSC shim", url
            if t == "hardware-verified":
                return "hardware test", url
            if t == "hardware-verified-partial":
                return "hardware test (partial)", url
            if t == "software-verified":
                return "verified on running software target", url
            return t, url
    legacy = doc.get("_source")
    if legacy:
        return "legacy _source", None
    return "—", None


def load_devices() -> list[tuple[Path, dict]]:
    out = []
    for p in sorted(DEVICES_DIR.rglob("*.json")):
        try:
            doc = json.loads(p.read_text())
        except Exception as e:
            print(f"error: {p}: {e}", file=sys.stderr); continue
        out.append((p, doc))
    return out


STATUS_PRIORITY = {"✅": 5, "📘": 4, "📡": 3, "🎛️": 2, "📦": 1, "🚧": 0, "📝": 0}


def best_status(variants: list[tuple[Path, dict]]) -> str:
    """Return the highest-tier marker across a device's variants."""
    return max((classify_status(d, p)[0] for p, d in variants), key=lambda s: STATUS_PRIORITY.get(s, 0))


def firmware_of(doc: dict) -> str:
    """Firmware tag for a variant: prefer _sources[].firmware, fall back to device.revision."""
    sources = doc.get("_sources") or []
    for s in sources:
        if isinstance(s, dict) and s.get("firmware"):
            return str(s["firmware"])
    return ""


def render(devices: list[tuple[Path, dict]]) -> str:
    # Stats: per-file (not per-device-group) so the tier counts still reflect
    # the actual number of driver files shipped.
    total_files = len(devices)
    by_status = {"✅": 0, "📘": 0, "📡": 0, "🎛️": 0, "📦": 0, "🚧": 0, "📝": 0}
    for path, d in devices:
        by_status[classify_status(d, path)[0]] += 1

    # Group by (vendor, device.name) so multiple variants of the same device
    # — e.g. tempera.vendor.fw-2.2.json + tempera.electra-community.fw-unknown.json —
    # show up as one entry with several listed variants.
    by_vendor: dict[str, dict[str, list[tuple[Path, dict]]]] = {}
    for path, doc in devices:
        vendor = doc.get("device", {}).get("vendor") or path.parent.name
        name = doc.get("device", {}).get("name") or path.stem
        by_vendor.setdefault(vendor, {}).setdefault(name, []).append((path, doc))
    vendors_sorted = sorted(by_vendor.keys(), key=str.lower)
    total_devices = sum(len(names) for names in by_vendor.values())

    out = []
    out.append("<!-- AUTO-GENERATED by scripts/regen_supported_devices.py — do not edit by hand. -->")
    out.append("<!-- Single source of truth: the JSONs under devices/. CI enforces this file is in sync. -->")
    out.append("")
    out.append("# Supported devices")
    out.append("")
    out.append(f"**{total_devices} devices** across **{len(by_vendor)} vendors** — shipped as **{total_files} driver files** (a device may have several variants: per firmware, per source).")
    out.append("")
    out.append(f"- **✅ hardware-verified** — {by_status['✅']} (tested on the physical device — scope varies per driver, see `_sources` in each JSON for what was exercised)")
    out.append(f"- **📘 vendor-doc derived** — {by_status['📘']} (from a manufacturer programmer's reference / official MIDI spec — bytes match the spec, still beta until tested)")
    out.append(f"- **📡 OSC-API (software targets)** — {by_status['📡']} (vendor-OSC or third-party OSC shim like AbletonOSC — DAWs and live coding environments)")
    out.append(f"- **🎛️ electra-preset derived** — {by_status['🎛️']} (from a community Electra One preset — covers SysEx + CC/NRPN, but reflects the preset author's editorial choices)")
    out.append(f"- **📦 pencilresearch derived** — {by_status['📦']} (community canonical CSVs — CC/NRPN only, no SysEx)")
    out.append(f"- **🚧 WIP** — {by_status['🚧']}")
    out.append(f"- **📝 stub** — {by_status['📝']}")
    out.append("")
    out.append("**Multiple variants per device are allowed and encouraged** — do NOT fuse sources silently. Name variants `<device>.<source-tier>.fw-<version>.json` (e.g. `tempera.vendor.fw-2.2.json`, `tempera.electra-community.fw-unknown.json`). See [`CONTRIBUTING.md`](../CONTRIBUTING.md).")
    out.append("")
    out.append("---")
    out.append("")

    # TOC
    out.append("## Vendors")
    out.append("")
    for v in vendors_sorted:
        anchor = v.lower().replace(" ", "-").replace(".", "")
        n_devices = len(by_vendor[v])
        n_files = sum(len(variants) for variants in by_vendor[v].values())
        if n_files == n_devices:
            out.append(f"- [{v}](#{anchor}) ({n_devices})")
        else:
            out.append(f"- [{v}](#{anchor}) ({n_devices} devices, {n_files} files)")
    out.append("")
    out.append("---")
    out.append("")

    # Vendor sections
    for v in vendors_sorted:
        out.append(f"## {v}")
        out.append("")
        for name in sorted(by_vendor[v].keys(), key=str.lower):
            variants = by_vendor[v][name]
            status = best_status(variants)

            # Pick a "primary" variant for top-level summary: highest tier, then
            # firmware-tagged (non-empty firmware) over firmware-unknown.
            def variant_sort_key(pd):
                p, d = pd
                s = classify_status(d, p)[0]
                fw = firmware_of(d)
                return (STATUS_PRIORITY.get(s, 0), 1 if fw and fw != "unknown" else 0)

            primary_path, primary_doc = max(variants, key=variant_sort_key)
            prefix = primary_doc.get("device", {}).get("osc_prefix", "—")

            out.append(f"### {status} {name}")
            out.append("")
            out.append(f"- **OSC prefix**: `{prefix}`")
            out.append(f"- **Variants**: {len(variants)}")
            out.append("")

            # One sub-block per variant, ordered by the same sort key (best first).
            for path, doc in sorted(variants, key=variant_sort_key, reverse=True):
                v_status, _ = classify_status(doc, path)
                rev = doc.get("device", {}).get("revision", "")
                fw = firmware_of(doc)
                cov = summarise_coverage(doc)
                src_label, src_url = source_summary(doc)
                rel = path.relative_to(REPO_ROOT).as_posix()
                limitations = doc.get("_limitations")

                header = f"#### {v_status} [`{path.name}`](../{rel})"
                if fw:
                    header += f" — firmware `{fw}`"
                out.append(header)
                out.append("")
                if rev:
                    out.append(f"- **Revision**: {rev}")
                if src_url:
                    out.append(f"- **Source**: [{src_label}]({src_url})")
                else:
                    out.append(f"- **Source**: {src_label}")
                out.append(f"- **Coverage**: {cov}")
                if limitations:
                    short = limitations.split(".")[0] + "."
                    out.append(f"- **Limitations**: {short}")
                out.append("")
    return "\n".join(out).rstrip() + "\n"


def main():
    devices = load_devices()
    content = render(devices)
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(content)
    print(f"wrote {OUTPUT.relative_to(REPO_ROOT)} — {len(devices)} devices")


if __name__ == "__main__":
    main()
