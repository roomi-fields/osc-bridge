//! OB-62b — every request of the online catalogue presents the machine's
//! account (SynthHub service contract, `crates/service/docs/INTERFACE.md`,
//! « Rien sans identifiant »):
//!
//! - the id goes in `X-Synthhub-Id` on every request that does not carry it
//!   in its path (`/u/<id>/…`);
//! - a machine without an id gets one first: `GET /v1/access/challenge`
//!   gives `{challenge, algorithm: "sha256", bits, deliver}`, the client finds
//!   the smallest decimal `nonce` such that SHA-256 of `<challenge>:<nonce>`
//!   starts with `bits` zero bits, and `POST <deliver>` with
//!   `{challenge, nonce}` answers `{id, user_id, mcp_path, tier}`;
//! - no id, or an unknown one: 401 whose body is the challenge itself; the
//!   client delivers, keeps the new id and replays the request once;
//! - a refused challenge is 403; a quota or issuing ceiling is 429 with
//!   `Retry-After` and a sentence in the body, shown to the person as it is.
//!
//! No network: the server is a fake one on a local port.

mod common;

use common::{catalogue, temp};
use osc_bridge::online::Online;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

const BITS: u32 = 8;
const CEILING_SENTENCE: &str = "Too many accounts issued from this address today: try again tomorrow.";
const QUOTA_SENTENCE: &str = "Daily quota reached: please update osc-bridge to carry on.";

#[derive(Clone, Debug)]
struct Req {
    method: String,
    url: String,
    id: Option<String>,
    if_none_match: Option<String>,
}

#[derive(Default)]
struct Knobs {
    /// Every id is unknown to the service (401 on all, even after delivery).
    reject_all: bool,
    /// Answer every non-access request with the quota sentence (429).
    quota: bool,
    /// Refuse every delivery (403).
    refuse_delivery: bool,
    /// The issuing ceiling is reached: the delivery answers 429 with a sentence.
    ceiling: bool,
    /// Where `/v1/trust` redirects to, when set.
    redirect_trust: Option<String>,
}

struct Fake {
    base: String,
    log: Arc<Mutex<Vec<Req>>>,
    known: Arc<Mutex<Vec<String>>>,
    knobs: Arc<Mutex<Knobs>>,
    deliveries: Arc<AtomicU32>,
}

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn zero_bits(bytes: &[u8]) -> u32 {
    let mut n = 0;
    for b in bytes {
        if *b == 0 { n += 8; } else { return n + b.leading_zeros(); }
    }
    n
}

