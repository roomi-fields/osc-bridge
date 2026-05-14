#!/usr/bin/env python3
"""Re-vendor a subset of `devices/electra-community/` using high-confidence
model→vendor heuristics. Moves the JSON to `devices/<vendor-slug>/` and
rewrites `device.vendor`. Files that stay are genuine platform demos,
MIDI-utility presets, or names we can't confidently attribute.

Usage:
    python3 scripts/rebucket_electra_community.py [--dry-run]
"""
from __future__ import annotations
import json
import re
import shutil
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
COMMUNITY = REPO_ROOT / "devices" / "electra-community"
DEVICES = REPO_ROOT / "devices"


def vslug(v: str) -> str:
    s = v.lower().strip()
    s = re.sub(r"[^a-z0-9]+", "-", s).strip("-")
    return s or "unknown"


# Ordered most-specific first. Each (pattern, vendor). Pattern matched as a
# word against the filename (underscores = word boundaries) OR against the
# device.name (lowercased, with non-alnum collapsed to spaces).
RULES: list[tuple[str, str]] = [
    # Explicit vendor prefixes in filename
    (r"\bakai\b", "Akai"),
    (r"\balesis\b", "Alesis"),
    (r"\bableton\b|\babl 3\b", "Ableton"),
    (r"\bbitwig\b", "Bitwig"),
    (r"\belka\b", "Elka"),
    (r"\bemu\b", "E-MU"),
    (r"\bboss\b|\brv 70\b|\brv70\b", "BOSS"),
    (r"\bcasio\b|\bvz 8m\b|\bvz v\b", "Casio"),
    (r"\bcrumar\b|\bbit01\b", "Crumar"),
    (r"\bcheetah\b|\bms6\b", "Cheetah"),
    (r"\bferrofish\b", "Ferrofish"),
    (r"\blexicon\b|\bpcm 80\b|\bpcm 96\b|\blxp 5\b|\blxp5\b", "Lexicon"),
    (r"\bmotu\b", "MOTU"),
    (r"\bmadrona\b|\baalto\b|\bkaivo\b", "Madrona Labs"),
    (r"\btal \b|\btal$", "TAL Software"),
    (r"\brepro \b|\brepro$|\bdiva\b|\bzebra\b", "u-he"),
    (r"\bomnisphere\b", "Spectrasonics"),
    (r"\bserum\b", "Xfer Records"),
    (r"\bsylenth1?\b", "LennarDigital"),
    (r"\bsynth1\b", "Ichiro Toda"),
    (r"\bmassive( x)?\b", "Native Instruments"),
    (r"\bpigments\b|\bmicrofreak\b|\bminifreak\b", "Arturia"),
    (r"\btx 6\b|\btx6\b", "Teenage Engineering"),
    (r"\bdx7\b|\bfs1r\b|\bplg150\b", "Yamaha"),
    (r"\bjuno \b|\bjuno$|\bjx \b|\bjx$|\bjv \b|\bjv$|\bmks \b|\bd 50\b|\bd50\b|\bjupiter\b", "Roland"),
    (r"\bprophet( 5| 6| 10| 12| rev| v|$)|\bpro 800\b|\bsix trak\b|\brev ?2\b", "Sequential"),
    (r"\bsub ?37\b|\bsubsequent\b|\bminimoog\b|\bmatriarch\b|\bgrandmother\b|\bmodel 72\b|\bmodel 82\b|\bmodel 84\b|\bminiverse\b", "Moog"),
    (r"\bob 6\b|\bob x\b|\bob xa\b|\bob matrix\b|\bub xa\b|\bobn mtet\b", "Oberheim"),
    (r"\bnymphes\b|\bmedusa dreadbox\b", "Dreadbox"),
    (r"\bshruthi\b|\bambika\b|\bmutable\b", "Mutable Instruments"),
    (r"\bdeluge\b", "Synthstrom Audible"),
    (r"\bmedusa\b|\btempera\b", "Polyend"),
    (r"\bdigitone\b|\bsyntakt\b|\boctatrack\b|\bocta digi\b|\banalog 4mki\b|\banalog drive pedal\b", "Elektron"),
    (r"\bminilogue\b|\bkronos\b|\bminikorg\b|\bpoly ?61\b|\bcs30l\b|\bpolysixex\b", "Korg"),
    (r"\bdeepmind\b|\bjt 4000\b", "Behringer"),
    (r"\bblofeld\b|\biridium\b|\bmicroq\b|\bxt sysex\b", "Waldorf"),
    (r"\bcontinuum\b|\bslim continuum\b", "Haken Audio"),
    (r"\bequator\b", "ROLI"),
    (r"\bhydrasynth\b", "ASM"),
    (r"\bbass station\b|\ba station\b|\bsupernova\b|\bsuper bass station\b|\bsuper 8\b", "Novation"),
    (r"\bh3000\b", "Eventide"),
    (r"\btc d two\b|\bfinalizer\b", "TC Electronic"),
    (r"\bdl 8000r\b", "Dreadbox"),  # DL-8000R is actually Marshall; leave it — skip; remove this
    (r"\bchase bliss\b|\bcba preamp\b", "Chase Bliss"),
    (r"\bredpanda\b", "Red Panda Lab"),
    (r"\bgfi \b|\bspecular tempus\b", "GFI System"),
    (r"\boto \b", "OTO Machines"),
    (r"\bstrymon\b", "Strymon"),
    (r"\bmodal \b|\bmodal$", "Modal Electronics"),
    (r"\bdrumazon\b|\bnepheton\b", "D16 Group"),
    (r"\bkasser\b|\bdafm\b", "Kasser Synths"),
    (r"\bfuture impact\b", "Panda-Audio"),
    (r"\blzx\b", "LZX Industries"),
    (r"\bnorns\b", "Monome"),
    (r"\bvcv rack\b", "VCV Rack"),
    (r"\bdirtywave\b|\bwbhfr m8\b|\bm8 model02\b", "Dirtywave"),
    (r"\btwin toraiz\b|\btoraiz as\b", "Pioneer DJ"),
    (r"\bdeckard\b", "Black Corporation"),
    (r"\bthe legend\b|\bthe prince\b|\bdune 3\b", "Synapse Audio"),
    (r"\btotalmix\b", "RME"),
    (r"\bwaves codex\b", "Waves"),
    (r"\buad \b|\bua polymax\b", "Universal Audio"),
    (r"\bvalhallavintag\b|\bvalhalla\b", "Valhalla DSP"),
    (r"\bcamelcrusher\b", "Camel Audio"),
    (r"\bmam \b", "MAM"),
    (r"\bmarion\b|\bmsr marion\b", "Marion Systems"),
    (r"\bndlr\b", "Conductive Labs"),
    (r"\bliven\b", "Sonicware"),
    (r"\boxi \b", "OXI Instruments"),
    (r"\bsquarp\b|\bpyramidi\b", "Squarp"),
    (r"\bgsi \b|\bvb3\b", "GSi"),
    (r"\bbohm\b", "Böhm"),
    (r"\bplinky\b", "Plinky Synth"),
    (r"\bknif \b|\bknifonium\b", "Knif Audio"),
    (r"\bjomox\b", "Jomox"),
    (r"\bflame \b|\bqmcv\b", "Flame"),
    (r"\bspektro \b", "Spektro Audio"),
    (r"\bsuonobuono\b", "Suonobuono"),
    (r"\blooperlative\b", "Looperlative"),
    (r"\bse 3x\b|\bse omega\b", "Studio Electronics"),
    (r"\bsci \b", "Sequential Circuits"),
    (r"\bdrumkid\b", "Bastl Instruments"),
    (r"\bds thorn\b", "Dmitry Sches"),
    (r"\bspire\b", "Reveal Sound"),
    (r"\bmemorymode\b", "Cherry Audio"),
    (r"\bminerva\b", "Kiwi Technics"),
    (r"\bof gforce\b|\bgforce\b", "GForce Software"),
    (r"\brhodes\b|\bchroma polaris\b|\bchroma console\b", "Rhodes"),
    (r"\bmoonwind\b", "Moonwind"),
    (r"\btauntek\b", "Tauntek"),
    (r"\bgenericmidi\b|\bgeneric midi\b", "Generic MIDI"),
]

