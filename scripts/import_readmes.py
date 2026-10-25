#!/usr/bin/env python3
"""Give a driver taken from an author's GitHub repository (a community
firmware's: `_sources[0]` of type vendor-doc on github.com) the start of that
repository's README, cited, as its companion `<driver>.md`: what the site
shows as the page's README and agents read with get_device_docs.

The text is the README's opening (up to its second section, about 3,000
characters at most), its HTML turned into Markdown, its links made absolute
at the commit read; above it, where it comes from (repository, commit,
licence) and below it, a link to the whole README. A companion written by
hand (one without this script's first line) is never touched; one this
script wrote is rewritten when the README changed.

    GITHUB_TOKEN=$(gh auth token) python3 scripts/import_readmes.py [--dry-run]
"""
from __future__ import annotations

import argparse
import base64
import html
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEVICES = ROOT / "devices"
MARK = "<!-- imported README:"
MAX = 3000


def gh(path: str) -> dict:
    req = urllib.request.Request(f"https://api.github.com/{path}", headers={"Accept": "application/vnd.github+json"})
    if os.environ.get("GITHUB_TOKEN"):
        req.add_header("Authorization", f"Bearer {os.environ['GITHUB_TOKEN']}")
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def to_markdown(text: str, blob: str) -> str:
    """The README's HTML as Markdown, its relative links absolute under
    `blob` (https://github.com/o/r/blob/<sha>/)."""
    text = re.sub(r"<!--.*?-->", "", text, flags=re.S)
    text = re.sub(r"<(https?://[^>\s]+)>", r"[\1](\1)", text)   # autolinks, before tags go
    text = re.sub(r"<img\b[^>]*>", "", text, flags=re.I)
    text = re.sub(r'<a\b[^>]*href="([^"]+)"[^>]*>(.*?)</a>', lambda m: f"[{m[2].strip()}]({m[1]})", text, flags=re.I | re.S)
    text = re.sub(r"</?(b|strong)>", "**", text, flags=re.I)
    text = re.sub(r"</?(i|em)>", "*", text, flags=re.I)
    text = re.sub(r"<br\s*/?>", "\n", text, flags=re.I)
    text = re.sub(r"</?(p|div|center|span|picture|source|h\d)\b[^>]*>", "\n", text, flags=re.I)
    text = re.sub(r"<[^>]+>", "", text)
    text = html.unescape(text)

    def absolute(m: re.Match) -> str:
        label, target = m[1], m[2].strip()
        if re.match(r"[a-z]+:", target):
            return m[0]
        if target.startswith("#"):
            return f"[{label}]({blob}README.md{target})"
        if target.startswith("../../"):   # the repository's own pages: releases, issues
            return f"[{label}]({blob.split('/blob/')[0]}/{target[6:]})"
        return f"[{label}]({blob}{target.lstrip('./')})"
    text = re.sub(r"(?<!!)\[([^\]]*)\]\(([^)\s]+)\)", absolute, text)
    text = re.sub(r"!\[[^\]]*\]\([^)]*\)", "", text)
    text = re.sub(r"\[\s*\]\([^)]*\)", "", text)   # badges whose picture went
    text = re.sub(r"^[ \t\u00a0]+$", "", text, flags=re.M)
    return re.sub(r"\n{3,}", "\n\n", text).strip()


def opening(md: str) -> tuple[str, bool]:
    """The README's start: before its second section, cut at a paragraph
    under MAX characters. True when something was left out."""
    sections = [m.start() for m in re.finditer(r"^## ", md, flags=re.M)]
    cut = sections[1] if len(sections) > 1 else len(md)
    if len(sections) == 1 and sections[0] > 400:
        cut = sections[0]
    part = md[:cut]
    if len(part) > MAX:
        part = part[:MAX].rsplit("\n\n", 1)[0]
    # No title left with nothing under it, nor a rule.
    part = re.sub(r"(\n(#{1,6} [^\n]*|---+)\s*)+$", "", "\n" + part.strip()).strip()
    return part, len(part) < len(md.strip())


def companion(repo: str) -> str | None:
    info = gh(f"repos/{repo}")
    sha = gh(f"repos/{repo}/commits/{info['default_branch']}")["sha"]
    try:
        readme = gh(f"repos/{repo}/readme?ref={sha}")
    except urllib.error.HTTPError:
        return None
    blob = f"https://github.com/{repo}/blob/{sha}/"
    text, more = opening(to_markdown(base64.b64decode(readme["content"]).decode("utf-8", "replace"), blob))
    if not text:
        return None
    licence = (info.get("license") or {}).get("spdx_id")
    licence = f", licence {licence}" if licence and licence != "NOASSERTION" else ""
    source = f"{blob}{readme['path']}"
    return (f"{MARK} {source} -->\n"
            f"> From the README of [{repo}](https://github.com/{repo}) at [{sha[:7]}]({source}){licence}.\n\n"
            f"{text}\n" + (f"\n[Read the whole README on GitHub]({source})\n" if more else ""))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    by_repo: dict[str, list[Path]] = {}
    for path in sorted(DEVICES.rglob("*.json")):
        try:
            source = (json.loads(path.read_text(encoding="utf-8")).get("_sources") or [{}])[0]
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
        m = re.match(r"https://github\.com/([^/]+/[^/#?]+)", source.get("url") or "")
        if source.get("type") == "vendor-doc" and m:
            by_repo.setdefault(m[1].removesuffix(".git"), []).append(path)
    written = 0
    for repo, paths in by_repo.items():
        todo = [p.with_suffix(".md") for p in paths]
        todo = [t for t in todo if not t.exists() or t.read_text(encoding="utf-8").startswith(MARK)]
        if not todo:
            continue
        try:
            text = companion(repo)
        except urllib.error.HTTPError as exc:
            print(f"{repo}: {exc}", file=sys.stderr)
            continue
        for t in todo:
            if text is None or (t.exists() and t.read_text(encoding="utf-8") == text):
                continue
            print(f"{'would write' if args.dry_run else 'wrote'} {t.relative_to(ROOT)}")
            if not args.dry_run:
                t.write_text(text, encoding="utf-8")
            written += 1
    print(f"{written} README(s) {'to write' if args.dry_run else 'written'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
