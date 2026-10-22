#!/usr/bin/env python3
"""
Import a pencilresearch/midi canonical CSV into an osc-bridge device JSON.

Usage:
    python3 scripts/import_pencilresearch.py <repo>/<Vendor>/<Device>.csv [--out path]
    python3 scripts/import_pencilresearch.py --update devices/<vendor>/<slug>.json --from <CSV>
    python3 scripts/import_pencilresearch.py --sync <repo-path> [--dry-run]

The CSV is expected to come from https://github.com/pencilresearch/midi
(MIDI Guide, https://midi.guide). That dataset is licensed CC BY-SA 4.0:
every source entry written here carries the licence and the attribution.

The generated JSON is `📄 doc-derived`, CC/NRPN only (no SysEx), plus the
device's note triggers when a sibling `<Device>.triggers.csv` exists. A
`_limitations` field documents the gap explicitly.

`--sync` reconciles the whole catalogue with a fresh clone, following the
one-source-per-file rule:
  - a file whose only source is pencilresearch is refreshed in place
    (CC/NRPN and triggers replaced, every other block preserved), also when
    the CSV was renamed upstream;
  - a file mixing several sources keeps its data; only the licence and
    attribution of its pencilresearch entry are filled in;
  - a device not yet imported from pencilresearch gets its own file,
    `<slug>.pencilresearch.fw-unknown.json`, next to any other variant.

Re-import behaviour (`--update`): the CC/NRPN entries are replaced with the
fresh ones; everything else in the JSON (commands, sysex header/footer,
replies, midi_out, custom `_notes`) is preserved verbatim. The
`_sources[0].commit` and `_sources[0].imported_at` are refreshed.
"""
import argparse
import csv
import json
import re
import subprocess
import sys
from datetime import date
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PENCIL_REPO_URL = "https://github.com/pencilresearch/midi"
LICENSE = "CC-BY-SA-4.0"
ATTRIBUTION = "MIDI Guide (https://midi.guide) by Pencil Research and contributors"


def slug(s: str) -> str:
    s = s.lower().strip()
    s = re.sub(r"\(nrpn\)", "", s)
    s = re.sub(r"[^a-z0-9]+", "_", s).strip("_")
    return s


def vendor_slug(s: str) -> str:
    s = s.lower().strip()
    s = re.sub(r"[^a-z0-9]+", "-", s).strip("-")
    return s


def parse_csv(path: Path):
    vendor = None
    device = None
    groups = {}
    order = []
    with path.open(encoding="utf-8-sig") as f:
        for row in csv.DictReader(f):
            vendor = vendor or row.get("manufacturer", "").strip()
            device = device or row.get("device", "").strip()
            sect_raw = row.get("section", "").strip()
            pname_raw = row.get("parameter_name", "").strip()
            if not pname_raw:
                continue
            sect = slug(sect_raw)
            pname_stripped = pname_raw
            if sect_raw and pname_stripped.lower().startswith(sect_raw.lower() + " "):
                pname_stripped = pname_stripped[len(sect_raw) + 1:]
            pname = slug(pname_stripped) or slug(pname_raw)
            if not pname:
                continue
            key = (sect, pname)
            if key not in groups:
                groups[key] = {"section_label": sect_raw}
                order.append(key)
            g = groups[key]
            if row.get("cc_msb", "").strip():
                try:
                    g["cc_msb"] = int(row["cc_msb"])
                    if row.get("cc_lsb", "").strip():
                        g["cc_lsb"] = int(row["cc_lsb"])
                    g["cc_min"] = int(row.get("cc_min_value") or 0)
                    g["cc_max"] = int(row.get("cc_max_value") or 127)
                except ValueError:
                    pass
            if row.get("nrpn_msb", "").strip() and row.get("nrpn_lsb", "").strip():
                try:
                    g["nrpn_msb"] = int(row["nrpn_msb"])
                    g["nrpn_lsb"] = int(row["nrpn_lsb"])
                    g["nrpn_min"] = int(row.get("nrpn_min_value") or 0)
                    g["nrpn_max"] = int(row.get("nrpn_max_value") or 16383)
                except ValueError:
                    pass
            if row.get("orientation", "").strip():
                g["orientation"] = row["orientation"].strip()
            if row.get("usage", "").strip():
                g["usage"] = row["usage"].strip()
    entries = []
    for key in order:
        g = groups[key]
        sect, pname = key
        e = {"osc": (f"/{sect}/{pname}" if sect else f"/{pname}")}
        if "cc_msb" in g:
            e["cc"] = g["cc_msb"]
            if "cc_lsb" in g:
                e["cc_lsb"] = g["cc_lsb"]; e["range"] = [0, 16383]
            else:
                e["range"] = [g.get("cc_min", 0), g.get("cc_max", 127)]
        if "nrpn_msb" in g:
            e["nrpn_msb"] = g["nrpn_msb"]; e["nrpn_lsb"] = g["nrpn_lsb"]
            if "cc_msb" not in g:
                e["range"] = [g.get("nrpn_min", 0), g.get("nrpn_max", 16383)]
        e["orientation"] = g.get("orientation", "0-based") or "0-based"
        if g.get("section_label"):
            e["section"] = g["section_label"]
        if g.get("usage"):
            e["_usage"] = g["usage"][:150]
        entries.append(e)
    return vendor or path.parent.name, device or path.stem, entries