impl Fake {
    fn start(root: &Path) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let fake = Fake {
            base,
            log: Arc::default(),
            known: Arc::default(),
            knobs: Arc::default(),
            deliveries: Arc::default(),
        };
        let (log, known, knobs, deliveries) = (fake.log.clone(), fake.known.clone(), fake.knobs.clone(), fake.deliveries.clone());
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let mut counter = 0u32;
            let challenge_doc = |n: u32| json!({ "challenge": format!("chal{n}"), "algorithm": "sha256", "bits": BITS,
                "deliver": "/v1/access/deliver" });
            for mut req in server.incoming_requests() {
                let url = req.url().to_string();
                let method = req.method().to_string();
                let id_header = req.headers().iter().find(|h| h.field.equiv("X-Synthhub-Id")).map(|h| h.value.to_string());
                let if_none_match = req.headers().iter().find(|h| h.field.equiv("If-None-Match")).map(|h| h.value.to_string());
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
                let path_id = url.strip_prefix("/u/").and_then(|r| r.split('/').next()).map(str::to_string);
                let id = path_id.clone().or(id_header.clone());
                log.lock().unwrap().push(Req { method: method.clone(), url: url.clone(), id: id_header, if_none_match: if_none_match.clone() });
                let respond = |req: tiny_http::Request, status: u16, body: Vec<u8>, extra: &[(&str, &str)]| {
                    let mut resp = tiny_http::Response::from_data(body).with_status_code(status);
                    for (k, v) in extra { resp.add_header(header(k, v)); }
                    let _ = req.respond(resp);
                };
                if url == "/v1/access/challenge" {
                    counter += 1;
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    respond(req, 200, challenge_doc(counter).to_string().into_bytes(), &[]);
                    continue;
                }
                if url == "/v1/access/deliver" && method == "POST" {
                    let doc: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    let proof = format!("{}:{}", doc["challenge"].as_str().unwrap_or(""), doc["nonce"].as_str().map(str::to_string)
                        .or_else(|| doc["nonce"].as_u64().map(|n| n.to_string())).unwrap_or_default());
                    let digest = ring::digest::digest(&ring::digest::SHA256, proof.as_bytes());
                    if knobs.lock().unwrap().ceiling {
                        respond(req, 429, CEILING_SENTENCE.as_bytes().to_vec(), &[("Retry-After", "86400")]);
                        continue;
                    }
                    if knobs.lock().unwrap().refuse_delivery || !doc["challenge"].as_str().is_some_and(|c| c.starts_with("chal"))
                        || zero_bits(digest.as_ref()) < BITS {
                        respond(req, 403, b"challenge refused".to_vec(), &[]);
                        continue;
                    }
                    let n = deliveries.fetch_add(1, Ordering::SeqCst) + 1;
                    let id = format!("{n:032x}");
                    known.lock().unwrap().push(id.clone());
                    respond(req, 200, json!({ "id": id, "user_id": id, "mcp_path": format!("/u/{id}/mcp"), "tier": "free" })
                        .to_string().into_bytes(), &[]);
                    continue;
                }
                let (reject_all, quota, redirect) = {
                    let k = knobs.lock().unwrap();
                    (k.reject_all, k.quota, k.redirect_trust.clone())
                };
                let accepted = id.as_ref().is_some_and(|i| known.lock().unwrap().contains(i)) && !reject_all;
                if !accepted {
                    counter += 1;
                    respond(req, 401, challenge_doc(counter).to_string().into_bytes(), &[]);
                    continue;
                }
                if quota {
                    respond(req, 429, QUOTA_SENTENCE.as_bytes().to_vec(), &[("Retry-After", "3600")]);
                    continue;
                }
                if let (Some(to), true) = (redirect, url == "/v1/trust") {
                    respond(req, 302, Vec::new(), &[("Location", &to)]);
                    continue;
                }
                let (status, out, etag): (u16, Vec<u8>, Option<String>) = if let Some(rel) = url.strip_prefix("/api/v1/devices/") {
                    match std::fs::read(root.join(rel)) {
                        Ok(bytes) => {
                            let etag = format!("\"{}\"", common::sha256_hex(&bytes));
                            if if_none_match.as_deref() == Some(etag.as_str()) { (304, Vec::new(), Some(etag)) } else { (200, bytes, Some(etag)) }
                        }
                        Err(_) => (404, b"not found".to_vec(), None),
                    }
                } else if url == "/v1/trust" {
                    (200, br#"{"devices":{}}"#.to_vec(), None)
                } else if url == "/v1/detect" && method == "GET" {
                    (200, br#"{"probes":[]}"#.to_vec(), None)
                } else if url == "/v1/detect" {
                    (200, json!({ "ports": [{ "port": "p", "candidates": [] }] }).to_string().into_bytes(), None)
                } else if url == "/v1/librarian" || url.starts_with("/v1/librarian/") || url.starts_with("/v1/presets/") {
                    (200, b"{}".to_vec(), None)
                } else if let Some(slug) = url.strip_prefix("/v1/devices/") {
                    (200, json!({ "best": format!("acme/{slug}"), "variants": [{ "id": format!("acme/{slug}"), "slug": slug }] })
                        .to_string().into_bytes(), None)
                } else if url == "/mcp" || url.ends_with("/mcp") {
                    (200, json!({ "jsonrpc": "2.0", "id": 1, "result": { "content": [] } }).to_string().into_bytes(), None)
                } else {
                    (404, b"{}".to_vec(), None)
                };
                let mut extra = Vec::new();
                if let Some(e) = &etag { extra.push(("ETag", e.as_str())); }
                respond(req, status, out, &extra);
            }
        });
        fake
    }

    fn requests(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }

    /// The requests that are not part of obtaining an id.
    fn catalogue_requests(&self) -> Vec<Req> {
        self.requests().into_iter().filter(|r| !r.url.starts_with("/v1/access/")).collect()
    }

    fn forget(&self) {
        self.log.lock().unwrap().clear();
    }
}

