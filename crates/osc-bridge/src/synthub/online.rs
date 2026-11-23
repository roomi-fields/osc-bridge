//! The link between a copy of osc-bridge on the user's machine and the hosted
//! server (`SERVER`), so that every install shares in, and adds to, what the
//! community builds there:
//!
//! - the drivers: the catalogue lives on the server and is handed out one
//!   driver at a time, never whole. A local copy asks for the driver it needs
//!   — by id (`GET /api/v1/devices/<id>.json`, its notes `<id>.md`), by slug
//!   (`GET /v1/devices/<slug>`: the device's variants, best first) or for
//!   the MIDI ports it found (`POST /v1/detect`) — and keeps it in a cache
//!   (`cache_dir`), which is all it has offline. Each kept file has the ETag
//!   the server gave beside it (`<file>.etag`); the next fetch sends it in
//!   `If-None-Match`, and `304 Not Modified` leaves the kept file as it is.
//!   These requests present the account kept on this machine;
//! - trust: how far other users confirmed each driver (`GET /v1/trust`);
//! - reports and contributions: `report_result`, `report_mapping`,
//!   `get_trust`, `submit_driver`, `submit_fix` and `get_contribution` are
//!   forwarded to the server's MCP endpoint under the account kept on this
//!   machine (`account_file`, the same one `osc-bridge connect` uses).
//!
//! The server serves nothing without an account. Every request goes through
//! `Online::send`, which presents the account in `X-Synthhub-Id` (a request to
//! `/u/<id>/…` carries it in its path), only to the base address. A machine
//! without one has it delivered first (`GET /v1/access/challenge`, a proof of
//! work, `POST` of the answer); a 401 gets a new account and one replay; a 429
//! sentence of the server is the error shown to the person.
//!
//! A reply marking this osc-bridge as deprecated (`Deprecation` with
//! `Link: <url>; rel="deprecation"`, RFC 9745) is told once per run: on
//! standard error, and in the next MCP tool result (`newer_notice_once`).
//!
//! The MIDI ports never leave the machine. Sharing is on by default; the
//! user turns it off with `osc-bridge privacy` (`Settings`): `--offline on`
//! cuts the link (only the drivers already cached are used, nothing is sent),
//! `--private <device>` keeps one device's reports on the machine.
//! `osc-bridge mcp --offline` and `OSC_BRIDGE_OFFLINE=1` cut it for one run.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

pub const SERVER: &str = "https://mcp.synthhub.roomi-fields.com";

/// The tools a local copy forwards to the server.
pub const FORWARDED_TOOLS: [&str; 18] = ["report_result", "report_mapping", "get_trust",
    "submit_driver", "submit_fix", "get_contribution",
    "share_preset", "update_preset", "find_presets", "get_preset", "report_preset", "withdraw_preset",
    "comment_preset", "star_preset", "preset_collection", "follow_author", "set_profile", "link_account"];

/// MIDI ports matched per `POST /v1/detect` request.
pub const PORTS_PER_REQUEST: usize = 32;

/// Whether the user cut the link (`OSC_BRIDGE_OFFLINE`, any value but 0).
pub fn offline_by_env() -> bool {
    std::env::var("OSC_BRIDGE_OFFLINE").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Whether this machine is linked to the server: neither the environment
/// nor the settings file cut the link.
pub fn linked() -> bool {
    !offline_by_env() && !settings_file().and_then(|f| Settings::load(Some(&f)).ok()).is_some_and(|s| s.offline)
}

fn dir_from(env: &[&str], home_sub: &str) -> Option<PathBuf> {
    env.iter().find_map(|k| std::env::var_os(k).map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(home_sub)))
}

/// Where the account id is kept: `$XDG_CONFIG_HOME`, `%APPDATA%` or
/// `~/.config`, under `osc-bridge/user`.
pub fn account_file() -> Option<PathBuf> {
    Some(dir_from(&["XDG_CONFIG_HOME", "APPDATA"], ".config")?.join("osc-bridge").join("user"))
}

/// Where the privacy settings are kept: next to the account, in
/// `osc-bridge/settings.toml`.
pub fn settings_file() -> Option<PathBuf> {
    Some(account_file()?.with_file_name("settings.toml"))
}

/// The user's privacy choices on this machine: a commented file the user may
/// edit by hand, or through `osc-bridge privacy`. The default shares: linked
/// to the server, no device private.
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    /// Cut the link to the server.
    #[serde(default)]
    pub offline: bool,
    /// Devices whose reports, fixes and trust lookups stay on this machine:
    /// a catalogue id (`korg/minilogue`), a slug (`minilogue`, every variant
    /// of the device) or a whole vendor (`korg/*`).
    #[serde(default)]
    pub private: Vec<String>,
}

