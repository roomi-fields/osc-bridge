"""Firestore source: authenticate via Firebase, list a collection filtered by
a predicate, download each matching doc, extract the primary payload field,
and save as JSON into the cache directory.

This driver is tailored for Electra One's preset collection layout:
    /projects/{id} with fields { name, isPublic, userId, revision, project:<string> }
The `project` field is a stringified JSON blob of the preset.

Config (from sources.toml):
    firestore_project, firebase_api_key, collection, filter, auth_file,
    cache_dir, incremental
"""
from __future__ import annotations
import json
import os
import re
import time
import urllib.request
import urllib.error
from pathlib import Path  # noqa: I001


def _slug(s: str) -> str:
    s = s.lower().strip()
    s = re.sub(r"[^a-z0-9]+", "_", s).strip("_")
    return s or "unnamed"


def _read_auth(auth_file: str) -> dict:
    p = Path(auth_file).expanduser()
    if not p.is_file():
        raise FileNotFoundError(
            f"auth file missing: {p} — create it with {{\"email\":..., \"password\":...}}"
        )
    return json.loads(p.read_text())


def _resolve_firebase_api_key(cfg: dict) -> str:
    """Return the Firebase API key. Either explicit (env or cfg field) or
    discovered by scraping it out of the source's published web bundle so we
    don't have to commit it."""
    # Allow override via env (CI etc.)
    if os.environ.get("FIREBASE_API_KEY"):
        return os.environ["FIREBASE_API_KEY"]
    # Cached for the session
    cache = Path(f"/tmp/_firebase_key_{cfg.get('name','default')}.txt")
    if cache.is_file():
        return cache.read_text().strip()
    home = cfg.get("firebase_api_key_source")
    if not home:
        raise ValueError(
            "no firebase_api_key_source in source config; set FIREBASE_API_KEY env var "
            "or add `firebase_api_key_source = '<vendor app URL>'` to sources.toml"
        )
    # Scrape: fetch index, find /_nuxt/*.js refs, grep each for the AIza* key
    pat = re.compile(r"AIza[A-Za-z0-9_\-]{35}")
    js_pat = re.compile(r"/_nuxt/[A-Za-z0-9._/-]+\.js")
    try:
        index = urllib.request.urlopen(home, timeout=15).read().decode("utf-8", errors="ignore")
    except Exception as e:
        raise RuntimeError(f"could not fetch {home}: {e}")
    # First try the index itself
    m = pat.search(index)
    if m:
        cache.write_text(m.group(0))
        return m.group(0)
    # Walk the JS bundles
    bundles = sorted(set(js_pat.findall(index)))
    for b in bundles:
        full = home.rstrip("/") + b
        try:
            body = urllib.request.urlopen(full, timeout=15).read().decode("utf-8", errors="ignore")
        except Exception:
            continue
        m = pat.search(body)
        if m:
            cache.write_text(m.group(0))
            return m.group(0)
    raise RuntimeError(
        f"could not discover Firebase API key from {home} — set FIREBASE_API_KEY env var manually"
    )


def _firebase_login(api_key: str, email: str, password: str) -> str:
    body = json.dumps({
        "email": email, "password": password, "returnSecureToken": True,
    }).encode()
    req = urllib.request.Request(
        f"https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key={api_key}",
        data=body, headers={"Content-Type": "application/json"},
    )
    r = urllib.request.urlopen(req, timeout=30)
    return json.loads(r.read())["idToken"]


def _firestore_list(base: str, col: str, id_token: str,
                    mask_fields: list[str] | None = None) -> list[dict]:
    """Paginate through a top-level Firestore collection and return raw docs."""
    mask = ""
    if mask_fields:
        mask = "&" + "&".join(f"mask.fieldPaths={f}" for f in mask_fields)
    all_docs, token = [], None
    while True:
        url = f"{base}/{col}?pageSize=300{mask}"
        if token:
            url += f"&pageToken={token}"
        req = urllib.request.Request(url, headers={"Authorization": f"Bearer {id_token}"})
        r = urllib.request.urlopen(req, timeout=30)
        data = json.loads(r.read())
        all_docs.extend(data.get("documents", []))
        token = data.get("nextPageToken")
        if not token:
            break
        time.sleep(0.05)
    return all_docs