def get_git_commit(repo_path: Path) -> str | None:
    try:
        r = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo_path,
                           capture_output=True, text=True, check=True)
        return r.stdout.strip()[:7]
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def pencilresearch_blob_url(commit: str, relative: str) -> str:
    return f"{PENCIL_REPO_URL}/blob/{commit}/{relative}"


def make_source(commit, csv_rel_path):
    source = {
        "type": "pencilresearch",
        "url": pencilresearch_blob_url(commit, csv_rel_path) if commit else f"{PENCIL_REPO_URL}/{csv_rel_path}",
        "imported_at": date.today().isoformat(),
        "firmware": "unknown",
        "license": LICENSE,
        "attribution": ATTRIBUTION,
    }
    if commit:
        source["commit"] = commit
    return source


def parse_triggers(path: Path):
    """Read a `<Device>.triggers.csv` into `triggers.entries`."""
    rows = []
    with path.open(encoding="utf-8-sig") as f:
        for row in csv.DictReader(f):
            sound = (row.get("sound_name") or "").strip()
            lo, hi = (row.get("note_number_min_value") or "").strip(), (row.get("note_number_max_value") or "").strip()
            if not sound or not lo.isdigit() or not hi.isdigit():
                continue
            rows.append(row | {"sound_name": sound, "_lo": int(lo), "_hi": int(hi)})
    sound_slugs = [slug(r["sound_name"]) for r in rows]
    unique = len(set(sound_slugs)) == len(sound_slugs)
    entries, seen = [], {}
    for r, ss in zip(rows, sound_slugs):
        section = (r.get("section") or "").strip()
        osc = f"/drum/{ss}" if unique or not section else f"/drum/{slug(section)}/{ss}"
        seen[osc] = seen.get(osc, 0) + 1
        if seen[osc] > 1:
            osc = f"{osc}_{seen[osc]}"
        e = {"osc": osc, "sound": r["sound_name"]}
        if section:
            e["section"] = section
        ch = (r.get("midi_channel") or "").strip().lower()
        if ch.isdigit() and 1 <= int(ch) <= 16:
            e["channel"] = int(ch) - 1
        elif ch in ("basic", "omni", "auto"):
            e["channel_mode"] = ch
        e["note_range"] = [r["_lo"], r["_hi"]]
        default = (r.get("note_number_default_value") or "").strip()
        if default.isdigit():
            e["note"] = int(default)
        for col, key in (("velocity_usage", "velocity_usage"), ("sound_description", "comments"), ("comments", "comments")):
            v = (r.get(col) or "").strip()
            if v:
                e[key] = f"{e[key]} {v}" if key in e else v
        entries.append(e)
    return entries


CSV_KEYS = {"osc", "cc", "cc_lsb", "range", "nrpn_msb", "nrpn_lsb", "orientation", "section", "_usage"}


def structural_expects(entry):
    """The catalogue's structural `expects` rule: binary [0,1] is a switch, a
    small NRPN stepper (no CC, 2..31 span) is discrete; everything else stays
    continuous (the default, left implicit)."""
    lo, hi = entry.get("range", [0, 127])
    if [lo, hi] == [0, 1]:
        return "switch"
    if entry.get("cc") is None and 2 <= hi - lo <= 31:
        return "discrete"
    return None


def annotate(entries, previous=()):
    """Carry over hand-added fields (`expects`, `transform`, …) from the
    previous entry with the same OSC path; apply the structural rule to the
    rest."""
    kept = {e["osc"]: {k: v for k, v in e.items() if k not in CSV_KEYS} for e in previous}
    for e in entries:
        extra = kept.get(e["osc"], {})
        e.update(extra)
        if "expects" not in e:
            nature = structural_expects(e)
            if nature:
                e["expects"] = nature
    return entries


def coverage(entries, triggers):
    cov = {
        "entries_total": len(entries),
        "cc_entries": sum(1 for e in entries if "cc" in e),
        "nrpn_entries": sum(1 for e in entries if "nrpn_msb" in e),
        "fourteen_bit_cc_pairs": sum(1 for e in entries if "cc_lsb" in e),
    }
    if triggers:
        cov["triggers"] = len(triggers)
    return cov