const SETTINGS_HELP: &str = "\
# osc-bridge — what this machine shares with the SynthHub server.
# Edit this file freely, or use `osc-bridge privacy`. Read when osc-bridge starts.
#
# Sharing is on by default: the drivers your devices need come from the
# server, one at a time, as other users and agents
# fixed and confirmed them; your reports and fixes go to the server under an
# anonymous account. MIDI ports and the traffic of your devices never leave
# this machine.
";

impl Settings {
    /// The settings at `path`, or the defaults when there are none.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else { return Ok(Self::default()) };
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("read {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    /// The file's text: the settings, each with what it does.
    pub fn render(&self) -> String {
        let list = self.private.iter().map(|p| format!("  {},\n", toml::Value::String(p.clone()))).collect::<String>();
        format!("{SETTINGS_HELP}
# true: do not contact the server at all (drivers already cached only, nothing sent).
offline = {}

# Devices kept private: nothing about them is sent to the server (reports,
# fixes, trust lookups). Each entry is a catalogue id (\"korg/minilogue\"), a
# slug for every variant of a device (\"minilogue\"), or a vendor (\"korg/*\").
private = [
{list}]
", self.offline)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        std::fs::write(path, self.render()).with_context(|| format!("write {}", path.display()))
    }

    /// The settings at `path`, writing the defaults there first when the
    /// file does not exist yet, so the user finds it ready to edit.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if !path.exists() {
            Self::default().save(path)?;
        }
        Self::load(Some(path))
    }

    /// Whether `private` covers a driver, by its id and slug.
    pub fn covers(private: &[String], id: &str, slug: &str) -> bool {
        private.iter().any(|p| match p.strip_suffix("/*") {
            Some(vendor) => id.starts_with(&format!("{vendor}/")),
            None => p == id || p == slug,
        })
    }
}

/// Where the drivers fetched from the server are kept: `$XDG_CACHE_HOME`,
/// `%LOCALAPPDATA%` or `~/.cache`, under `osc-bridge/drivers`, each at its
/// id (`korg/minilogue.json`, its notes `korg/minilogue.md`).
pub fn cache_dir() -> Option<PathBuf> {
    Some(dir_from(&["XDG_CACHE_HOME", "LOCALAPPDATA"], ".cache")?.join("osc-bridge").join("drivers"))
}

/// Where the ETag the server gave with a kept file is kept: beside it, in
/// `<file>.etag`.
fn etag_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".etag");
    PathBuf::from(name)
}

/// Where a newer osc-bridge is, once a reply of the server marked this one
/// as deprecated.
static NEWER: OnceLock<String> = OnceLock::new();
/// Whether an MCP tool result already carried the notice.
static NEWER_TOLD: AtomicBool = AtomicBool::new(false);

fn newer_notice(url: &str) -> String {
    format!("A newer osc-bridge exists: {url} (the SynthHub server marks this one, {}, as deprecated).",
        env!("CARGO_PKG_VERSION"))
}

/// Whether a `Deprecation` value (RFC 9745: `@<Unix seconds>`) announces a
/// withdrawal at `now`: a date still to come does not; a date reached, or a
/// value that is not a date, does.
fn deprecation_announces(value: &str, now: i64) -> bool {
    match value.trim().strip_prefix('@').and_then(|s| s.parse::<i64>().ok()) {
        Some(at) => at <= now,
        None => true,
    }
}

/// Note a reply's `Deprecation` with its `Link rel="deprecation"`: told on
/// standard error the first time in this run.
fn note_deprecation(headers: &ureq::http::HeaderMap) {
    let Some(value) = headers.get("deprecation") else { return };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    if !deprecation_announces(value.to_str().unwrap_or(""), now) { return; }
    let link = headers.get_all("link").iter().filter_map(|v| v.to_str().ok()).find_map(deprecation_link);
    if let Some(url) = link {
        if NEWER.set(url.clone()).is_ok() {
            eprintln!("{}", newer_notice(&url));
        }
    }
}