def _get_field(fields: dict, key: str):
    v = fields.get(key)
    if v is None:
        return None
    for kind in ("stringValue", "booleanValue", "integerValue", "doubleValue",
                 "timestampValue"):
        if kind in v:
            return v[kind]
    return v


def _matches_filter(fields: dict, predicate: str) -> bool:
    """Very small filter DSL: 'field == value' or 'field != value'.
    `value` parsed as JSON literal.
    """
    m = re.match(r"^\s*(\w+)\s*(==|!=)\s*(.+)\s*$", predicate or "")
    if not m:
        return True
    field, op, raw = m.group(1), m.group(2), m.group(3).strip()
    try:
        want = json.loads(raw)
    except Exception:
        want = raw
    got = _get_field(fields, field)
    return (got == want) if op == "==" else (got != want)


def fetch(cfg: dict, repo_root: Path) -> dict:
    cache = (repo_root / cfg["cache_dir"]).resolve()
    cache.mkdir(parents=True, exist_ok=True)
    auth = _read_auth(cfg["auth_file"])
    api_key = cfg.get("firebase_api_key") or _resolve_firebase_api_key(cfg)
    token = _firebase_login(api_key, auth["email"], auth["password"])
    project = cfg["firestore_project"]
    base = f"https://firestore.googleapis.com/v1/projects/{project}/databases/(default)/documents"
    collection = cfg["collection"]
    predicate = cfg.get("filter", "")
    incremental = bool(cfg.get("incremental", True))

    # Phase 1 — list with a light mask
    print(f"  listing /{collection}…")
    docs = _firestore_list(base, collection, token,
                           mask_fields=["name", "isPublic", "revision", "targetDevice"])
    print(f"  total docs: {len(docs)}")
    # Phase 2 — filter
    kept = []
    for d in docs:
        fields = d.get("fields", {})
        if _matches_filter(fields, predicate):
            kept.append(d)
    print(f"  after filter '{predicate}': {len(kept)}")

    # Phase 3 — fetch full contents (with incremental skip)
    fetched, skipped, errors = 0, 0, []
    for d in kept:
        pid = d["name"].split("/")[-1]
        fields = d.get("fields", {})
        name = _get_field(fields, "name") or pid
        revision = _get_field(fields, "revision") or 0
        slug = _slug(name)[:40]
        out = cache / f"{slug}__{pid}.json"
        # Incremental: skip if same revision already cached
        if incremental and out.is_file():
            try:
                cached = json.loads(out.read_text())
                if cached.get("_cache_meta", {}).get("revision") == revision:
                    skipped += 1
                    continue
            except Exception:
                pass
        # Full fetch
        url = f"{base}/{collection}/{pid}"
        req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
        try:
            r = urllib.request.urlopen(req, timeout=30)
            doc = json.loads(r.read())
            project_str = _get_field(doc.get("fields", {}), "project") or ""
            if not project_str:
                skipped += 1
                continue
            body = json.loads(project_str)
            body["_cache_meta"] = {
                "source": cfg["name"], "id": pid, "revision": revision,
                "name": name, "fetched_at": time.strftime("%Y-%m-%d"),
            }
            out.write_text(json.dumps(body, indent=2))
            fetched += 1
        except Exception as e:
            errors.append(f"{pid}: {e}")
        time.sleep(0.03)
    print(f"  fetched: {fetched}  skipped: {skipped}  errors: {len(errors)}")
    return {
        "fetched": fetched, "skipped": skipped, "errors": errors,
        "cache_dir": cache,
    }
