#!/usr/bin/env python3
"""Drivers for the synths KnobKraft Orm can read and write sounds for, when
the catalogue has no other driver for them.

KnobKraft Orm (https://github.com/christofmuc/KnobKraft-orm, AGPL-3.0) ships
one "adaptation" per synth: how to find it, ask for its sound, recognise and
name the answer. SynthHub's librarian runs those adaptations to take sounds
from a synth and load them back; a shared sound belongs to a catalogue
driver, so each synth needs one. These drivers say only what the device is
and where its sound exchange comes from: no control is mapped.

Usage:
    python3 scripts/import_knobkraft.py <KnobKraft-orm checkout>

Writes devices/<vendor>/<slug>.knobkraft.fw-<firmware>.json for each entry
of TARGETS — the adaptations no other source covers. A synth that another
source already describes keeps that driver and is not listed here.
"""
from __future__ import annotations

import contextlib
import datetime
import importlib.util
import io
import json
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEVICES_DIR = REPO_ROOT / "devices"
LICENSE = "AGPL-3.0"
ATTRIBUTION = "KnobKraft Orm (https://github.com/christofmuc/KnobKraft-orm) by Christof Ruch and contributors"

# adaptation file stem -> (vendor folder, vendor, model name, slug, firmware)
TARGETS = {
    "Akai_AX80": ("akai", "Akai", "AX80", "ax80", "tauntek"),
    "BC_Kijimi": ("black-corporation", "Black Corporation", "Kijimi", "kijimi", "unknown"),
    "Casio CZ-101": ("casio", "Casio", "CZ-101 / CZ-1000", "cz_101", "unknown"),
    "DSI Pro 2": ("dave-smith-instruments", "Dave Smith Instruments", "Pro 2", "pro_2", "unknown"),
    "DSI_Tempest": ("dave-smith-instruments", "Dave Smith Instruments", "Tempest", "tempest", "unknown"),
    "ElectraOne": ("electra-one", "Electra One", "Electra One", "electra_one", "unknown"),
    "Ensoniq_Mirage_Soundprocess": ("ensoniq", "Ensoniq", "Mirage", "mirage", "soundprocess"),
    "Ensoniq_VFX": ("ensoniq", "Ensoniq", "VFX", "vfx", "unknown"),
    "JB_Solaris": ("john-bowen", "John Bowen Synth Design", "Solaris", "solaris", "unknown"),
    "KawaiK3": ("kawai", "Kawai", "K3", "k3", "unknown"),
    "KorgDW6000": ("korg", "Korg", "DW-6000", "dw_6000", "unknown"),
    "Line_6_POD_Series": ("line-6", "Line 6", "POD", "pod", "unknown"),
    "OberheimOB8": ("oberheim", "Oberheim", "OB-8", "ob_8", "unknown"),
    "OberheimOBX": ("oberheim", "Oberheim", "OB-X", "ob_x", "encore"),
    "QuasimidiCyber6": ("quasimidi", "Quasimidi", "Cyber-6", "cyber_6", "unknown"),
    "Roland_JV80": ("roland", "Roland", "JV-80", "jv_80", "unknown"),
    "Roland_JunoDS": ("roland", "Roland", "Juno-DS", "juno_ds", "unknown"),
    "Roland_TD-07": ("roland", "Roland", "TD-07", "td_07", "unknown"),
    "Roland_U20_U220": ("roland", "Roland", "U-20 / U-220", "u_20", "unknown"),
    "Roland_XV3080": ("roland", "Roland", "XV-3080", "xv_3080", "unknown"),
    "Sequential_Trigon6": ("sequential", "Sequential", "Trigon-6", "trigon_6", "unknown"),
    "Yamaha-YC-Series": ("yamaha", "Yamaha", "YC61 / YC73 / YC88", "yc", "unknown"),
}