/// The target of the `rel="deprecation"` link in a `Link` header (RFC 8288).
fn deprecation_link(header: &str) -> Option<String> {
    header.split(',').find_map(|link| {
        let (target, params) = link.trim().strip_prefix('<')?.split_once('>')?;
        params.split(';').any(|p| p.split_once('=').is_some_and(|(k, v)| k.trim().eq_ignore_ascii_case("rel")
            && v.trim().trim_matches('"').split_ascii_whitespace().any(|r| r.eq_ignore_ascii_case("deprecation"))))
            .then(|| target.to_string())
    })
}

/// The notice that a newer osc-bridge exists and where, for the next MCP
/// tool result: once per run, after a reply marked this one as deprecated.
pub fn newer_notice_once() -> Option<String> {
    let url = NEWER.get()?;
    (!NEWER_TOLD.swap(true, Ordering::SeqCst)).then(|| newer_notice(url))
}

pub fn is_user_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// A driver id as the catalogue names it (`korg/minilogue`,
/// `arturia/microfreak.vendor.fw-1.0`): a maker folder and a file name
/// without `.json`, nothing that could leave the cache.
pub fn is_driver_id(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    parts.len() == 2 && parts.iter().all(|c| !c.is_empty() && !c.starts_with('.')
        && c.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b)))
}

/// A file the server gave: its text, the ETag it came with, and whether it is
/// the server's new copy (`fresh`) rather than the one already kept.
struct Got {
    text: String,
    etag: Option<String>,
    fresh: bool,
}

/// `path` with `suffix` appended to its name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Write `text` to `path` whole: to a temporary file beside it, then renamed
/// into place, as Cargo's registry cache does.
fn write_whole(path: &Path, text: &str) -> Result<()> {
    let tmp = with_suffix(path, ".tmp");
    let written = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() { let _ = std::fs::remove_file(&tmp); }
    written.with_context(|| format!("write {}", path.display()))
}

/// Write a kept file and its ETag, under an exclusive lock on
/// `<file>.lock` (so the pair is one writer's), each whole (temporary file
/// then rename). The old ETag goes first, so that it never stands for the
/// new file.
fn keep(file: &Path, text: &str, etag: Option<&str>) -> Result<()> {
    let etag_file = etag_path(file);
    std::fs::create_dir_all(file.parent().expect("a maker folder"))?;
    let lock_path = with_suffix(file, ".lock");
    let lock = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&lock_path)
        .with_context(|| format!("open {}", lock_path.display()))?;
    lock.lock().with_context(|| format!("lock {}", lock_path.display()))?;
    let _ = std::fs::remove_file(&etag_file);
    write_whole(file, text)?;
    if let Some(etag) = etag { write_whole(&etag_file, etag)?; }
    Ok(())
}

/// The files of the catalogue as `osd::resolve` reclaims them, asked of the
/// server one at a time and held in memory (`got`: the text, or none when the
/// server does not have it). A failure other than "not there" is kept in
/// `failed`, to be told instead of the fault it causes.
struct Fetching<'a> {
    online: &'a Online,
    cache: &'a Path,
    got: std::cell::RefCell<std::collections::HashMap<String, Option<Got>>>,
    failed: std::cell::Cell<Option<anyhow::Error>>,
}

impl osd::Files for Fetching<'_> {
    fn get(&self, id: &str) -> Option<String> {
        if let Some(held) = self.got.borrow().get(id) { return held.as_ref().map(|g| g.text.clone()); }
        if !is_driver_id(id) { return None; }
        let fetched = self.online.fetch_text(&format!("/api/v1/devices/{id}.json"), &driver_path(self.cache, id));
        let got = match fetched {
            Ok((200 | 304, got)) => got,
            Ok((404, _)) => None,
            Ok((status, _)) => { self.failed.set(Some(anyhow!("driver {id}: HTTP {status}"))); None }
            Err(e) => { self.failed.set(Some(e)); None }
        };
        let text = got.as_ref().map(|g| g.text.clone());
        self.got.borrow_mut().insert(id.to_string(), got);
        text
    }
}

/// Where driver `id` is kept under `root`.
pub fn driver_path(root: &Path, id: &str) -> PathBuf {
    let (maker, name) = id.split_once('/').unwrap_or(("", id));
    root.join(maker).join(format!("{name}.json"))
}

/// The header that presents the machine's account on every request that
/// does not carry it in its path (`/u/<id>/…`).
const ACCOUNT_HEADER: &str = "X-Synthhub-Id";
/// Redirects followed per request.
const MAX_REDIRECTS: usize = 5;
/// The most zero bits a delivery challenge may ask for (about a minute of work).
const MAX_CHALLENGE_BITS: u64 = 28;