def build_device(vendor, device_name, entries, csv_rel_path, commit, triggers=None):
    prefix = "/" + slug(device_name)
    annotate(entries)
    source = make_source(commit, csv_rel_path)
    doc = {
        "_sources": [source],
        "_limitations": (
            "CC/NRPN only. SysEx (preset management, display control, bulk dump/load, "
            "vendor-proprietary opcodes) is NOT covered by pencilresearch. To extend "
            "this device with SysEx, sniff the vendor editor, add `sysex.header/footer` "
            "+ `commands` blocks, and use `update_device.md` PR template."
        ),
        "_coverage": coverage(entries, triggers),
        "device": {
            "name": device_name,
            "vendor": vendor,
            "revision": "per pencilresearch canonical CSV",
            "osc_prefix": prefix,
            "manufacturer_id": [],
            "device_id": [],
            "rate_limit_hz": 1000,
        },
        "sysex": {"header": [], "footer": []},
        "commands": [],
        "cc_params": {"channel": 0, "entries": entries},
        "midi_out": {"default_channel": 0, "note_offset": 0},
        "midi_in": {
            "note_on":   "/note/on {note} {velocity} {channel}",
            "note_off":  "/note/off {note} {velocity} {channel}",
            "cc":        "/cc/{num} {value} {channel}",
            "pitchbend": "/pitchbend {value_u14} {channel}",
            "aftertouch":"/aftertouch {value} {channel}",
        },
        "replies": [],
    }
    if triggers:
        doc["triggers"] = {"entries": triggers}
    return doc


def default_out_path(vendor, device_name):
    return REPO_ROOT / "devices" / vendor_slug(vendor) / f"{slug(device_name)}.json"


def variant_out_path(vendor, device_name):
    return REPO_ROOT / "devices" / vendor_slug(vendor) / f"{slug(device_name)}.pencilresearch.fw-unknown.json"


def triggers_for(csv_path: Path):
    t = csv_path.with_name(csv_path.stem + ".triggers.csv")
    return parse_triggers(t) if t.is_file() else []


def rel_to(path: Path, repo: Path | None) -> str:
    try:
        return path.relative_to(repo).as_posix() if repo else path.name
    except ValueError:
        return path.name


def import_one(csv_path: Path, pencil_repo: Path | None, out_path: Path | None, min_entries: int = 3):
    vendor, device_name, entries = parse_csv(csv_path)
    triggers = triggers_for(csv_path)
    if len(entries) < min_entries and not triggers:
        return None, f"skip (skeleton, {len(entries)} entries): {csv_path.name}"
    commit = get_git_commit(pencil_repo) if pencil_repo else None
    doc = build_device(vendor, device_name, entries, rel_to(csv_path, pencil_repo), commit, triggers)
    out = out_path or default_out_path(vendor, device_name)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, indent=2) + "\n")
    return out, f"{out.relative_to(REPO_ROOT)} — {len(entries)} entries, {len(triggers)} triggers"


def update_existing(json_path: Path, csv_path: Path, pencil_repo: Path | None):
    """Re-import CC/NRPN and triggers while preserving every other block."""
    existing = json.loads(json_path.read_text())
    _, _, entries = parse_csv(csv_path)
    triggers = triggers_for(csv_path)
    commit = get_git_commit(pencil_repo) if pencil_repo else None
    table = existing.setdefault("cc_params", {"channel": 0, "entries": []})
    table["entries"] = annotate(entries, table.get("entries", []))
    if triggers:
        existing["triggers"] = {"entries": triggers}
    else:
        existing.pop("triggers", None)
    sources = [s for s in existing.get("_sources", []) if s.get("type") != "pencilresearch"]
    existing["_sources"] = [make_source(commit, rel_to(csv_path, pencil_repo))] + sources
    existing["_coverage"] = coverage(entries, triggers)
    json_path.write_text(json.dumps(existing, indent=2) + "\n")
    return f"{json_path.relative_to(REPO_ROOT)} — refreshed {len(entries)} entries, {len(triggers)} triggers"


def license_only(json_path: Path):
    """Fill licence + attribution on the pencilresearch entry of a mixed file."""
    doc = json.loads(json_path.read_text())
    changed = False
    for src in doc.get("_sources", []):
        if src.get("type") == "pencilresearch" and (src.get("license"), src.get("attribution")) != (LICENSE, ATTRIBUTION):
            src["license"], src["attribution"] = LICENSE, ATTRIBUTION
            changed = True
    if changed:
        json_path.write_text(json.dumps(doc, indent=2) + "\n")
    return changed


def norm(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "", s.lower())


def pencil_csv_of(doc) -> str | None:
    from urllib.parse import unquote
    for src in doc.get("_sources", []):
        if src.get("type") == "pencilresearch":
            m = re.search(r"pencilresearch/midi/(?:blob/[0-9a-f]+/)?(.+\.csv)$", src.get("url", ""))
            return unquote(m.group(1)) if m else None
    return None