fn client(fake: &Fake, dir: &Path) -> (Online, PathBuf) {
    let account = dir.join("config/user");
    (Online::keeping_account_in(&fake.base, Some(account.clone())), account)
}

fn kept(account: &Path) -> String {
    std::fs::read_to_string(account).unwrap().trim().to_string()
}

fn setup(tag: &str) -> (tempfile::TempDir, Fake) {
    let dir = temp(tag);
    catalogue(&dir.path().join("server"));
    let fake = Fake::start(&dir.path().join("server"));
    (dir, fake)
}

#[test]
fn criterion_4_every_family_presents_the_account_in_the_same_header() {
    let (dir, fake) = setup("acct-families");
    let (online, account) = client(&fake, dir.path());
    let cache = dir.path().join("cache");

    online.fetch_driver("acme/synth", &cache).unwrap();
    online.variants("acme/synth").unwrap();
    online.variants("synth").unwrap();
    online.probes().unwrap();
    online.detect(&[json!({ "port": "p", "replies_hex": [] })]).unwrap();
    online.trust().unwrap();
    online.librarian_synths().unwrap();
    online.librarian("recognize", &json!({})).unwrap();
    online.preset_messages("abc", Some(1)).unwrap();
    online.catalogue_call("list_devices", &json!({})).unwrap();

    let id = kept(&account);
    assert!(osc_bridge::online::is_user_id(&id), "{id}");
    let seen = fake.catalogue_requests();
    for url in ["/api/v1/devices/acme/synth.json", "/api/v1/devices/acme/synth.md", "/v1/devices/acme/synth", "/v1/devices/synth",
        "/v1/detect", "/v1/trust", "/v1/librarian", "/v1/librarian/recognize", "/v1/presets/abc/messages?channel=1", "/mcp"] {
        assert!(seen.iter().any(|r| r.url == url), "no request to {url}: {seen:?}");
    }
    assert!(seen.iter().any(|r| r.url == "/v1/detect" && r.method == "GET"));
    assert!(seen.iter().any(|r| r.url == "/v1/detect" && r.method == "POST"));
    for r in &seen {
        assert_eq!(r.id.as_deref(), Some(id.as_str()), "{} {} presents the account: {seen:?}", r.method, r.url);
    }
}

#[test]
fn criterion_2_a_machine_without_account_obtains_one_before_its_first_request() {
    let (dir, fake) = setup("acct-first");
    let (online, account) = client(&fake, dir.path());
    assert!(!account.exists());
    online.trust().unwrap();

    let all = fake.requests();
    assert_eq!((all[0].method.as_str(), all[0].url.as_str()), ("GET", "/v1/access/challenge"), "{all:?}");
    assert_eq!((all[1].method.as_str(), all[1].url.as_str()), ("POST", "/v1/access/deliver"), "{all:?}");
    assert_eq!(fake.catalogue_requests().len(), 1, "the request goes once, with the account: {all:?}");
    assert_eq!(fake.deliveries.load(Ordering::SeqCst), 1);
    assert_eq!(kept(&account), fake.known.lock().unwrap()[0], "the delivered id is the one kept");

    // The kept account is presented later, without a new delivery.
    fake.forget();
    online.trust().unwrap();
    assert!(fake.requests().iter().all(|r| !r.url.starts_with("/v1/access/")), "{:?}", fake.requests());
    assert_eq!(fake.deliveries.load(Ordering::SeqCst), 1);
}

#[test]
fn criterion_2_a_kept_account_is_presented_without_asking_for_another() {
    let (dir, fake) = setup("acct-kept");
    let (first, account) = client(&fake, dir.path());
    first.trust().unwrap();
    let id = kept(&account);
    fake.forget();
    let (again, _) = client(&fake, dir.path());
    again.trust().unwrap();
    assert_eq!(fake.requests().len(), 1, "{:?}", fake.requests());
    assert_eq!(fake.requests()[0].id.as_deref(), Some(id.as_str()));
}