/// Leading zero bits of a digest.
fn zero_bits(bytes: &[u8]) -> u32 {
    let mut n = 0;
    for b in bytes {
        if *b == 0 { n += 8; } else { return n + b.leading_zeros(); }
    }
    n
}

/// The smallest decimal `nonce` such that SHA-256 of `<challenge>:<nonce>`
/// starts with `bits` zero bits (the service's proof of work).
fn solve_challenge(challenge: &str, bits: u32) -> u64 {
    (0u64..).find(|n| zero_bits(ring::digest::digest(&ring::digest::SHA256, format!("{challenge}:{n}").as_bytes()).as_ref()) >= bits)
        .expect("a nonce exists")
}

/// The service's sentence from a response body, or `fallback` when it gave none.
fn sentence(body: &str, fallback: &str) -> String {
    let t = body.trim();
    if t.is_empty() { fallback.to_string() } else { t.to_string() }
}

/// A client of the hosted server.
pub struct Online {
    base: String,
    agent: ureq::Agent,
    /// Where the account id is kept (`account_file` unless set otherwise).
    account_path: Option<PathBuf>,
    /// Held while the account is read or delivered, so that simultaneous
    /// requests on a machine without one deliver a single account.
    delivering: std::sync::Mutex<()>,
    /// The account this client delivered, for when the file cannot be kept.
    delivered: std::sync::Mutex<Option<String>>,
}

impl Online {
    pub fn new(base: &str) -> Self {
        Self::keeping_account_in(base, account_file())
    }

