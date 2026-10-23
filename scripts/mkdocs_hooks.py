"""MkDocs hooks for the osc-bridge site.

1. One page per driver file, generated from `devices/**/*.json` at build time:
   its named OSC addresses with their CC / NRPN numbers, SysEx commands, drum
   sounds, source and licence, and the command that runs it. Nothing to keep
   in sync by hand — the JSON is the single source.
2. Each page carries its own structured data (TechArticle / BlogPosting /
   CollectionPage) linked to one publisher, one website and the software,
   whose version is read from Cargo.toml; the last change date is shown.
3. `robots.txt` and `llms.txt` write `{site_url}`, filled in from mkdocs.yml.
4. A static JSON API for agents under `api/v1/`: `index.json` lists every
   driver with its source tier, firmware and coverage; `devices/<vendor>/
   <driver>.json` is the driver file itself, byte for byte.

After the build every page is checked as a crawler will receive it: a page
without its block, with a block that does not parse or that lacks a field,
fails the build, as does a robots / llms file not pointing at the site.
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
from collections import defaultdict
from datetime import date
from pathlib import Path

from mkdocs.exceptions import PluginError

from mkdocs.structure.files import File

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_device_index import coverage  # noqa: E402 — the catalogue's own counts

ROOT = Path(__file__).resolve().parent.parent
DEVICES = ROOT / "devices"
REPO = "https://github.com/roomi-fields/osc-bridge"
ORG = "https://roomi-fields.com/#organization"
REQUIRED = ("@type", "headline", "description", "url", "dateModified",
            "publisher", "isPartOf", "about")
FILLED = ("robots.txt", "llms.txt")
EXEMPT = {"404.html"}
EXEMPT_UNDER = ("blog/page/", "overrides/")

TIERS = {
    "hardware-verified": ("✅", "Verified on the physical device"),
    "software-verified": ("✅", "Verified against the running software"),
    "vendor-doc": ("📘", "From the manufacturer's documentation"),
    "vendor": ("📘", "From the manufacturer's documentation"),
    "vendor-osc-api": ("📡", "From the software's own OSC API"),
    "third-party-osc": ("📡", "From a third-party OSC API"),
    "electra-preset": ("🎛️", "Derived from an Electra One community preset"),
    "pencilresearch": ("📦", "Derived from MIDI Guide (community CC/NRPN data)"),
    "agent-contributed": ("🤖", "Contributed by an AI agent; checked automatically, not reviewed"),
    "test-fixture": ("🧪", "Virtual test device, simulated by osc-bridge connect --test-synth"),
}

_LD = re.compile(r'<script type="application/ld\+json">(.*?)</script>', re.DOTALL)
_DEVICE_PAGES: dict[str, dict] = {}


# ---------------------------------------------------------------- utilities

def _version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    return re.search(r'^version = "([^"]+)"', text, re.MULTILINE).group(1)


def _git_date(path: Path, first: bool = False) -> str | None:
    args = ["git", "log", "--format=%cs"] + (["--follow", "--diff-filter=A"] if first else ["-1"])
    try:
        out = subprocess.run(args + ["--", str(path)], cwd=ROOT, capture_output=True,
                             text=True, check=True).stdout.split()
    except (OSError, subprocess.CalledProcessError):
        return None
    return (out[-1] if first else out[0]) if out else None


_DEVICE_DATES: dict[str, tuple[str, str]] = {}


def _device_dates() -> dict[str, tuple[str, str]]:
    """(last change, first commit) of every file under devices/, from one
    `git log` pass instead of two calls per page."""
    if _DEVICE_DATES:
        return _DEVICE_DATES
    try:
        out = subprocess.run(["git", "log", "--format=@%cs", "--name-only", "--", "devices"],
                             cwd=ROOT, capture_output=True, text=True, check=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return _DEVICE_DATES
    when = None
    for line in out.splitlines():
        if line.startswith("@"):
            when = line[1:]
        elif line and when:
            last, _ = _DEVICE_DATES.get(line, (when, when))
            _DEVICE_DATES[line] = (last, when)   # log is newest first
    return _DEVICE_DATES


def _cell(v) -> str:
    if v is None or v == "":
        return ""
    return str(v).replace("|", "\\|").replace("\n", " ")


def _text(v) -> str:
    """Escape Markdown-active characters in prose taken from JSON."""
    return re.sub(r"([*_`<>\[\]])", r"\\\1", str(v))


def _range(r) -> str:
    return f"{r[0]}–{r[1]}" if isinstance(r, list) and len(r) == 2 else ""


# ----------------------------------------------------------- device pages

def _load_devices() -> list[dict]:
    out = []
    for path in sorted(DEVICES.rglob("*.json")):
        try:
            doc = json.loads(path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
        meta = doc.get("device") or {}
        if not meta.get("name"):
            continue
        rel = path.relative_to(DEVICES).with_suffix("").as_posix()
        out.append({"doc": doc, "meta": meta, "path": path, "rel": rel,
                    "src_uri": f"devices/{rel}.md"})
    return out


def _title(meta: dict) -> str:
    name, vendor = meta["name"], meta.get("vendor", "")
    full = name if name.lower().startswith(vendor.lower()) or not vendor else f"{vendor} {name}"
    return full


def _device_markdown(d: dict, siblings: list[dict]) -> tuple[str, str, str]:
    doc, meta = d["doc"], d["meta"]
    full = _title(meta)
    software = (meta.get("transport") or {}).get("kind") == "osc"
    prefix = meta.get("osc_prefix", "")
    source = (doc.get("_sources") or [{}])[0]
    icon, tier_text = TIERS.get(source.get("type", ""), ("•", "Community source"))
    cc = (doc.get("cc_params") or {}).get("entries", [])
    sx = (doc.get("params") or {}).get("entries", [])
    cmds = doc.get("commands", [])
    trig = (doc.get("triggers") or {}).get("entries", [])
    n_cc = sum(1 for e in cc if e.get("cc") is not None)
    n_nrpn = sum(1 for e in cc if e.get("nrpn_msb") is not None)

    if software:
        title = f"{full}: OSC control from Claude, scripts and controllers"
        summary = (f"{len(cmds)} named commands to drive {full} over OSC — from Claude "
                   f"(MCP), live-coding environments or a MIDI controller.")
    else:
        parts = [f"{n_cc} CC" if n_cc else "", f"{n_nrpn} NRPN" if n_nrpn else "",
                 f"{len(sx)} SysEx parameters" if sx else "", f"{len(cmds)} SysEx commands" if cmds else "",
                 f"{len(trig)} drum sounds" if trig else ""]
        what = ", ".join(p for p in parts if p) or "its MIDI messages"
        title = f"{full}: MIDI CC, NRPN and OSC map"
        summary = (f"The {full} as named OSC addresses — {what} — ready to use from Claude "
                   f"(MCP), SuperCollider, Sonic Pi, TouchDesigner or any OSC app via osc-bridge.")

    L = [
        "---",
        f"title: {json.dumps(title, ensure_ascii=False)}",
        f"description: {json.dumps(summary, ensure_ascii=False)}",
        "search:",
        "  exclude: true",
        "---",
        "",
        f"# {_text(title)}",
        "",
        _text(summary),
        "",
        f"| Device | {_cell(full)} |",
        "|---|---|",
        f"| Vendor | {_cell(meta.get('vendor'))} |",
        f"| OSC prefix | `{_cell(prefix)}` |",
        f"| Source | {icon} {tier_text} |",
    ]
    if source.get("firmware"):
        L.append(f"| Firmware | {_cell(source['firmware'])} |")
    if meta.get("revision"):
        L.append(f"| Revision | {_cell(meta['revision'])} |")
    if source.get("url"):
        L.append(f"| Upstream | [{_cell(source.get('type', 'source'))}]({source['url']}) |")
    if source.get("license"):
        L.append(f"| Data licence | {_cell(source['license'])} — {_cell(source.get('attribution', ''))} |")
    L.append(f"| Driver file | [`devices/{d['rel']}.json`]({REPO}/blob/main/devices/{d['rel']}.json) |")
    L.append("")

    if source.get("type") not in ("hardware-verified", "software-verified"):
        L += ["!!! warning \"Not tested on the device yet\"",
              "    This driver comes from documentation or community data and has not been",
              "    checked on real hardware. If you own one, a quick test and a pull request",
              f"    promoting it helps everyone — see [CONTRIBUTING]({REPO}/blob/main/CONTRIBUTING.md).",
              ""]

    others = [s for s in siblings if s is not d]
    if others:
        here = d["rel"].rsplit("/", 1)[0]
        L.append("**Other variants of this device:** " + ", ".join(
            f"[{s['rel'].split('/')[-1]}]({s['rel'].split('/')[-1]}.md)" if s["rel"].rsplit("/", 1)[0] == here
            else f"[{s['rel']}](../{s['rel']}.md)" for s in others) + ".")
        L.append("")

    L += ["## How do you control it?", ""]
    if software:
        L += ["Start the bridge with this driver; it forwards named OSC commands to the software:", ""]
        L += ["```bash", f"osc-bridge run --device devices/{d['rel']}.json", "```", ""]
    else:
        L += ["Find the MIDI ports, then start the bridge with this driver:", "",
              "```bash", "osc-bridge detect        # prints the ready-to-run line for what is plugged in",
              f"osc-bridge run --device devices/{d['rel']}.json --out-port <N> --in-port <M>", "```", ""]
    example = None
    if cc:
        e = cc[0]
        r = e.get("range", [0, 127])
        example = f"{prefix}{e['osc']} {r[1] // 2 if isinstance(r, list) else 64}"
    elif trig:
        example = f"{prefix}{trig[0]['osc']} 100"
    elif cmds:
        example = f"{prefix}{cmds[0]['osc']}"
    if example:
        L += ["Then send OSC to `127.0.0.1:7777`, for example:", "",
              "```bash", f"osc-bridge osc-send {example}", "```", ""]
    L += [f"From Claude or any MCP client, the `osc-bridge mcp` server exposes this driver as "
          f"`{prefix.lstrip('/')}`: ask *“what can the {_text(full)} do?”* and it reads this map with "
          "`list_routes`. See [the MCP guide](../../MCP.md).", ""]

    if cc:
        L += [f"## MIDI CC and NRPN parameters ({len(cc)})", "",
              "| OSC address | CC | NRPN (MSB/LSB) | Range | Nature | Section |",
              "|---|---|---|---|---|---|"]
        for e in cc:
            ccn = e.get("cc")
            ccs = "" if ccn is None else (f"{ccn} / {e['cc_lsb']}" if e.get("cc_lsb") is not None else str(ccn))
            nr = f"{e['nrpn_msb']} / {e['nrpn_lsb']}" if e.get("nrpn_msb") is not None else ""
            L.append(f"| `{_cell(prefix + e['osc'])}` | {ccs} | {nr} | {_range(e.get('range'))} | "
                     f"{_cell(e.get('expects', 'continuous'))} | {_cell(e.get('section', ''))} |")
        L.append("")
    if sx:
        L += [f"## SysEx parameters ({len(sx)})", "", "| OSC address | Range | Nature |", "|---|---|---|"]
        for e in sx:
            L.append(f"| `{_cell(prefix + e['osc'])}` | {_range(e.get('range'))} | {_cell(e.get('expects', 'continuous'))} |")
        L.append("")
    if cmds:
        label = "OSC commands" if software else "SysEx commands"
        L += [f"## {label} ({len(cmds)})", "", "| OSC address | Arguments |", "|---|---|"]
        for c in cmds:
            args = ", ".join(f"{a.get('name')}: {a.get('type')}" for a in c.get("args", []))
            L.append(f"| `{_cell(prefix + c['osc'])}` | {_cell(args)} |")
        L.append("")
    if trig:
        L += [f"## Drum sounds ({len(trig)})", "",
              "Send `<address> [velocity] [note] [channel]` to fire the sound (Note On).", "",
              "| OSC address | Sound | Note(s) | Channel | Velocity |", "|---|---|---|---|---|"]
        for t in trig:
            lo, hi = t["note_range"]
            notes = str(lo) if lo == hi else f"{lo}–{hi}" + (f" (natural {t['note']})" if t.get("note") is not None else "")
            ch = str(t["channel"] + 1) if t.get("channel") is not None else t.get("channel_mode", "")
            L.append(f"| `{_cell(prefix + t['osc'])}` | {_cell(t['sound'])} | {notes} | {_cell(ch)} | {_cell(t.get('velocity_usage', ''))} |")
        L.append("")
    if doc.get("midi_out"):
        L += ["## Performance MIDI", "",
              f"Notes, pitch bend and any CC are also available: `{prefix}/note/on <note> <velocity>`, "
              f"`{prefix}/cc/<number> <value>`, `{prefix}/pitchbend <0-16383>`, `{prefix}/program_change <n>`.", ""]
    if doc.get("_limitations"):
        L += ["## Limitations", "", _text(doc["_limitations"]), ""]
    return "\n".join(L), title, summary


def on_files(files, config):
    devices = _load_devices()
    groups: dict[tuple, list] = defaultdict(list)
    for d in devices:
        groups[(d["meta"].get("vendor", "").lower(), d["meta"]["name"].lower())].append(d)
    for d in devices:
        siblings = groups[(d["meta"].get("vendor", "").lower(), d["meta"]["name"].lower())]
        content, title, summary = _device_markdown(d, siblings)
        files.append(File.generated(config, d["src_uri"], content=content))
        _DEVICE_PAGES[d["src_uri"]] = {"path": d["path"], "meta": d["meta"]}
    return files


# ------------------------------------------------------ structured data

def _site_nodes(site: str, description: str) -> list[dict]:
    return [
        {"@type": "Organization", "@id": ORG, "name": "roomi-fields", "url": "https://roomi-fields.com",
         "founder": {"@type": "Person", "name": "Romain Peyrichou", "url": "https://github.com/roomi-fields"},
         "sameAs": ["https://github.com/roomi-fields"]},
        {"@type": "WebSite", "@id": f"{site}#website", "url": site,
         "name": "osc-bridge documentation", "inLanguage": "en", "publisher": {"@id": ORG}},
        {"@type": "SoftwareApplication", "@id": f"{site}#software", "name": "osc-bridge",
         "description": description, "applicationCategory": "MultimediaApplication",
         "operatingSystem": "Linux, macOS, Windows", "softwareVersion": _version(),
         "license": "https://www.gnu.org/licenses/gpl-3.0.html",
         "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD"},
         "downloadUrl": f"{REPO}/releases", "codeRepository": REPO,
         "programmingLanguage": "Rust", "publisher": {"@id": ORG}},
    ]


def on_page_content(content, page, config, files):
    device = _DEVICE_PAGES.get(page.file.src_uri)
    if device:
        rel = device["path"].relative_to(ROOT).as_posix()
        modified, published = _device_dates().get(rel, (None, None))
    else:
        src = Path(config["docs_dir"]) / page.file.src_path
        modified, published = _git_date(src), _git_date(src, first=True)
    modified = modified or date.today().isoformat()
    page.meta["_modified"] = modified
    page.meta["_published"] = published
    content += (f'\n<aside class="md-source-file"><small>Last updated: '
                f'<time datetime="{modified}">{modified}</time></small></aside>')
    return content


def _post_date(page) -> str | None:
    created = getattr(getattr(getattr(page, "config", None), "date", None), "created", None)
    return created.strftime("%Y-%m-%d") if created else None


def on_post_page(output, page, config):
    site = config["site_url"]
    is_post = page.file.src_uri.startswith("blog/posts/")
    device = _DEVICE_PAGES.get(page.file.src_uri)
    node = {
        "@type": page.meta.get("schema_type", "BlogPosting" if is_post else "TechArticle"),
        "@id": f"{page.canonical_url}#page",
        "headline": page.meta.get("title") or page.title,
        "description": " ".join(str(page.meta.get("description") or config["site_description"]).split()),
        "url": page.canonical_url,
        "inLanguage": "en",
        "dateModified": page.meta.get("_modified") or _post_date(page),
        "datePublished": _post_date(page) or page.meta.get("_published"),
        "author": ({"@type": "Person", "name": "Romain Peyrichou",
                    "url": "https://github.com/roomi-fields"} if is_post else None),
        "publisher": {"@id": ORG},
        "isPartOf": {"@id": f"{site}#website"},
        "about": {"@id": f"{site}#software"},
        "mentions": ({"@type": "Product", "name": device["meta"]["name"],
                      "brand": {"@type": "Brand", "name": device["meta"].get("vendor") or "unknown"}}
                     if device else None),
    }
    node = {k: v for k, v in node.items() if v}
    graph = {"@context": "https://schema.org",
             "@graph": _site_nodes(site, " ".join(config["site_description"].split())) + [node]}
    block = ('<script type="application/ld+json">'
             + json.dumps(graph, ensure_ascii=False).replace("</", "<\\/") + "</script>")
    return output.replace("</head>", block + "\n</head>", 1)


def _api_entry(d: dict, site: str) -> dict:
    doc, meta = d["doc"], d["meta"]
    source = (doc.get("_sources") or [{}])[0]
    firmware = source.get("firmware")
    kind = meta.get("kind") or ("software" if (meta.get("transport") or {}).get("kind") == "osc" else "hardware")
    return {
        "id": d["rel"],
        "vendor": meta.get("vendor", ""),
        "name": meta["name"],
        "slug": (meta.get("osc_prefix") or "").lstrip("/"),
        "kind": kind,
        "source_tier": source.get("type", "unknown"),
        "firmware": None if firmware in (None, "unknown") else firmware,
        "license": source.get("license"),
        "manufacturer_id": meta.get("manufacturer_id"),
        "coverage": coverage(doc),
        "url": f"{site}api/v1/devices/{d['rel']}.json",
        "page": f"{site}devices/{d['rel']}/",
    }


def _write_api(site_dir: Path, site: str) -> int:
    api = site_dir / "api" / "v1"
    devices = _load_devices()
    for d in devices:
        out = api / "devices" / f"{d['rel']}.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(d["path"].read_bytes())
    index = {
        "version": _version(),
        "count": len(devices),
        "source": REPO,
        "license": "Drivers are GPL-3.0; those derived from MIDI Guide are CC BY-SA 4.0 (see each entry).",
        "devices": [_api_entry(d, site) for d in devices],
    }
    (api / "index.json").write_text(json.dumps(index, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    return len(devices)


def on_post_build(config):
    site_dir = Path(config["site_dir"])
    problems = []
    print(f"INFO    -  API: {_write_api(site_dir, config['site_url'])} drivers under api/v1/")
    for name in FILLED:
        path = site_dir / name
        text = path.read_text(encoding="utf-8").replace("{site_url}", config["site_url"])
        path.write_text(text, encoding="utf-8")
        if config["site_url"] not in text:
            problems.append(f"{name}: does not point at {config['site_url']}")
    checked = 0
    for page in sorted(site_dir.rglob("*.html")):
        rel = page.relative_to(site_dir).as_posix()
        if rel in EXEMPT or rel.startswith(EXEMPT_UNDER):
            continue
        text = page.read_text(encoding="utf-8")
        if 'http-equiv="refresh"' in text:
            continue
        blocks = _LD.findall(text)
        if not blocks:
            problems.append(f"{rel}: no structured data")
            continue
        try:
            graph = json.loads(blocks[-1])
        except json.JSONDecodeError as exc:
            problems.append(f"{rel}: structured data does not parse — {exc}")
            continue
        own = graph.get("@graph", [{}])[-1]
        missing = [f for f in REQUIRED if not own.get(f)]
        if missing:
            problems.append(f"{rel}: page block lacks {', '.join(missing)}")
        checked += 1
    if problems:
        raise PluginError("Pages a crawler would misread:\n  " + "\n  ".join(problems[:40]))
    print(f"INFO    -  Structured data checked on {checked} pages")