#[test]
fn criterion_5_the_mirror_presents_the_account_on_a_conditional_request_and_serves_a_304() {
    let (dir, fake) = setup("acct-mirror");
    let cache = dir.path().join("cache");
    let (online, account) = client(&fake, dir.path());
    online.fetch_driver("acme/synth", &cache).unwrap();
    let id = kept(&account);
    let before = std::fs::read(cache.join("acme/synth.json")).unwrap();

    fake.forget();
    let (later, _) = client(&fake, dir.path());
    let path = later.fetch_driver("acme/synth", &cache).unwrap();
    let seen = fake.catalogue_requests();
    assert_eq!(seen.len(), 2, "{seen:?}");
    for r in &seen {
        assert!(r.if_none_match.is_some(), "conditional: {r:?}");
        assert_eq!(r.id.as_deref(), Some(id.as_str()), "{r:?}");
    }
    assert_eq!(std::fs::read(path).unwrap(), before, "the kept driver is served on 304");
}

#[test]
fn criterion_6_an_unknown_account_is_replaced_and_the_request_replayed_once() {
    let (dir, fake) = setup("acct-unknown");
    let (online, account) = client(&fake, dir.path());
    std::fs::create_dir_all(account.parent().unwrap()).unwrap();
    let stale = "f".repeat(32);
    std::fs::write(&account, &stale).unwrap();

    online.trust().unwrap();
    let seen = fake.requests();
    let urls: Vec<_> = seen.iter().map(|r| (r.method.as_str(), r.url.as_str())).collect();
    assert_eq!(urls, [("GET", "/v1/trust"), ("GET", "/v1/access/challenge"), ("POST", "/v1/access/deliver"), ("GET", "/v1/trust")], "{seen:?}");
    assert_eq!(seen[0].id.as_deref(), Some(stale.as_str()));
    let fresh = kept(&account);
    assert_ne!(fresh, stale);
    assert_eq!(seen[3].id.as_deref(), Some(fresh.as_str()));
}

#[test]
fn criterion_6_a_conditional_mirror_request_is_replayed_too() {
    let (dir, fake) = setup("acct-unknown-mirror");
    let cache = dir.path().join("cache");
    let (online, account) = client(&fake, dir.path());
    online.fetch_driver("acme/synth", &cache).unwrap();
    fake.known.lock().unwrap().clear(); // the service forgot the account
    fake.forget();
    let path = online.fetch_driver("acme/synth", &cache).unwrap();
    assert!(path.is_file());
    let seen = fake.requests();
    assert_eq!(seen.iter().filter(|r| r.url == "/api/v1/devices/acme/synth.json").count(), 2, "{seen:?}");
    assert_eq!(fake.deliveries.load(Ordering::SeqCst), 2);
    assert_eq!(kept(&account), fake.known.lock().unwrap()[0]);
}

#[test]
fn criterion_6_when_the_replay_fails_too_the_person_sees_the_answer_without_another_replay() {
    let (dir, fake) = setup("acct-replay-fails");
    fake.knobs.lock().unwrap().reject_all = true;
    let (online, _) = client(&fake, dir.path());

    let err = online.trust().unwrap_err().to_string();
    assert!(err.contains("401"), "{err}");
    let trust = fake.requests().iter().filter(|r| r.url == "/v1/trust").count();
    assert_eq!(trust, 2, "one request and one replay: {:?}", fake.requests());
    assert_eq!(fake.deliveries.load(Ordering::SeqCst), 1, "a single delivery");
}

#[test]
fn criterion_7_a_failed_delivery_fails_the_request_naming_the_delivery_and_sends_nothing_without_account() {
    let (dir, fake) = setup("acct-delivery-fails");
    fake.knobs.lock().unwrap().refuse_delivery = true;
    let (online, account) = client(&fake, dir.path());

    for result in [online.trust().map(drop), online.variants("synth").map(drop), online.probes().map(drop)] {
        let err = result.unwrap_err().to_string().to_lowercase();
        assert!(err.contains("deliver"), "the message names the delivery: {err}");
    }
    assert!(fake.catalogue_requests().is_empty(), "nothing left without an account: {:?}", fake.requests());
    assert!(!account.exists() || kept(&account).is_empty());
}