    pub fn keeping_account_in(base: &str, account_path: Option<PathBuf>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .timeout_connect(Some(Duration::from_secs(5)))
            .user_agent(concat!("osc-bridge/", env!("CARGO_PKG_VERSION")))
            .http_status_as_error(false)
            .max_redirects(0)
            .build()
            .new_agent();
        Self { base: base.trim_end_matches('/').to_string(), agent, account_path, delivering: std::sync::Mutex::new(()),
            delivered: std::sync::Mutex::new(None) }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn read_text(&self, mut resp: ureq::http::Response<ureq::Body>) -> Result<(u16, String)> {
        let status = resp.status().as_u16();
        let text = resp.body_mut().with_config().limit(64 << 20).read_to_string()?;
        Ok((status, text))
    }

    fn read(&self, resp: ureq::http::Response<ureq::Body>) -> Result<(u16, Value)> {
        let (status, text) = self.read_text(resp)?;
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }

    /// One HTTP exchange, redirects followed by hand: `id` goes in
    /// `ACCOUNT_HEADER` only to the base address, never to another host.
    /// The response, and whether it came from the base address.
    fn exchange(&self, method: &str, path: &str, id: Option<&str>, headers: &[(&str, &str)], body: Option<&[u8]>)
        -> Result<(ureq::http::Response<ureq::Body>, bool)> {
        let mut url = format!("{}{path}", self.base);
        for hop in 0..=MAX_REDIRECTS {
            let same = url == self.base || url.starts_with(&format!("{}/", self.base)) || url.starts_with(&format!("{}?", self.base));
            let mut req = ureq::http::Request::builder().method(if hop == 0 { method } else { "GET" }).uri(&url);
            for (k, v) in headers { req = req.header(*k, *v); }
            if let (Some(id), true) = (id, same) { req = req.header(ACCOUNT_HEADER, id); }
            let payload = if hop == 0 { body.unwrap_or_default().to_vec() } else { Vec::new() };
            let resp = self.agent.run(req.body(payload)?).with_context(|| format!("{method} {path}"))?;
            let status = resp.status().as_u16();
            let target = resp.headers().get("location").and_then(|v| v.to_str().ok());
            match target {
                Some(loc) if matches!(status, 301 | 302 | 303 | 307 | 308) && method == "GET" => {
                    url = if loc.starts_with('/') { format!("{}{loc}", self.base) } else { loc.to_string() };
                }
                _ => return Ok((resp, same)),
            }
        }
        bail!("{method} {path}: too many redirects")
    }

    /// Every request of the client goes through here: it presents the
    /// machine's account (obtained first when there is none), replays the
    /// request once with a new account when the service answers 401, and
    /// turns the service's 429 sentence into the error. Requests to
    /// `/u/<id>/…` carry the account in their path instead.
    fn send(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> Result<ureq::http::Response<ureq::Body>> {
        let in_path = path.starts_with("/u/");
        // An account delivered for this very request is not delivered again on a 401: the request is replayed with it.
        let delivered_now = !in_path && self.kept_account().is_none();
        let mut id = if in_path { None } else { Some(self.account(false)?) };
        let mut replayed = false;
        loop {
            let (mut resp, same) = self.exchange(method, path, id.as_deref(), headers, body)?;
            note_deprecation(resp.headers());
            let status = resp.status().as_u16();
            if status == 401 && same && !in_path && !replayed {
                replayed = true;
                if !delivered_now { id = Some(self.renew(id.as_deref())?); }
                continue;
            }
            if status == 429 && same {
                let text = resp.body_mut().with_config().limit(1 << 16).read_to_string().unwrap_or_default();
                let text = text.trim();
                bail!("{}", if text.is_empty() { format!("{method} {path}: HTTP 429 (quota reached)") } else { text.to_string() });
            }
            return Ok(resp);
        }
    }

    /// `GET path`, kept in `file` with the ETag the server gave
    /// (`etag_path`), as Cargo's sparse registry keeps its index files: a
    /// file kept with its ETag is asked for with `If-None-Match`, and
    /// `304 Not Modified` leaves it as it is. On 200 the text, once `check`
    /// accepts it, replaces the kept file and its ETag. The status, and the
    /// file's text on 200 or 304.
    fn fetch_kept(&self, path: &str, file: &Path, check: impl Fn(&str) -> Result<()>) -> Result<(u16, Option<String>)> {
        let (status, got) = self.fetch_text(path, file)?;
        match got {
            Some(got) if status == 200 => {
                check(&got.text)?;
                keep(file, &got.text, got.etag.as_deref())?;
                Ok((200, Some(got.text)))
            }
            got => Ok((status, got.map(|g| g.text))),
        }
    }

    /// `GET path` as `fetch_kept` asks for it, but nothing is written: the
    /// status, and the text on 200 (`fresh`, with the server's ETag) or on
    /// 304 (the kept file's).
    fn fetch_text(&self, path: &str, file: &Path) -> Result<(u16, Option<Got>)> {
        let etag_file = etag_path(file);
        let kept = std::fs::read_to_string(&etag_file).ok().filter(|e| !e.trim().is_empty() && file.is_file());
        let mut headers = Vec::new();
        if let Some(etag) = &kept {
            headers.push(("If-None-Match", etag.trim()));
        }
        let mut resp = self.send("GET", path, &headers, None)?;
        match resp.status().as_u16() {
            304 if kept.is_some() => {
                let text = std::fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?;
                Ok((304, Some(Got { text, etag: None, fresh: false })))
            }
            200 => {
                let text = resp.body_mut().with_config().limit(16 << 20).read_to_string()?;
                let etag = resp.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string);
                Ok((200, Some(Got { text, etag, fresh: true })))
            }
            status => Ok((status, None)),
        }
    }

    fn get(&self, path: &str) -> Result<(u16, Value)> {
        let resp = self.send("GET", path, &[], None)?;
        self.read(resp)
    }

    fn post(&self, path: &str, body: &Value) -> Result<(u16, Value)> {
        let resp = self.send("POST", path, &[("Content-Type", "application/json")], Some(body.to_string().as_bytes()))?;
        self.read(resp)
    }

    fn kept_account(&self) -> Option<String> {
        let on_disk = self.account_path.as_ref().and_then(|f| std::fs::read_to_string(f).ok())
            .map(|s| s.trim().to_ascii_lowercase()).filter(|id| is_user_id(id));
        on_disk.or_else(|| self.delivered.lock().ok().and_then(|g| g.clone()))
    }

    /// The account kept on this machine, or a new one the service delivers
    /// (then kept). `forget` drops a kept id the service no longer knows.
    pub fn account(&self, forget: bool) -> Result<String> {
        let _held = self.delivering.lock().map_err(|_| anyhow!("account unavailable"))?;
        if !forget {
            if let Some(id) = self.kept_account() { return Ok(id); }
        }
        self.deliver()
    }

    /// A new account after the service refused `stale`; the one another
    /// request already delivered meanwhile, when there is one.
    fn renew(&self, stale: Option<&str>) -> Result<String> {
        let _held = self.delivering.lock().map_err(|_| anyhow!("account unavailable"))?;
        match self.kept_account() {
            Some(id) if Some(id.as_str()) != stale => Ok(id),
            _ => self.deliver(),
        }
    }

    /// The service's delivery: `GET /v1/access/challenge`, the proof of work,
    /// `POST <deliver>`; the id it gives is kept.
    fn deliver(&self) -> Result<String> {
        let failed = |why: String| anyhow!("account delivery failed: {why}");
        let (resp, _) = self.exchange("GET", "/v1/access/challenge", None, &[], None).map_err(|e| failed(format!("{e:#}")))?;
        let (status, text) = self.read_text(resp).map_err(|e| failed(format!("{e:#}")))?;
        if status == 429 { bail!(failed(sentence(&text, "HTTP 429"))); }
        let ch: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let challenge = ch["challenge"].as_str().filter(|_| status == 200 && ch["algorithm"] == "sha256");
        let (Some(challenge), Some(bits), Some(deliver)) = (challenge, ch["bits"].as_u64(), ch["deliver"].as_str().filter(|d| d.starts_with('/'))) else {
            bail!(failed(format!("the service gave no challenge (HTTP {status})")));
        };
        if bits > MAX_CHALLENGE_BITS { bail!(failed(format!("the challenge asks for {bits} bits"))); }
        let nonce = solve_challenge(challenge, bits as u32);
        let body = json!({ "challenge": challenge, "nonce": nonce.to_string() }).to_string();
        let (resp, _) = self.exchange("POST", deliver, None, &[("Content-Type", "application/json")], Some(body.as_bytes()))
            .map_err(|e| failed(format!("{e:#}")))?;
        let (status, text) = self.read_text(resp).map_err(|e| failed(format!("{e:#}")))?;
        if status == 429 { bail!(failed(sentence(&text, "HTTP 429"))); }
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let id = v["id"].as_str().map(str::to_ascii_lowercase).filter(|id| status == 200 && is_user_id(id))
            .ok_or_else(|| failed(format!("HTTP {status}")))?;
        if let Ok(mut g) = self.delivered.lock() { *g = Some(id.clone()); }
        if let Some(f) = &self.account_path {
            if let Some(dir) = f.parent() { let _ = std::fs::create_dir_all(dir); }
            let _ = std::fs::write(f, &id);
        }
        Ok(id)
    }

    /// Call one of the server's MCP tools as `user`; the result as the
    /// server gives it. Fails with the server's message on a protocol error.
    pub fn call_tool(&self, user: &str, name: &str, args: &Value) -> Result<Value> {
        let rpc = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
        let (status, v) = self.post(&format!("/u/{user}/mcp"), &rpc)?;
        if status == 404 || status == 401 {
            bail!("unknown account");
        }
        match (v.get("result"), v.get("error")) {
            (Some(r), _) => Ok(r.clone()),
            (None, Some(e)) => bail!("{}", e["message"].as_str().unwrap_or("error")),
            _ => bail!("the server answered HTTP {status}"),
        }
    }

    /// Attach a preview (`audio` or `image`) to a preset the account shared.
    pub fn upload(&self, user: &str, preset: &str, kind: &str, bytes: &[u8]) -> Result<()> {
        let path = format!("/u/{user}/v1/presets/{preset}/{kind}");
        let resp = self.send("PUT", &path, &[("Content-Type", "application/octet-stream")], Some(bytes))?;
        let (status, v) = self.read(resp)?;
        if status != 200 { bail!("{}", v["error"].as_str().unwrap_or("upload refused")); }
        Ok(())
    }

    /// Trust of every reported driver (`GET /v1/trust`).
    pub fn trust(&self) -> Result<Value> {
        let (status, v) = self.get("/v1/trust")?;
        if status != 200 { bail!("trust: HTTP {status}"); }
        Ok(v)
    }

    /// The synths the server's librarian can take sounds from (`GET /v1/librarian`).
    pub fn librarian_synths(&self) -> Result<Value> {
        let (status, v) = self.get("/v1/librarian")?;
        if status != 200 { bail!("{}", v["error"].as_str().unwrap_or("the server has no librarian")); }
        Ok(v)
    }

    /// One librarian step (`POST /v1/librarian/<op>`): recognize, request, receive.
    pub fn librarian(&self, op: &str, body: &Value) -> Result<Value> {
        let (status, v) = self.post(&format!("/v1/librarian/{op}"), body)?;
        if status != 200 { bail!("{}", v["error"].as_str().unwrap_or("librarian: refused")); }
        Ok(v)
    }

    /// The MIDI messages that load a shared preset, fitted to a synth on
    /// `channel` when the librarian took it (`GET /v1/presets/<id>/messages`).
    pub fn preset_messages(&self, id: &str, channel: Option<i64>) -> Result<Value> {
        let query = channel.map(|c| format!("?channel={c}")).unwrap_or_default();
        let (status, v) = self.get(&format!("/v1/presets/{id}/messages{query}"))?;
        if status != 200 { bail!("{}", v["error"].as_str().unwrap_or("no such preset")); }
        Ok(v)
    }

    /// Fetch driver `id` into `cache` (`driver_path`), with its notes and the
    /// files `osd::resolve` says it is built from (the drivers it takes its
    /// MIDI from, the maker profiles its blocks name, kept as
    /// `<maker>/_profile….json` beside the drivers), so that it loads there.
    /// The files are asked of the server as `osd` reclaims them (`Fetching`)
    /// and held in memory; they are written to the cache only once the
    /// resolution has succeeded. Each file already kept is downloaded again
    /// only when the server's copy changed (`fetch_text`). A driver `osd`
    /// refuses, or the server no longer has, is an error, and the cache stays
    /// as it was.
    pub fn fetch_driver(&self, id: &str, cache: &Path) -> Result<PathBuf> {
        if !is_driver_id(id) { bail!("{id:?} is not a driver id (<maker>/<name>)"); }
        let files = Fetching { online: self, cache, got: Default::default(), failed: Default::default() };
        let resolved = match osd::resolve(id, &files) {
            Ok(r) => r,
            Err(fault) => {
                if let Some(e) = files.failed.take() { return Err(e); }
                if files.got.borrow().get(id).is_some_and(Option::is_none) {
                    bail!("the SynthHub catalogue has no driver {id}");
                }
                return Err(anyhow::Error::from(fault).context(format!("driver {id} from the server")));
            }
        };
        for file in &resolved.files {
            let path = driver_path(cache, file);
            if let Some(Some(got)) = files.got.borrow().get(file) {
                if got.fresh { keep(&path, &got.text, got.etag.as_deref())?; }
            }
            if !file.split_once('/').is_some_and(|(_, f)| f.starts_with("_profile")) {
                let notes = path.with_extension("md");
                match self.fetch_kept(&format!("/api/v1/devices/{file}.md"), &notes, |_| Ok(()))? {
                    (200 | 304, Some(_)) => {}
                    (404, _) => {
                        let _ = std::fs::remove_file(&notes);
                        let _ = std::fs::remove_file(etag_path(&notes));
                    }
                    (s, _) => bail!("notes of {file}: HTTP {s}"),
                }
            }
        }
        Ok(driver_path(cache, id))
    }

    /// The variants of a device, by one of its ids or its slug, best first
    /// (`GET /v1/devices/<id or slug>`): `{ "best": id, "variants": [...] }`.
    pub fn variants(&self, id_or_slug: &str) -> Result<Value> {
        let (status, v) = self.get(&format!("/v1/devices/{id_or_slug}"))?;
        match status {
            200 => Ok(v),
            404 => bail!("the SynthHub catalogue has no device {id_or_slug:?} (see list_devices)"),
            s => bail!("{}", v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("device {id_or_slug}: HTTP {s}"))),
        }
    }

