#!/usr/bin/env python3
"""
Import a pencilresearch/midi canonical CSV into an osc-bridge device JSON.

Usage:
    python3 scripts/import_pencilresearch.py <repo>/<Vendor>/<Device>.csv [--out path]
    python3 scripts/import_pencilresearch.py --update devices/<vendor>/<slug>.json --from <CSV>
    python3 scripts/import_pencilresearch.py --bulk <repo-path>

The CSV is expected to come from https://github.com/pencilresearch/midi.

The generated JSON is `📄 doc-derived`, CC/NRPN only (no SysEx). A
`_limitations` field documents the gap explicitly. The `_sources` field is
an array so future SysEx / vendor-doc layers can be appended without
losing the link back to pencilresearch.

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
    with path.open() as f:
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
    return vendor or "Unknown", device or path.stem, entries


def get_git_commit(repo_path: Path) -> str | None:
    try:
        r = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo_path,
                           capture_output=True, text=True, check=True)
        return r.stdout.strip()[:7]
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def pencilresearch_blob_url(commit: str, relative: str) -> str:
    return f"{PENCIL_REPO_URL}/blob/{commit}/{relative}"


def build_device(vendor, device_name, entries, csv_rel_path, commit):
    prefix = "/" + slug(device_name)
    n_cc = sum(1 for e in entries if "cc" in e)
    n_nrpn = sum(1 for e in entries if "nrpn_msb" in e)
    n_14bit = sum(1 for e in entries if "cc_lsb" in e)
    source = {
        "type": "pencilresearch",
        "url": pencilresearch_blob_url(commit, csv_rel_path) if commit else f"{PENCIL_REPO_URL}/{csv_rel_path}",
        "imported_at": date.today().isoformat(),
    }
    if commit:
        source["commit"] = commit
    return {
        "_sources": [source],
        "_limitations": (
            "CC/NRPN only. SysEx (preset management, display control, bulk dump/load, "
            "vendor-proprietary opcodes) is NOT covered by pencilresearch. To extend "
            "this device with SysEx, sniff the vendor editor, add `sysex.header/footer` "
            "+ `commands` blocks, and use `update_device.md` PR template."
        ),
        "_coverage": {
            "entries_total": len(entries),
            "cc_entries": n_cc,
            "nrpn_entries": n_nrpn,
            "fourteen_bit_cc_pairs": n_14bit,
        },
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


def default_out_path(vendor, device_name):
    return REPO_ROOT / "devices" / vendor_slug(vendor) / f"{slug(device_name)}.json"


def import_one(csv_path: Path, pencil_repo: Path | None, out_path: Path | None, min_entries: int = 3):
    vendor, device_name, entries = parse_csv(csv_path)
    if len(entries) < min_entries:
        return None, f"skip (skeleton, {len(entries)} entries): {csv_path.name}"
    commit = get_git_commit(pencil_repo) if pencil_repo else None
    # csv path relative to the pencilresearch repo root
    if pencil_repo:
        try:
            rel = csv_path.relative_to(pencil_repo).as_posix()
        except ValueError:
            rel = csv_path.name
    else:
        rel = csv_path.name
    doc = build_device(vendor, device_name, entries, rel, commit)
    out = out_path or default_out_path(vendor, device_name)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, indent=2) + "\n")
    return out, f"{out.relative_to(REPO_ROOT)} — {len(entries)} entries"


def update_existing(json_path: Path, csv_path: Path, pencil_repo: Path | None):
    """Re-import CC/NRPN while preserving every other block (SysEx, custom commands, etc.)"""
    existing = json.loads(json_path.read_text())
    vendor, device_name, entries = parse_csv(csv_path)
    commit = get_git_commit(pencil_repo) if pencil_repo else None
    try:
        rel = csv_path.relative_to(pencil_repo).as_posix() if pencil_repo else csv_path.name
    except ValueError:
        rel = csv_path.name
    # Replace only cc_params
    if "cc_params" not in existing:
        existing["cc_params"] = {"channel": 0, "entries": []}
    existing["cc_params"]["entries"] = entries
    # Update / replace the pencilresearch source entry
    src = {
        "type": "pencilresearch",
        "url": pencilresearch_blob_url(commit, rel) if commit else f"{PENCIL_REPO_URL}/{rel}",
        "imported_at": date.today().isoformat(),
    }
    if commit:
        src["commit"] = commit
    sources = existing.get("_sources", [])
    # Remove any prior pencilresearch entry, prepend new one, keep others.
    sources = [s for s in sources if s.get("type") != "pencilresearch"]
    sources.insert(0, src)
    existing["_sources"] = sources
    # Refresh coverage
    n_cc = sum(1 for e in entries if "cc" in e)
    n_nrpn = sum(1 for e in entries if "nrpn_msb" in e)
    n_14bit = sum(1 for e in entries if "cc_lsb" in e)
    existing["_coverage"] = {
        "entries_total": len(entries),
        "cc_entries": n_cc,
        "nrpn_entries": n_nrpn,
        "fourteen_bit_cc_pairs": n_14bit,
    }
    json_path.write_text(json.dumps(existing, indent=2) + "\n")
    return f"{json_path.relative_to(REPO_ROOT)} — refreshed {len(entries)} entries"


def bulk_import(repo_path: Path, min_entries: int = 3):
    imported = []
    skipped = []
    for csv_path in sorted(repo_path.rglob("*.csv")):
        if csv_path.parent == repo_path:  # top-level CSVs aren't device files
            continue
        out, msg = import_one(csv_path, repo_path, None, min_entries)
        (imported if out else skipped).append(msg)
    print(f"\n=== Imported {len(imported)} devices ===")
    for m in imported: print("  " + m)
    print(f"\n=== Skipped {len(skipped)} CSVs ===")
    for m in skipped: print("  " + m)


def main():
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--update", metavar="JSON",
                   help="update an existing device JSON with a fresh CSV")
    g.add_argument("--bulk", metavar="REPO",
                   help="import every CSV from a pencilresearch/midi clone")
    ap.add_argument("--from", dest="from_csv", help="CSV path (with --update)")
    ap.add_argument("--out", help="output path (single-import mode)")
    ap.add_argument("--min-entries", type=int, default=3,
                    help="skip CSVs with fewer than this many entries (default 3)")
    ap.add_argument("csv", nargs="?", help="single CSV to import")
    args = ap.parse_args()

    if args.update:
        if not args.from_csv:
            ap.error("--update requires --from <CSV>")
        out_json = Path(args.update)
        csv_path = Path(args.from_csv)
        # Try to detect pencilresearch repo root by walking up from the CSV
        repo = csv_path.resolve()
        while repo != repo.parent:
            if (repo / ".git").exists() or (repo / "README.md").exists() and (repo / "CONTRIBUTING.md").exists():
                break
            repo = repo.parent
        print(update_existing(out_json, csv_path, repo if (repo / ".git").exists() else None))
        return

    if args.bulk:
        bulk_import(Path(args.bulk), args.min_entries)
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