#[test]
fn criterion_7_two_simultaneous_requests_on_a_machine_without_account_deliver_one() {
    let (dir, fake) = setup("acct-concurrent");
    let (online, account) = client(&fake, dir.path());
    let online = Arc::new(online);
    let handles: Vec<_> = (0..2).map(|i| {
        let online = online.clone();
        std::thread::spawn(move || if i == 0 { online.trust().map(drop) } else { online.probes().map(drop) })
    }).collect();
    for h in handles { h.join().unwrap().unwrap(); }
    assert_eq!(fake.deliveries.load(Ordering::SeqCst), 1, "{:?}", fake.requests());
    let id = kept(&account);
    for r in fake.catalogue_requests() {
        assert_eq!(r.id.as_deref(), Some(id.as_str()), "{r:?}");
    }
}

#[test]
fn criterion_3_the_quota_sentence_of_the_service_reaches_the_person() {
    let (dir, fake) = setup("acct-quota");
    let (online, _) = client(&fake, dir.path());
    online.trust().unwrap();
    fake.knobs.lock().unwrap().quota = true;
    let cache = dir.path().join("cache");

    let errors = [
        online.trust().unwrap_err(),
        online.variants("synth").unwrap_err(),
        online.probes().unwrap_err(),
        online.detect(&[json!({ "port": "p", "replies_hex": [] })]).unwrap_err(),
        online.fetch_driver("acme/synth", &cache).unwrap_err(),
        online.librarian_synths().unwrap_err(),
        online.catalogue_call("list_devices", &json!({})).unwrap_err(),
    ];
    for e in errors {
        assert!(format!("{e:#}").contains(QUOTA_SENTENCE), "{e:#}");
    }
    let trusts = fake.requests().iter().filter(|r| r.url == "/v1/trust").count();
    assert_eq!(trusts, 2, "a 429 is not replayed");
}

#[test]
fn criterion_3_the_issuing_ceiling_sentence_reaches_the_person() {
    let (dir, fake) = setup("acct-ceiling");
    let (online, account) = client(&fake, dir.path());
    fake.knobs.lock().unwrap().ceiling = true;

    let e = online.trust().unwrap_err();
    assert!(format!("{e:#}").contains(CEILING_SENTENCE), "{e:#}");
    assert!(!account.exists(), "no account kept when the delivery is refused");
    assert!(fake.catalogue_requests().iter().all(|r| r.id.is_none()), "{:?}", fake.requests());
    assert_eq!(fake.deliveries.load(Ordering::SeqCst), 0);
}

#[test]
fn criterion_8_the_account_is_not_followed_to_another_host() {
    let (dir, other) = setup("acct-redirect-other");
    let (dir2, fake) = setup("acct-redirect");
    let (online, account) = client(&fake, dir2.path());
    online.trust().unwrap();
    let id = kept(&account);
    // The other host accepts any id and records what it receives.
    other.known.lock().unwrap().push(id.clone());
    fake.knobs.lock().unwrap().redirect_trust = Some(format!("{}/v1/trust", other.base));

    let _ = online.trust();
    let elsewhere = other.requests();
    assert!(!elsewhere.is_empty(), "the redirect was followed: {:?}", fake.requests());
    assert!(elsewhere.iter().all(|r| r.id.is_none()), "the account stays at the base address: {elsewhere:?}");
    drop(dir);
}

#[test]
fn criterion_9_the_texts_no_longer_say_the_requests_carry_no_account() {
    let settings = osc_bridge::online::Settings::default().render();
    assert!(!settings.contains("without your account"), "{settings}");

    let dir = temp("acct-text");
    let index = osc_bridge::mcp::DeviceIndex::build(dir.path());
    let text = osc_bridge::mcp::privacy(&dir.path().join("settings.toml"), &index, None, None, &[], &[]).unwrap();
    assert!(!text.contains("without your account"), "{text}");
}