    /// The maker requests to send while detecting, after the Universal
    /// Identity Request (`GET /v1/detect`): those the catalogue's identity
    /// blocks name, each with the ports it may go to.
    pub fn probes(&self) -> Result<Vec<crate::recognition::Probe>> {
        let (status, v) = self.get("/v1/detect")?;
        if status != 200 { bail!("detect probes: HTTP {status}"); }
        v["probes"].as_array().into_iter().flatten().map(|p| {
            let request = crate::librarian::parse_hex(p["request_hex"].as_str().unwrap_or_default())?;
            if let Some(fault) = crate::recognition::maker_request_fault(&request) { bail!("a detect probe of the service: {fault}"); }
            let ports = p["ports"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            // `usb` and `match_ports` are optional: a probe without them is held back without consent.
            let usb = p["usb"].as_array().into_iter().flatten().filter_map(|u| {
                Some((u16::try_from(u["vendor_id"].as_u64()?).ok()?, u16::try_from(u["product_id"].as_u64()?).ok()?))
            }).collect();
            let match_ports = p["match_ports"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            Ok(crate::recognition::Probe { request, ports, usb, match_ports })
        }).collect()
    }

    /// The drivers matching MIDI ports found on this machine
    /// (`POST /v1/detect`): `ports` is `[{ "port", "replies_hex": [...] }]`
    /// (the SysEx each device sent back while detected), the answer
    /// `{ "ports": [{ "port", "candidates": [...] }] }` in the same order.
    pub fn detect(&self, ports: &[Value]) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for batch in ports.chunks(PORTS_PER_REQUEST) {
            let (status, v) = self.post("/v1/detect", &json!({ "ports": batch }))?;
            if status != 200 { bail!("{}", v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("detect: HTTP {status}"))); }
            out.extend(v["ports"].as_array().cloned().unwrap_or_default());
        }
        Ok(out)
    }