def sync(repo: Path, min_entries: int = 3, dry_run: bool = False):
    repo = repo.resolve()
    upstream = {p.relative_to(repo).as_posix(): p for p in repo.rglob("*.csv")
                if p.parent != repo and not p.name.endswith(".triggers.csv")}
    # Devices documented only by their note triggers have no main CSV.
    for t in repo.rglob("*.triggers.csv"):
        main = t.with_name(t.name.replace(".triggers.csv", ".csv"))
        if not main.exists():
            upstream.setdefault(main.relative_to(repo).as_posix(), main)
    by_norm = {}
    for rel in upstream:
        vendor_dir, _, name = rel.rpartition("/")
        by_norm[(norm(vendor_dir), norm(name[:-4]))] = rel

    refreshed, licensed, created, skipped, orphaned = [], [], [], [], []
    claimed = set()
    for f in sorted((REPO_ROOT / "devices").rglob("*.json")):
        try:
            doc = json.loads(f.read_text())
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
        rel = pencil_csv_of(doc)
        if rel is None:
            continue
        if rel not in upstream:
            vendor_dir, _, name = rel.rpartition("/")
            rel = by_norm.get((norm(vendor_dir), norm(name[:-4])), rel)
        claimed.add(rel)
        if len(doc.get("_sources", [])) > 1:
            if dry_run or license_only(f):
                licensed.append(f.relative_to(REPO_ROOT).as_posix())
            continue
        if rel not in upstream:
            orphaned.append(f.relative_to(REPO_ROOT).as_posix())
            continue
        refreshed.append(f.relative_to(REPO_ROOT).as_posix() if dry_run
                         else update_existing(f, upstream[rel], repo))

    for rel in sorted(set(upstream) - claimed):
        csv_path = upstream[rel]
        if csv_path.exists():
            vendor, device_name, entries = parse_csv(csv_path)
        else:
            rows = list(csv.DictReader(csv_path.with_name(csv_path.stem + ".triggers.csv").open(encoding="utf-8-sig")))
            vendor, device_name, entries = rows[0]["manufacturer"], rows[0]["device"], []
        triggers = triggers_for(csv_path)
        if len(entries) < min_entries and not triggers:
            skipped.append(rel)
            continue
        out = variant_out_path(vendor, device_name)
        if not dry_run:
            out.parent.mkdir(parents=True, exist_ok=True)
            doc = build_device(vendor, device_name, entries, rel, get_git_commit(repo), triggers)
            out.write_text(json.dumps(doc, indent=2) + "\n")
        created.append(f"{out.relative_to(REPO_ROOT)} — {len(entries)} entries, {len(triggers)} triggers")

    for title, items in (("Refreshed", refreshed), ("Licence filled (mixed sources, data untouched)", licensed),
                         ("Created", created), ("Skipped (fewer than %d entries)" % min_entries, skipped),
                         ("Kept, CSV gone upstream", orphaned)):
        print(f"\n=== {title}: {len(items)} ===")
        for m in items:
            print("  " + m)


def main():
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--update", metavar="JSON",
                   help="update an existing device JSON with a fresh CSV")
    g.add_argument("--sync", metavar="REPO",
                   help="reconcile the catalogue with a pencilresearch/midi clone")
    ap.add_argument("--from", dest="from_csv", help="CSV path (with --update)")
    ap.add_argument("--out", help="output path (single-import mode)")
    ap.add_argument("--min-entries", type=int, default=3,
                    help="skip CSVs with fewer than this many entries and no triggers (default 3)")
    ap.add_argument("--dry-run", action="store_true", help="with --sync: report without writing")
    ap.add_argument("csv", nargs="?", help="single CSV to import")
    args = ap.parse_args()

    if args.update:
        if not args.from_csv:
            ap.error("--update requires --from <CSV>")
        csv_path = Path(args.from_csv)
        repo = csv_path.resolve()
        while repo != repo.parent and not (repo / ".git").exists():
            repo = repo.parent
        print(update_existing(Path(args.update), csv_path, repo if (repo / ".git").exists() else None))
        return

    if args.sync:
        sync(Path(args.sync), args.min_entries, args.dry_run)
        return

    if not args.csv:
        ap.print_help()
        sys.exit(2)
    csv_path = Path(args.csv)
    if not csv_path.is_file():
        print(f"error: {csv_path}", file=sys.stderr); sys.exit(1)
    repo = csv_path.resolve().parent.parent  # <repo>/<Vendor>/Device.csv
    out, msg = import_one(csv_path, repo if (repo / ".git").exists() else None,
                          Path(args.out) if args.out else None,
                          args.min_entries)
    if out is None:
        print(msg, file=sys.stderr); sys.exit(1)
    print(f"wrote {msg}")


if __name__ == "__main__":
    main()
