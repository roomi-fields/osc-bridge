#!/usr/bin/env python3
"""One driver per norns script listed by norns.community: the script's
parameters, read from its Lua source at a pinned commit, as /param/<id>
routes, on top of the norns base driver (`midi_of`), which gives the
system's OSC (/param, /remote). The README comes from the script's
repository, cited (import_readmes.py).

The parameters are read without running anything: the calls
params:add_number/option/control/taper/binary/trigger/file/text and
params:add{...} whose id is written out as a string, with their range,
options and default when those are written out too (literals, controlspec
presets, controlspec.new/def, a local set once in the same file). Ids built
at run time (in loops, from variables) and parameters added by libraries
outside the repository cannot be read: each driver says how many were left.

    GITHUB_TOKEN=$(gh auth token) python3 scripts/import_norns_scripts.py [--only NAME] [--dry-run]
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import import_readmes  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "devices" / "monome"
BASE = "monome/norns.vendor.fw-2.9.4"
INDEX_REPO = "monome/norns-community"
INDEX_COMMIT = "41d05e7a038d76155688e907f4d66c22c09b1740"
AGENT = "synthhub-ops"
TODAY = time.strftime("%Y-%m-%d")

# lua/core/controlspec.lua at monome/norns 57fa534: the presets scripts name.
PRESETS = {
    "UNIPOLAR": (0, 1, 0, ""), "BIPOLAR": (-1, 1, 0, ""), "FREQ": (20, 20000, 440, "Hz"),
    "LOFREQ": (0.1, 100, 6, "Hz"), "MIDFREQ": (25, 4200, 440, "Hz"), "WIDEFREQ": (0.1, 20000, 440, "Hz"),
    "PHASE": (0, 3.141592653589793, 0, ""), "RQ": (0.001, 2, 0.707, ""), "MIDI": (0, 127, 64, ""),
    "MIDINOTE": (0, 127, 60, ""), "MIDIVELOCITY": (1, 127, 64, ""), "DB": (None, 0, None, "dB"),
    "AMP": (0, 1, 0, ""), "BOOSTCUT": (-20, 20, 0, "dB"), "PAN": (-1, 1, 0, ""), "DETUNE": (-20, 20, 0, "Hz"),
    "RATE": (0.125, 8, 1, ""), "BEATS": (0, 20, 0, "Hz"), "DELAY": (0.0001, 1, 0.3, "secs"),
}
KINDS = {"number", "option", "control", "taper", "binary", "trigger", "file", "text"}


class Unknown:
    """A value that is only known when the script runs."""


UNKNOWN = Unknown()


def get(url: str, api: bool = False):
    """A page, from NORNS_CACHE (a folder) when it was fetched before."""
    cache = os.environ.get("NORNS_CACHE")
    if cache:
        import hashlib
        hit = Path(cache) / hashlib.sha256(url.encode()).hexdigest()
        if hit.exists():
            data = hit.read_text(encoding="utf-8")
            return json.loads(data) if api else data
        os.environ.pop("NORNS_CACHE")
        try:
            out = get(url, api)
        finally:
            os.environ["NORNS_CACHE"] = cache
        hit.parent.mkdir(parents=True, exist_ok=True)
        hit.write_text(json.dumps(out) if api else out, encoding="utf-8")
        return out
    req = urllib.request.Request(url, headers={"User-Agent": AGENT})
    if api and os.environ.get("GITHUB_TOKEN"):
        req.add_header("Authorization", f"Bearer {os.environ['GITHUB_TOKEN']}")
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                data = r.read()
            return json.loads(data) if api else data.decode("utf-8", "replace")
        except urllib.error.HTTPError as e:
            if e.code in (403, 429) and attempt < 2:
                time.sleep(60)
                continue
            raise


# --- Lua, as tokens --------------------------------------------------------

TOKEN = re.compile(r"""
    (?P<ws>\s+)
  | (?P<comment>--\[(?P<ceq>=*)\[.*?\](?P=ceq)\]|--[^\n]*)
  | (?P<long>\[(?P<leq>=*)\[.*?\](?P=leq)\])
  | (?P<str>"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*')
  | (?P<num>0[xX][0-9a-fA-F]+|(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?)
  | (?P<name>[A-Za-z_][A-Za-z0-9_]*)
  | (?P<op>\.\.\.|\.\.|==|~=|<=|>=|::|//|[-+*/%^#&~|<>=(){}\[\];:,.])
""", re.S | re.X)


def tokens(src: str) -> list[tuple[str, str]]:
    out, i = [], 0
    while i < len(src):
        m = TOKEN.match(src, i)
        if not m:
            i += 1
            continue
        i = m.end()
        for k in ("str", "num", "name", "op", "long"):
            if m.group(k) is not None:
                out.append(("str" if k == "long" else k, m.group(k)))
                break
    return out


def unquote(s: str) -> str:
    if s.startswith("["):
        return re.sub(r"^\[=*\[\n?|\]=*\]$", "", s)
    body = s[1:-1]
    return re.sub(r"\\(.)", lambda m: {"n": "\n", "t": "\t"}.get(m[1], m[1]), body)


OPEN, CLOSE = {"(": ")", "{": "}", "[": "]"}, {")", "}", "]"}


def split(toks: list, sep=(",",)) -> list[list]:
    """Comma-separated parts at depth 0."""
    parts, cur, depth = [], [], 0
    for t in toks:
        if t[0] == "op" and t[1] in OPEN:
            depth += 1
        elif t[0] == "op" and t[1] in CLOSE:
            depth -= 1
        if depth == 0 and t[0] == "op" and t[1] in sep:
            parts.append(cur)
            cur = []
        else:
            cur.append(t)
    if cur:
        parts.append(cur)
    return parts


def closing(toks: list, i: int) -> int:
    """Index of the bracket closing the one at i."""
    depth = 0
    for j in range(i, len(toks)):
        if toks[j][0] == "op" and toks[j][1] in OPEN:
            depth += 1
        elif toks[j][0] == "op" and toks[j][1] in CLOSE:
            depth -= 1
            if depth == 0:
                return j
    return len(toks) - 1


class Spec:
    def __init__(self, lo, hi, default, units):
        self.lo, self.hi, self.default, self.units = lo, hi, default, units


class Reader:
    """Values written out in one file: literals, tables, controlspecs and
    the locals set once."""

    def __init__(self, toks: list):
        self.toks = toks
        self.aliases = {"controlspec", "ControlSpec"}
        self.locals: dict[str, list] = {}
        seen: dict[str, int] = {}
        for i, t in enumerate(toks):
            if t == ("name", "local") and i + 3 < len(toks) and toks[i + 1][0] == "name" and toks[i + 2] == ("op", "="):
                name = toks[i + 1][1]
                seen[name] = seen.get(name, 0) + 1
                value = self.primary(i + 3)
                if value is not None:
                    self.locals[name] = toks[i + 3:value]
                if toks[i + 3:i + 5] == [("name", "require"), toks[i + 4]] and toks[i + 4][0] == "str" \
                        and unquote(toks[i + 4][1]).split("/")[-1] == "controlspec":
                    self.aliases.add(name)
            elif t[0] == "name" and i + 2 < len(toks) and toks[i + 1] == ("op", "=") \
                    and (i == 0 or toks[i - 1][1] not in (".", ":", ",", "local")):
                seen[t[1]] = seen.get(t[1], 0) + 1
                value = self.primary(i + 2)
                if value is not None:
                    self.locals[t[1]] = toks[i + 2:value]
        for name, n in seen.items():
            if n > 1:
                self.locals.pop(name, None)
        self.depth = 0

    def primary(self, i: int) -> int | None:
        """End of the one balanced value starting at i, if what follows
        does not carry on the expression."""
        toks, j = self.toks, i
        if j >= len(toks):
            return None
        if toks[j][0] in ("str", "num") or toks[j] in (("name", "true"), ("name", "false")):
            j += 1
        elif toks[j] == ("op", "-") and j + 1 < len(toks) and toks[j + 1][0] == "num":
            j += 2
        elif toks[j] == ("op", "{"):
            j = closing(toks, j) + 1
        elif toks[j][0] == "name":
            j += 1
            while j + 1 < len(toks) and toks[j][1] in (".", ":") and toks[j + 1][0] == "name":
                j += 2
            if j < len(toks) and toks[j][1] in ("(", "{"):
                j = closing(toks, j) + 1
        else:
            return None
        if j < len(toks) and toks[j][0] == "op" and toks[j][1] not in (",", ";", ")", "}", "]"):
            if toks[j][1] not in ("=",):
                return None
        return j

    def value(self, part: list):
        if not part:
            return UNKNOWN
        if len(part) == 1:
            k, v = part[0]
            if k == "str":
                return unquote(v)
            if k == "num":
                return int(v, 16) if v.lower().startswith("0x") else (float(v) if re.search(r"[.eE]", v) else int(v))
            if v in ("true", "false"):
                return v == "true"
            if v == "nil":
                return None
            if k == "name" and v in self.locals and self.depth < 8:
                self.depth += 1
                try:
                    return self.value(self.locals[v])
                finally:
                    self.depth -= 1
            return UNKNOWN
        if part[0] == ("op", "-") and len(part) == 2:
            v = self.value(part[1:])
            return -v if isinstance(v, (int, float)) and not isinstance(v, bool) else UNKNOWN
        if part[0] == ("op", "{") and closing(part, 0) == len(part) - 1:
            return self.table(part[1:-1])
        names = [t[1] for t in part]
        if names in (["math", ".", "pi"],):
            return 3.141592653589793
        if names in (["math", ".", "huge"],):
            return float("inf")
        if len(part) >= 3 and part[0][1] in self.aliases and part[1] == ("op", "."):
            what = part[2][1]
            rest = part[3:]
            if what in PRESETS and (not rest or [t[1] for t in rest] == [":", "copy", "(", ")"]):
                return Spec(*PRESETS[what])
            if what == "new" and rest and rest[0] == ("op", "(") and closing(rest, 0) == len(rest) - 1:
                a = [self.value(p) for p in split(rest[1:-1])] + [None] * 8
                lo = a[0] if a[0] is not None else 0
                hi = a[1] if a[1] is not None else 1
                return Spec(lo, hi, a[4] if a[4] is not None else lo, a[5] if isinstance(a[5], str) else "")
            if what == "def" and rest:
                inner = rest[1:-1] if rest[0] == ("op", "(") else rest
                t = self.value(inner)
                if isinstance(t, dict):
                    lo, hi = t.get("min", 0), t.get("max", 1)
                    d = t.get("default", lo)
                    return Spec(lo, hi, d, t.get("units", "") if isinstance(t.get("units", ""), str) else "")
        return UNKNOWN

    def table(self, inner: list):
        arr, keyed = [], {}
        for field in split(inner, (",", ";")):
            if len(field) >= 3 and field[0][0] == "name" and field[1] == ("op", "="):
                keyed[field[0][1]] = self.value(field[2:])
            elif field and field[0] == ("op", "[") and ("op", "=") in field:
                end = closing(field, 0)
                key = self.value(field[1:end])
                if isinstance(key, (str, int)):
                    keyed[key] = self.value(field[end + 2:])
            else:
                arr.append(self.value(field))
        if keyed and not arr:
            return keyed
        if arr and not keyed:
            return arr
        return UNKNOWN


def params_of(src: str) -> tuple[list[dict], int]:
    """The parameters a file adds that can be read, and how many it adds
    that cannot (id or kind only known when it runs)."""
    toks = tokens(src)
    r = Reader(toks)
    found, unread, group, left = [], 0, None, 0
    for i in range(len(toks) - 3):
        if toks[i] != ("name", "params") or toks[i + 1] != ("op", ":") or toks[i + 2][0] != "name":
            continue
        method = toks[i + 2][1]
        j = i + 3
        if toks[j] == ("op", "{"):
            fields = r.value(toks[j:closing(toks, j) + 1])
        elif toks[j] == ("op", "("):
            end = closing(toks, j)
            args = split(toks[j + 1:end])
            if method == "add":
                fields = r.value(args[0]) if len(args) == 1 else UNKNOWN
            else:
                fields = [r.value(a) for a in args]
        else:
            continue
        if method == "add":
            if not isinstance(fields, dict):
                unread += 1
                continue
            kind = fields.get("type")
            if kind in ("separator", "group"):
                if kind == "group":
                    group = fields.get("name") if isinstance(fields.get("name"), str) else None
                    left = fields.get("n") if num(fields.get("n")) else 1
                continue
            p = {k: fields.get(k) for k in ("id", "name", "min", "max", "default", "options", "controlspec", "units", "behavior")}
            p["kind"] = kind
        elif method.startswith("add_") and method[4:] in KINDS:
            kind, a = method[4:], fields + [None] * 8
            p = {"kind": kind, "id": a[0], "name": a[1]}
            if kind == "number":
                p.update(min=a[2], max=a[3], default=a[4])
            elif kind == "option":
                p.update(options=a[2], default=a[3])
            elif kind == "control":
                p.update(controlspec=a[2])
            elif kind == "taper":
                p.update(min=a[2], max=a[3], default=a[4], units=a[6])
            elif kind == "binary":
                p.update(behavior=a[2], default=a[3])
        elif method == "add_group":
            # add_group(name, n) or add_group(id, name, n)
            if isinstance(fields, list) and len(fields) >= 2:
                group, left = (fields[0], fields[1]) if num(fields[1]) else (fields[1], fields[2] if len(fields) > 2 else 1)
                group = group if isinstance(group, str) else None
                left = left if num(left) else 0
            continue
        else:
            continue
        left -= 1
        if not isinstance(p.get("id"), str) or not re.fullmatch(r"[A-Za-z0-9_.\-]+", p["id"]) or p.get("kind") not in KINDS:
            unread += 1
            continue
        p["group"] = group if left >= 0 else None
        found.append(p)
    return found, unread


# --- one driver per script -------------------------------------------------

def num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool) and v not in (float("inf"), float("-inf"))


def show(v) -> str:
    return str(int(v)) if num(v) and float(v).is_integer() else f"{v:g}" if num(v) else str(v)


def command(p: dict) -> dict:
    name = p["name"] if isinstance(p.get("name"), str) else p["id"]
    arg: dict = {"name": "value"}
    notes = f"{name}"
    kind = p["kind"]
    if kind == "number":
        ints = all(isinstance(p.get(k), int) and not isinstance(p.get(k), bool) for k in ("min", "max"))
        arg["type"] = "int" if ints else "float"
        if ints:
            arg["range"] = [p["min"], p["max"]]
        elif num(p.get("min")) and num(p.get("max")):
            notes += f": {show(p['min'])} to {show(p['max'])}"
        if num(p.get("default")):
            arg["default"] = p["default"]
    elif kind == "option":
        arg["type"] = "int"
        opts = p.get("options")
        if isinstance(opts, list) and opts and all(isinstance(o, (str, int, float)) and not isinstance(o, bool) for o in opts):
            arg["range"] = [1, len(opts)]
            arg["values"] = {show(o): i + 1 for i, o in enumerate(opts)}
        notes += ": option index, from 1"
        if isinstance(p.get("default"), int):
            arg["default"] = p["default"]
    elif kind in ("control", "taper"):
        arg["type"] = "float"
        spec = p.get("controlspec") if kind == "control" else Spec(p.get("min"), p.get("max"), p.get("default"), p.get("units"))
        if kind == "control" and p.get("controlspec") is None and num(p.get("min")):
            spec = Spec(p.get("min"), p.get("max"), p.get("default"), p.get("units"))
        if isinstance(spec, Spec):
            if num(spec.lo) and num(spec.hi):
                units = f" {spec.units}" if isinstance(spec.units, str) and spec.units else ""
                notes += f": {show(spec.lo)} to {show(spec.hi)}{units}"
            if num(spec.default):
                arg["default"] = spec.default
    elif kind == "binary":
        arg.update(type="int", range=[0, 1])
        b = p.get("behavior")
        notes += f": {b}, 0 or 1" if isinstance(b, str) else ": 0 or 1"
        if isinstance(p.get("default"), int):
            arg["default"] = p["default"]
    elif kind == "trigger":
        return {"osc": f"/param/{p['id']}", "section": p.get("group") or "Parameters", "args": [],
                "forward": {"path": f"/param/{p['id']}", "args": [1]}, "_notes": f"{name}: trigger."}
    else:   # file, text
        arg["type"] = "string"
        notes += ": a file path on the norns" if kind == "file" else ": text"
    return {"osc": f"/param/{p['id']}", "section": p.get("group") or "Parameters", "args": [arg],
            "forward": {"path": f"/param/{p['id']}", "args": ["{value}"]}, "_notes": notes + "."}


def slug(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-") or "script"


def script(entry: dict) -> dict | None:
    m = re.match(r"https://github\.com/([^/]+)/([^/#?]+)(?:/tree/([^/]+)/(.+))?", entry["project_url"].rstrip("/"))
    if not m:
        return None
    repo, folder = f"{m[1]}/{m[2].removesuffix('.git')}", (m[4] or "").strip("/")
    info = get(f"https://api.github.com/repos/{repo}", api=True)
    sha = get(f"https://api.github.com/repos/{repo}/commits/{m[3] or info['default_branch']}", api=True)["sha"]
    tree = get(f"https://api.github.com/repos/{repo}/git/trees/{sha}?recursive=1", api=True)["tree"]
    files = [t["path"] for t in tree if t["type"] == "blob" and t["path"].endswith(".lua")
             and (not folder or t["path"].startswith(folder + "/"))]
    seen, cmds, unread, read = set(), [], 0, []
    for f in sorted(files, key=lambda f: (f.count("/"), f)):
        src = get(f"https://raw.githubusercontent.com/{repo}/{sha}/{f}")
        ps, n = params_of(src)
        unread += n
        if ps or n:
            read.append(f)
        for p in ps:
            if p["id"] in seen:
                continue
            seen.add(p["id"])
            cmds.append(command(p))
    licence = (info.get("license") or {}).get("spdx_id")
    licence = licence if licence and licence != "NOASSERTION" else None
    return {"repo": repo, "sha": sha, "folder": folder, "files": len(files), "read": read,
            "commands": cmds, "unread": unread, "licence": licence}


def driver(entry: dict, s: dict) -> dict:
    name, sha7 = entry["project_name"], s["sha"][:7]
    where = f"https://github.com/{s['repo']}/tree/{s['sha']}" + (f"/{s['folder']}" if s["folder"] else "")
    n = len(s["commands"])
    left = (f" {s['unread']} parameter call(s) could not be read without running the script (ids built at run time, "
            "values from variables) and are not mapped: reach them with the base driver's /param/{id}." if s["unread"] else "")
    lib = (" Parameters added by libraries or engines outside the repository are not mapped either." if n or s["unread"] else
           " No parameter could be read from its source (it may add none, or add them through a library or at run time):"
           " reach its parameters with the base driver's /param/{id}.")
    limitations = (f"Source: the Lua source of the norns script {name} by {entry.get('author') or s['repo'].split('/')[0]}"
                   f" ({s['repo']} at {sha7}), listed in norns.community. Shares the norns system OSC of {BASE} (port 10111,"
                   f" /param, /remote): give the norns' IP address as host override. Its own routes are the {n} parameter(s)"
                   " whose id is written out in its source, read without running it, with range, options and default when"
                   f" written out too; set them while the script runs on the norns.{left}{lib}"
                   " A value goes in the parameter's units (an option's index from 1). MIDI is not mapped: the script's MIDI"
                   " handling is its own and parameter MIDI mappings are learned by each user (PMAP). Untested on hardware.")
    notes = f"norns script {name}: " + (", ".join(s["read"][:12]) + (" and more" if len(s["read"]) > 12 else "") if s["read"]
                                         else f"{s['files']} Lua file(s) read") + f" @ {sha7}."
    if s["licence"]:
        notes += f" Licence {s['licence']}."
    else:
        notes += " No licence declared."
    notes += f" Listed in norns.community ({INDEX_REPO} community.json @ {INDEX_COMMIT[:7]})."
    doc = {
        "_sources": [{"type": "vendor-doc", "name": f"norns script {name}", "url": where, "notes": notes,
                      "firmware": f"{slug(name)}-{sha7}", "imported_at": TODAY}],
        "_limitations": limitations,
        "_coverage": {"host_software": f"norns script {name}", "commands": n, "subscriptions": 0, "replies": 0},
        "midi_of": BASE,
        "device": {"name": "norns", "vendor": "Monome", "revision": f"script {name} ({s['repo']} @ {sha7})",
                   "osc_prefix": "/norns_" + slug(name).replace("-", "_")},
    }
    if s["commands"]:
        doc["commands"] = s["commands"]
    return doc


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--only", help="the scripts whose name contains this")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    index = get(f"https://raw.githubusercontent.com/{INDEX_REPO}/{INDEX_COMMIT}/community.json")
    entries = json.loads(index)["entries"]
    if args.only:
        entries = [e for e in entries if args.only.lower() in e["project_name"].lower()]
    ours = lambda p: (json.loads(p.read_text(encoding="utf-8")).get("_sources") or [{}])[0].get("name", "").startswith("norns script ")
    stale = {p for p in OUT.glob("norns.vendor.fw-*.json") if ours(p)}
    total = written = params = unread = 0
    skipped: list[str] = []
    names: dict[str, str] = {}
    for e in entries:
        total += 1
        try:
            s = script(e)
        except (urllib.error.HTTPError, urllib.error.URLError, KeyError) as exc:
            skipped.append(f"{e['project_name']}: {exc}")
            continue
        if s is None:
            skipped.append(f"{e['project_name']}: not a GitHub repository")
            continue
        doc = driver(e, s)
        prefix = doc["device"]["osc_prefix"]
        if prefix in names:
            skipped.append(f"{e['project_name']}: same name as {names[prefix]}")
            continue
        names[prefix] = e["project_name"]
        path = OUT / f"norns.vendor.fw-{doc['_sources'][0]['firmware']}.json"
        stale.discard(path)
        params += len(s["commands"])
        unread += s["unread"]
        written += 1
        if not args.dry_run:
            path.write_text(json.dumps(doc, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
            md = path.with_suffix(".md")
            text = import_readmes.companion(s["repo"], s["sha"], s["folder"])
            if text and (not md.exists() or md.read_text(encoding="utf-8").startswith(import_readmes.MARK)):
                md.write_text(text, encoding="utf-8")
        print(f"{e['project_name']}: {len(s['commands'])} parameter(s), {s['unread']} unread, {s['files']} Lua file(s)")
    if not args.only and not args.dry_run:
        for p in stale:   # scripts gone from the index, or read at another commit
            p.unlink()
            p.with_suffix(".md").unlink(missing_ok=True)
    print(f"{total} scripts indexed, {written} {'to write' if args.dry_run else 'written'}, {len(skipped)} skipped; "
          f"{params} parameters read, {unread} left unread")
    for s in skipped:
        print("  skipped", s)
    return 0


if __name__ == "__main__":
    sys.exit(main())