    /// Shares a device fingerprint with its vendor, model and firmware (`POST /v1/fingerprints`).
    /// A field left empty is refused here, before anything is sent. Nothing goes out for a
    /// private device (by its `vendor/model` id), when offline: naming the device is the
    /// person's act of sending. A failure leaves the fingerprint here, with no retry.
    pub fn share_fingerprint(&self, settings_file: &Path, vendor: &str, model: &str, firmware: &str, fp: &Value) -> Sharing {
        let missing: Vec<&str> = [("vendor", vendor), ("model", model), ("firmware", firmware)]
            .into_iter().filter(|(_, v)| v.trim().is_empty()).map(|(k, _)| k).collect();
        if !missing.is_empty() {
            return Sharing::Failed(format!("device: missing {}", missing.join(", ")));
        }
        let settings = match Settings::load(Some(settings_file)) {
            Ok(s) => s,
            Err(e) => return Sharing::Failed(format!("settings: {e:#}")),
        };
        let id = super::learn::device_id(vendor, model);
        if settings.offline || Settings::covers(&settings.private, &id, &id) {
            return Sharing::Withheld(format!("{vendor} {model} is private or the link is cut: nothing is sent"));
        }
        let body = serde_json::to_value(FingerprintBody::new(vendor, model, firmware, fp)).unwrap_or(Value::Null);
        match self.post("/v1/fingerprints", &body) {
            Ok((200..=299, _)) => Sharing::Sent,
            Ok((status, v)) => Sharing::Failed(v["error"].as_str().map(|e| format!("HTTP {status}: {e}")).unwrap_or_else(|| format!("HTTP {status}"))),
            Err(e) => Sharing::Failed(format!("{e:#}")),
        }
    }

