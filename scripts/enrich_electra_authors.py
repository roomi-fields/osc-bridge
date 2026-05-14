#!/usr/bin/env python3
"""
Walk every device JSON whose first `_sources[]` entry is an Electra preset,
resolve the original author's display name via Firestore (`/projects/{id}` →
`userId`, then `/users/{uid}` → `name`), and add an `author` field to the
source record. Skips devices already enriched.

Usage:
    python3 scripts/enrich_electra_authors.py
"""
from __future__ import annotations
import json
import sys
import time
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEVICES_DIR = REPO_ROOT / "devices"
TOKEN_FILE = Path("/tmp/electra_firebase_token.json")  # written by recon scripts
SOURCES_TOML = REPO_ROOT / "sources.toml"


def _login() -> str:
    """Re-login if no token cached. Reads creds from sources.toml's auth_file."""
    if TOKEN_FILE.is_file():
        try:
            tok = json.loads(TOKEN_FILE.read_text())
            return tok["idToken"]
        except Exception:
            pass
    # Fresh login — reuse the firestore source driver so the API key stays
    # out of the repo (it's discovered from Electra's published JS bundle).
    import tomllib
    import sys as _sys
    _sys.path.insert(0, str(REPO_ROOT))
    from scripts.sources.firestore import _resolve_firebase_api_key
    cfg = tomllib.loads(SOURCES_TOML.read_text())
    src = next(s for s in cfg["source"] if s.get("name") == "electra-one-public")
    api_key = src.get("firebase_api_key") or _resolve_firebase_api_key(src)
    auth = json.loads(Path(src["auth_file"]).expanduser().read_text())
    body = json.dumps({"email": auth["email"], "password": auth["password"], "returnSecureToken": True}).encode()
    req = urllib.request.Request(
        f"https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key={api_key}",
        data=body, headers={"Content-Type": "application/json"})
    r = urllib.request.urlopen(req, timeout=30)
    data = json.loads(r.read())
    TOKEN_FILE.write_text(json.dumps(data, indent=2))
    return data["idToken"]


def _firestore_get(token: str, path: str) -> dict | None:
    url = f"https://firestore.googleapis.com/v1/projects/electra-one-716c4/databases/(default)/documents/{path}"
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    try:
        return json.loads(urllib.request.urlopen(req, timeout=15).read())
    except urllib.error.HTTPError:
        return None


def _string_field(doc: dict, key: str) -> str | None:
    return (doc.get("fields", {}) or {}).get(key, {}).get("stringValue")


def main():
    token = _login()
    user_cache: dict[str, str] = {}  # uid → display name
    device_files = []
    for p in DEVICES_DIR.rglob("*.json"):
        try:
            d = json.loads(p.read_text())
        except Exception:
            continue
        sources = d.get("_sources") or []
        electra_idx = None
        for i, s in enumerate(sources):
            if isinstance(s, dict) and s.get("type") == "electra-preset":
                electra_idx = i; break
        if electra_idx is None:
            continue
        if sources[electra_idx].get("author"):  # already enriched
            continue
        device_files.append((p, d, electra_idx))

    print(f"devices to enrich: {len(device_files)}")
    enriched = 0
    for p, d, idx in device_files:
        src = d["_sources"][idx]
        pid = src.get("preset_id")
        if not pid:
            continue
        # Step 1: project doc → userId
        proj = _firestore_get(token, f"projects/{pid}?mask.fieldPaths=userId")
        if not proj:
            continue
        uid = _string_field(proj, "userId")
        if not uid:
            continue
        # Step 2: user doc → name
        if uid not in user_cache:
            user_doc = _firestore_get(token, f"users/{uid}?mask.fieldPaths=name&mask.fieldPaths=displayName")
            if user_doc:
                user_cache[uid] = (
                    _string_field(user_doc, "displayName")
                    or _string_field(user_doc, "name")
                    or f"user:{uid[:8]}"
                )
            else:
                user_cache[uid] = f"user:{uid[:8]}"
            time.sleep(0.05)
        author = user_cache[uid]
        src["author"] = author
        # NOTE: the Firestore userId (uid) is deliberately NOT written to the
        # device JSON. It's an internal account identifier — re-publishing it
        # in a public repo is a privacy concern even for publicly-shared
        # presets. Only the human-readable display name is kept, as credit.
        p.write_text(json.dumps(d, indent=2) + "\n")
        enriched += 1
        if enriched % 50 == 0:
            print(f"  {enriched}/{len(device_files)} done; {len(user_cache)} unique authors")
        time.sleep(0.03)

    print(f"\n=== enriched {enriched} devices, {len(user_cache)} unique authors ===")
    # Save the user cache for reference
    Path('/tmp/electra_authors.json').write_text(json.dumps(user_cache, indent=2))


if __name__ == "__main__":
    main()