def load(adaptations: Path, stem: str):
    sys.path.insert(0, str(adaptations))
    name = "adaptation_" + "".join(c if c.isalnum() else "_" for c in stem)
    spec = importlib.util.spec_from_file_location(name, adaptations / f"{stem}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    with contextlib.redirect_stdout(io.StringIO()):
        spec.loader.exec_module(module)
    return module


# Synths whose sounds may come under another maker's id than their requests:
# the OB-8 answers as Oberheim, or as Encore Electronics with its MIDI kit.
NO_MAKER_CHECK = {"OberheimOB8"}


def manufacturer_id(m) -> list[int]:
    """The maker's SysEx id, from the messages the adaptation sends the synth."""
    asks = []
    for f, args in (("createEditBufferRequest", (0,)), ("createProgramDumpRequest", (0, 0)), ("createDeviceDetectMessage", (0,))):
        if callable(getattr(m, f, None)):
            with contextlib.suppress(Exception), contextlib.redirect_stdout(io.StringIO()):
                asks.append(list(getattr(m, f)(*args)))
    for data in asks:
        for i, b in enumerate(data):
            if b == 0xF0 and i + 1 < len(data) and data[i + 1] < 0x7E:
                return data[i + 1:i + 4] if data[i + 1] == 0 else [data[i + 1]]
    return []


def driver(m, stem: str, target: tuple, commit: str) -> dict:
    folder, vendor, model, slug, firmware = target
    can = [w for f, w in (("createEditBufferRequest", "take the sound being played"),
                          ("createProgramDumpRequest", "take a stored program"),
                          ("convertToEditBuffer", "load a sound back"),
                          ("nameFromDump", "read the sound's name")) if callable(getattr(m, f, None))]
    return {
        "_sources": [{
            "type": "knobkraft",
            "url": f"https://github.com/christofmuc/KnobKraft-orm/blob/{commit[:7]}/adaptations/{stem}.py".replace(" ", "%20"),
            "imported_at": datetime.date.today().isoformat(),
            "firmware": firmware,
            "license": LICENSE,
            "attribution": ATTRIBUTION,
            "commit": commit[:7],
            "adaptation": stem,
        }],
        "_limitations": "Sound exchange only: SynthHub's librarian can " + ", ".join(can) +
            " with the KnobKraft Orm adaptation named in _sources. No control is mapped (no CC, NRPN or SysEx parameter): "
            "to play or tweak the device, add them from its MIDI implementation chart.",
        "device": {
            "name": model,
            "vendor": vendor,
            "revision": f"per KnobKraft Orm adaptation {m.name()!r}",
            "osc_prefix": "/" + slug,
            "manufacturer_id": [] if stem in NO_MAKER_CHECK else manufacturer_id(m),
            "device_id": [],
            "rate_limit_hz": 0,
        },
        "sysex": {"header": [240], "footer": [247]},
        "commands": [],
        "replies": [],
    }


def main() -> int:
    checkout = Path(sys.argv[1]).resolve()
    adaptations = checkout / "adaptations"
    commit = subprocess.run(["git", "-C", str(checkout), "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    written = 0
    for stem, target in TARGETS.items():
        m = load(adaptations, stem)
        folder, _, _, slug, firmware = target
        path = DEVICES_DIR / folder / f"{slug}.knobkraft.fw-{firmware}.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        doc = driver(m, stem, target, commit)
        if path.exists():  # keep the first import date when nothing else changed
            old = json.loads(path.read_text())
            if {**old["_sources"][0], "imported_at": ""} == {**doc["_sources"][0], "imported_at": ""}:
                doc["_sources"][0]["imported_at"] = old["_sources"][0]["imported_at"]
        path.write_text(json.dumps(doc, indent=2, ensure_ascii=False) + "\n")
        written += 1
        print(f"{path.relative_to(REPO_ROOT)}  manufacturer {bytes(doc['device']['manufacturer_id']).hex(' ') or '-'}")
    print(f"{written} driver(s) written from KnobKraft Orm {commit[:7]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