    /// One of the server's catalogue tools (`list_devices`…), (`POST /mcp`).
    pub fn catalogue_call(&self, name: &str, args: &Value) -> Result<Value> {
        let rpc = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
        let (status, v) = self.post("/mcp", &rpc)?;
        match (v.get("result"), v.get("error")) {
            (Some(r), _) => Ok(r.clone()),
            (None, Some(e)) => bail!("{}", e["message"].as_str().unwrap_or("error")),
            _ => bail!("the server answered HTTP {status}"),
        }
    }
}

/// What `Online::share_fingerprint` did.
#[derive(Debug, PartialEq)]
pub enum Sharing {
    Sent,
    /// Not sent, and why (private, offline).
    Withheld(String),
    /// The sending failed, and why; the fingerprint stays on this machine.
    Failed(String),
}

/// The device a fingerprint belongs to: its maker, its model and its firmware version.
#[derive(serde::Serialize)]
pub struct DeviceIds {
    vendor: String,
    model: String,
    firmware: String,
}

#[derive(serde::Serialize)]
pub struct UsbIds {
    vendor_id: Value,
    product_id: Value,
}

/// The body of `POST /v1/fingerprints`, as the service reads it today (a mirror until
/// synthhub-protocol holds the type): the device (vendor, model, firmware) and the
/// identification fields of the fingerprint. The serial number, the host name and everything local are left out.
#[derive(serde::Serialize)]
pub struct FingerprintBody {
    device: DeviceIds,
    #[serde(skip_serializing_if = "Option::is_none")]
    usb: Option<UsbIds>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_reply: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ports: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sends: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    controls: Option<Value>,
}

impl FingerprintBody {
    pub fn new(vendor: &str, model: &str, firmware: &str, fp: &Value) -> Self {
        let given = |k: &str| fp.get(k).filter(|v| !v.is_null()).cloned();
        let usb = fp.get("usb").filter(|u| u.is_object()).map(|u| UsbIds {
            vendor_id: u.get("vendor_id").cloned().unwrap_or(Value::Null),
            product_id: u.get("product_id").cloned().unwrap_or(Value::Null),
        });
        let device = DeviceIds { vendor: vendor.trim().into(), model: model.trim().into(), firmware: firmware.trim().into() };
        Self { device, usb, identity_reply: given("identity_reply"), ports: given("ports"), sends: given("sends"), controls: given("controls") }
    }
}

#[cfg(test)]
mod tests;