# Remove the accidental bad rule (DL-8000R is Marshall Time Modulator etc.,
# not Dreadbox — drop it to avoid mis-assigning).
RULES = [(p, v) for (p, v) in RULES if v != "Dreadbox" or "nymphes" in p or "dreadbox" in p]


def match_vendor(filename_stem: str, device_name: str) -> str | None:
    """Try every rule against the filename stem; return the first match's vendor.
    Underscores/hyphens/spaces all count as word boundaries (Python's `\\b`
    treats `_` as a word char, so we normalise first)."""
    def norm(s: str) -> str:
        return re.sub(r"[^a-z0-9]+", " ", (s or "").lower()).strip()
    hay = norm(filename_stem)
    hay2 = norm(device_name)
    for pat, vendor in RULES:
        if re.search(pat, hay) or re.search(pat, hay2):
            return vendor
    return None


def main():
    dry = "--dry-run" in sys.argv
    if not COMMUNITY.is_dir():
        print(f"no {COMMUNITY}")
        return
    moves: list[tuple[Path, Path, str]] = []
    for p in sorted(COMMUNITY.glob("*.json")):
        try:
            d = json.loads(p.read_text())
        except Exception:
            continue
        dev = d.get("device") or {}
        dname = dev.get("name") or ""
        vendor = match_vendor(p.stem, dname)
        if not vendor:
            continue
        dest_dir = DEVICES / vslug(vendor)
        dest = dest_dir / p.name
        if dest.exists():
            # Name collision — keep in community to avoid silent overwrite
            print(f"  SKIP (collision) {p.name} → {dest}")
            continue
        moves.append((p, dest, vendor))

    by_vendor: dict[str, int] = {}
    for _, _, v in moves:
        by_vendor[v] = by_vendor.get(v, 0) + 1
    print(f"will move {len(moves)} / {len(list(COMMUNITY.glob('*.json')))} community files")
    for v, n in sorted(by_vendor.items(), key=lambda x: (-x[1], x[0])):
        print(f"  {v:30s} {n}")
    if dry:
        return
    for src, dest, vendor in moves:
        dest.parent.mkdir(parents=True, exist_ok=True)
        d = json.loads(src.read_text())
        d.setdefault("device", {})["vendor"] = vendor
        dest.write_text(json.dumps(d, indent=2) + "\n")
        src.unlink()
    print(f"\nmoved {len(moves)} files. Remaining in electra-community: "
          f"{len(list(COMMUNITY.glob('*.json')))}")


if __name__ == "__main__":
    main()
