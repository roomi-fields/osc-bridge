//! MCP (Model Context Protocol) server — exposes osc-bridge's catalogue and
//! OSC surface to LLMs via JSON-RPC 2.0 over stdio.
//!
//! Spec: <https://modelcontextprotocol.io/specification/2025-06-18>
//!
//! Tools exposed:
//!  - `list_devices`   — enumerate all device JSONs under `--devices-dir`
//!  - `get_device_docs`— fetch the .md companion of one device by slug
//!  - `list_routes`    — return the OSC routes (commands + cc/params) of one device
//!  - `send`           — emit an OSC message to a running osc-bridge bind port
//!  - `get_status`     — round-trip `/bridge/status` to a running bridge
//!
//!  - `detect_hardware`, `backup_presets`, `restore_presets` — the MIDI
//!    ports of the machine the server runs on
//!  - `report_result`, `get_trust` — hosted server only: usage reports and
//!    the trust they earn a driver (`crate::reports`)
//!
//! A local copy (stdio) is linked to the hosted server unless offline
//! (`crate::online`): it mirrors the server's catalogue, shows each driver's
//! trust, and forwards the report and contribution tools there.
//!
//! Two transports. On stdio (`run`) the server reads one JSON-RPC request per
//! line from stdin and writes one response per line to stdout; logging goes
//! to stderr. Over HTTP (`run_http`, MCP "streamable HTTP" without the
//! optional event stream) each POST carries one request and gets its
//! response as JSON; only the catalogue and report tools are offered there,
//! since the others act on the machine hosting the server, not the caller's.

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::device::Device;

const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "osc-bridge";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct McpOptions {
    pub devices_dir: PathBuf,
    pub default_target: SocketAddr,
    /// Served over the network: offer `HOSTED_TOOLS` only.
    pub hosted: bool,
    /// A local copy linked to the hosted server (`crate::online`): mirror its
    /// catalogue (when `devices_dir` is the default one) and forward the
    /// report and contribution tools to it.
    pub online: bool,
}

/// The tools of a server reached over the network: reading the catalogue
/// and reporting on it. None touches the host's MIDI ports.
const HOSTED_TOOLS: [&str; 13] = ["list_devices", "get_device_docs", "list_routes", "report_result", "get_trust",
    "report_mapping", "submit_driver", "submit_fix", "get_contribution",
    "relay_status", "relay_detect", "relay_send", "relay_listen"];

/// Who is calling, as far as the hosted server knows: the account (an id
/// the server issued), the ledger, the relay doors and the usage log. All
/// absent on stdio.
#[derive(Clone, Copy, Default)]
pub struct Caller<'a> {
    pub user: Option<&'a str>,
    pub ledger: Option<&'a crate::reports::Ledger>,
    pub relays: Option<&'a crate::relay::Relays>,
    usage: Option<&'a UsageLog>,
    /// A local copy's link to the hosted server.
    online: Option<&'a Link>,
}

/// A local copy's link to the hosted server: the client, the account kept on
/// this machine, and the trust of every reported driver, as last fetched.
struct Link {
    client: crate::online::Online,
    user: std::sync::Mutex<Option<String>>,
    trust: std::sync::RwLock<Value>,
}

impl Link {
    fn new(base: &str) -> Self {
        Self { client: crate::online::Online::new(base), user: std::sync::Mutex::new(None), trust: std::sync::RwLock::new(Value::Null) }
    }

    /// The account, issued by the server on first use; `forget` replaces one
    /// the server no longer knows.
    fn user(&self, forget: bool) -> Result<String, String> {
        let mut g = self.user.lock().map_err(|_| "account unavailable")?;
        if let (Some(id), false) = (g.as_ref(), forget) {
            return Ok(id.clone());
        }
        let id = self.client.account(forget).map_err(|e| e.to_string())?;
        *g = Some(id.clone());
        Ok(id)
    }

    fn forward(&self, name: &str, args: &Value) -> Result<Value, String> {
        let unreachable = |e: anyhow::Error| format!("{name} goes to the osc-bridge server, which could not be reached: {e}");
        let user = self.user(false).map_err(|e| unreachable(anyhow::anyhow!(e)))?;
        match self.client.call_tool(&user, name, args) {
            Err(e) if e.to_string() == "unknown account" => {
                let user = self.user(true).map_err(|e| unreachable(anyhow::anyhow!(e)))?;
                self.client.call_tool(&user, name, args).map_err(unreachable)
            }
            r => r.map_err(unreachable),
        }
    }

    /// A driver's trust status, once trust has been fetched.
    fn status(&self, id: &str) -> Option<String> {
        let trust = self.trust.read().ok()?;
        if trust.is_null() { return None; }
        Some(trust["devices"][id]["status"].as_str().unwrap_or("unreported").to_string())
    }
}

/// What only the hosted server has: the ledger, the relay doors, the usage
/// log, the administrator's token (`OSC_BRIDGE_ADMIN_TOKEN`) guarding
/// `/admin/*`, and the merge job's token (`OSC_BRIDGE_SYNC_TOKEN`), which
/// opens `/admin/contributions*` only. Without a token its endpoints answer 404.
pub struct Hosted {
    pub ledger: std::sync::Arc<crate::reports::Ledger>,
    pub relays: std::sync::Arc<crate::relay::Relays>,
    usage: UsageLog,
    pub admin_token: Option<String>,
    pub sync_token: Option<String>,
    /// The catalogue served to local copies (`GET /v1/catalogue`): its root
    /// and, computed on first request, every file with its SHA-256.
    catalogue_root: PathBuf,
    catalogue: std::sync::OnceLock<BTreeMap<String, String>>,
}

impl Hosted {
    fn caller<'a>(&'a self, user: Option<&'a str>) -> Caller<'a> {
        Caller { user, ledger: Some(&self.ledger), relays: Some(&self.relays), usage: Some(&self.usage), online: None }
    }

    fn catalogue(&self) -> &BTreeMap<String, String> {
        self.catalogue.get_or_init(|| crate::online::catalogue_files(&self.catalogue_root))
    }
}

/// Relay doors keep their account: a known id stays, anything else gets a
/// new account.
struct LedgerAccounts(std::sync::Arc<crate::reports::Ledger>);

impl crate::relay::Accounts for LedgerAccounts {
    fn resolve(&self, presented: Option<&str>) -> Option<String> {
        match presented.map(str::to_ascii_lowercase) {
            Some(id) if self.0.knows(&id) => Some(id),
            _ => self.0.issue_account().ok(),
        }
    }
}

/// One lightweight catalogue entry. The full `Device` is loaded on demand
/// (per `get_device_docs` / `list_routes` call) — only this metadata is held
/// resident.
struct DeviceIndexEntry {
    path: PathBuf,
    /// Path under the catalogue root without `.json` (`korg/minilogue`),
    /// the same id as the site's API.
    id: String,
    slug: String,
    name: String,
    vendor: String,
    kind: String,
    revision: String,
    tier: String,
    firmware: Option<String>,
    manufacturer_id: Vec<u8>,
}

/// Built once at startup. Without it, every device tool call would re-walk
/// and re-parse the whole `devices/` tree (~900 files); with it, `list_devices`
/// is a cheap serialize and slug lookup is O(1).
struct DeviceIndex {
    entries: Vec<DeviceIndexEntry>,
    by_slug: HashMap<String, usize>,
}

impl DeviceIndex {
    /// Walk `devices/` once, parsing each JSON just enough to capture the
    /// catalogue metadata. When several files share an `osc_prefix` (the
    /// per-variant convention), the most trustworthy source wins the slug
    /// lookup (`tier_rank`), ties broken by path so the choice is stable.
    fn build(dir: &Path) -> Self {
        let mut entries: Vec<DeviceIndexEntry> = Vec::new();
        let mut by_slug: HashMap<String, usize> = HashMap::new();
        let _ = walk_json(dir, &mut |path, doc| {
            let device = doc.get("device");
            let get = |k: &str| device.and_then(|d| d.get(k)).and_then(Value::as_str);
            let slug = get("osc_prefix").unwrap_or("").trim_start_matches('/').to_string();
            let source = doc.get("_sources").and_then(Value::as_array).and_then(|a| a.first());
            let tier = source.and_then(|s| s.get("type").and_then(Value::as_str))
                .unwrap_or("unknown").to_string();
            let firmware = source.and_then(|s| s.get("firmware").and_then(Value::as_str))
                .filter(|f| *f != "unknown").map(str::to_string);
            let idx = entries.len();
            if !slug.is_empty() {
                let better = match by_slug.get(&slug) {
                    None => true,
                    Some(&cur) => {
                        let c = &entries[cur];
                        (tier_rank(&tier), path) < (tier_rank(&c.tier), c.path.as_path())
                    }
                };
                if better { by_slug.insert(slug.clone(), idx); }
            }
            entries.push(DeviceIndexEntry {
                path: path.to_path_buf(),
                id: path.strip_prefix(dir).unwrap_or(path).with_extension("")
                    .to_string_lossy().replace('\\', "/"),
                slug,
                name: get("name").unwrap_or("?").to_string(),
                vendor: get("vendor").unwrap_or("?").to_string(),
                kind: get("kind").unwrap_or("hardware").to_string(),
                revision: get("revision").unwrap_or("").to_string(),
                tier,
                firmware,
                manufacturer_id: device.and_then(|d| d.get("manufacturer_id"))
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_u64).map(|b| b as u8).collect())
                    .unwrap_or_default(),
            });
        });
        Self { entries, by_slug }
    }

    fn find_path(&self, slug: &str) -> Option<&Path> {
        self.by_slug.get(slug).map(|&i| self.entries[i].path.as_path())
    }

    /// A driver by its id (`korg/minilogue`) or, failing that, its slug.
    fn find(&self, id_or_slug: &str) -> Option<&DeviceIndexEntry> {
        self.entries.iter().find(|e| e.id == id_or_slug)
            .or_else(|| self.by_slug.get(id_or_slug).map(|&i| &self.entries[i]))
    }
}

/// Lower is more trustworthy: tested on the device, then the vendor's own
/// documentation, then curated OSC surfaces, then community sources, then
/// what agents contributed (checked automatically, not reviewed).
fn tier_rank(tier: &str) -> u8 {
    match tier {
        "hardware-verified" | "software-verified" => 0,
        "vendor-doc" | "vendor" => 1,
        "vendor-osc-api" | "third-party-osc" => 2,
        "electra-preset" => 3,
        "pencilresearch" => 4,
        "agent-contributed" => 5,
        "test-fixture" => 6,
        _ => 7,
    }
}

/// Build the catalogue index once — every device tool reads from it
/// instead of re-walking the tree.
fn build_index(devices_dir: &Path, opts: &McpOptions) -> DeviceIndex {
    eprintln!("MCP server starting (devices_dir={}, default_target={})",
        devices_dir.display(), opts.default_target);
    let index = DeviceIndex::build(devices_dir);
    eprintln!("indexed {} device file(s)", index.entries.len());
    index
}

/// Sent with `initialize` by a linked local copy.
const LINKED_INSTRUCTIONS: &str = "osc-bridge catalogue of MIDI implementations (CC, NRPN, SysEx, drum \
    maps) for about 980 synths, drum machines and DAWs, kept up to date from the osc-bridge server with \
    what other users and agents fixed and confirmed. Start with list_devices, then list_routes with the \
    slug; usage_status and get_trust tell how far other users confirmed a driver, per firmware. After \
    testing a driver on the real device, call report_result with the firmware and the device's Identity \
    Reply: it improves the driver for everyone.";

/// Link a local copy to the hosted server, in the background: fetch trust,
/// mirror the catalogue when `sync` (then swap in an index of the mirror).
fn link_in_background(link: std::sync::Arc<Link>, index: std::sync::Arc<std::sync::RwLock<std::sync::Arc<DeviceIndex>>>,
                      sync: Option<(PathBuf, PathBuf, bool)>) {
    std::thread::spawn(move || {
        match link.client.trust() {
            Ok(t) => if let Ok(mut g) = link.trust.write() { *g = t },
            Err(e) => { eprintln!("osc-bridge server unreachable ({e}): offline catalogue"); return; }
        }
        let Some((bundled, cache, on_cache)) = sync else { return };
        let user = match link.user(false) { Ok(u) => u, Err(e) => { eprintln!("no account: {e}"); return; } };
        match link.client.sync_catalogue(&user, &bundled, &cache) {
            Ok(done) => {
                eprintln!("catalogue synced with the osc-bridge server: {} files, {} copied, {} downloaded, {} removed",
                    done.files, done.copied, done.downloaded, done.removed);
                if done.changed() || !on_cache {
                    let fresh = DeviceIndex::build(&cache);
                    eprintln!("indexed {} device file(s) from {}", fresh.entries.len(), cache.display());
                    if let Ok(mut g) = index.write() { *g = std::sync::Arc::new(fresh); }
                }
            }
            Err(e) => eprintln!("catalogue sync failed ({e}): keeping the current catalogue"),
        }
    });
}

pub fn run(opts: McpOptions) -> Result<()> {
    use std::sync::{Arc, RwLock};
    let bundled = crate::catalogue::resolve(&opts.devices_dir);
    // Only the default catalogue is mirrored: one the user points to is theirs.
    let mirror = (opts.online && opts.devices_dir == Path::new("devices"))
        .then(crate::online::cache_dir).flatten();
    let cached = mirror.as_ref().and(crate::online::usable_cache());
    let start = cached.clone().unwrap_or_else(|| bundled.clone());
    let index = Arc::new(RwLock::new(Arc::new(build_index(&start, &opts))));
    let link = opts.online.then(|| Arc::new(Link::new(crate::online::SERVER)));
    if let Some(link) = &link {
        link_in_background(link.clone(), index.clone(), mirror.map(|m| (bundled, m, cached.is_some())));
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if l.trim().is_empty() => continue,
            Ok(l) => l,
            Err(e) => { eprintln!("stdin err: {e}"); break; }
        };
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("parse err: {e} on line: {line}");
                continue;
            }
        };
        let current = index.read().map(|g| g.clone()).map_err(|_| anyhow::anyhow!("catalogue index poisoned"))?;
        let caller = Caller { online: link.as_deref(), ..Caller::default() };
        if let Some(mut resp) = handle_request(&req, &opts, &current, caller) {
            if let (true, Some(result)) = (link.is_some() && req["method"] == "initialize", resp.get_mut("result")) {
                result["instructions"] = json!(LINKED_INSTRUCTIONS);
            }
            let s = serde_json::to_string(&resp)?;
            writeln!(out, "{s}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Web pages allowed to open a relay door, besides localhost: the site's
/// `/connect` page.
const RELAY_ORIGINS: [&str; 1] = ["https://osc-bridge.roomi-fields.com"];

/// Largest request body accepted over HTTP. A tool call is a few hundred
/// bytes; this only stops a client from making the server buffer garbage.
const MAX_HTTP_BODY: usize = 64 * 1024;

/// Worker threads answering HTTP requests. Most calls are short reads of the
/// in-memory index and of one driver file; relay calls wait up to a few
/// seconds for the hardware to answer.
const HTTP_WORKERS: usize = 16;

/// Requests one client may make per `RATE_WINDOW` before getting 429.
const RATE_LIMIT: u32 = 120;
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// Fixed-window request counter per client. The map is pruned of expired
/// windows whenever it grows past a few thousand clients.
#[derive(Default)]
struct RateLimiter {
    windows: std::sync::Mutex<HashMap<String, (std::time::Instant, u32)>>,
}

impl RateLimiter {
    fn allow(&self, client: &str) -> bool {
        let now = std::time::Instant::now();
        let Ok(mut map) = self.windows.lock() else { return true };
        if map.len() > 4096 {
            map.retain(|_, (start, _)| now.duration_since(*start) < RATE_WINDOW);
        }
        let slot = map.entry(client.to_string()).or_insert((now, 0));
        if now.duration_since(slot.0) >= RATE_WINDOW {
            *slot = (now, 0);
        }
        slot.1 += 1;
        slot.1 <= RATE_LIMIT
    }
}

/// Who is asking: the TCP peer, or behind a reverse proxy the address the
/// proxy saw (the peer is then the proxy itself).
fn client_key(req: &tiny_http::Request, behind_proxy: bool) -> String {
    let forwarded = behind_proxy.then(|| req.headers().iter()
        .find(|h| h.field.equiv("X-Forwarded-For"))
        .and_then(|h| crate::ws::proxy_seen_address(h.value.as_str())))
        .flatten();
    forwarded.unwrap_or_else(|| req.remote_addr().map(|a| a.ip().to_string()).unwrap_or_default())
}

/// Serve MCP over HTTP on `bind`: `POST /mcp` with one JSON-RPC message,
/// answered as `application/json` (202 with no body for a notification).
/// The catalogue is public data and no tool has an effect, so any origin
/// may call it (CORS `*`); each client is held to `RATE_LIMIT` requests per
/// minute. `GET /` returns a one-line description for people who open the
/// URL in a browser, `GET /healthz` answers `ok`. With `behind_proxy`, the
/// client is identified by `X-Forwarded-For` — only set it when a proxy that
/// overwrites that header is the sole way in. `data_dir` holds
/// `usage.jsonl` and the ledger `ledger.jsonl` (accounts, reports,
/// decisions); without it they live in memory only.
pub fn run_http(opts: McpOptions, bind: &str, behind_proxy: bool, data_dir: Option<&Path>, relay_bind: Option<&str>) -> Result<()> {
    let opts = McpOptions { hosted: true, online: false, ..opts };
    let devices_dir = crate::catalogue::resolve(&opts.devices_dir);
    let hosted = Hosted {
        ledger: std::sync::Arc::new(crate::reports::Ledger::open(data_dir.map(|d| d.join("ledger.jsonl")).as_deref())?),
        relays: std::sync::Arc::new(crate::relay::Relays::default()),
        usage: UsageLog::open(data_dir.map(|d| d.join("usage.jsonl")).as_deref())?,
        admin_token: std::env::var("OSC_BRIDGE_ADMIN_TOKEN").ok().filter(|t| t.len() >= 16),
        sync_token: std::env::var("OSC_BRIDGE_SYNC_TOKEN").ok().filter(|t| t.len() >= 16),
        catalogue_root: devices_dir.clone(),
        catalogue: std::sync::OnceLock::new(),
    };
    if let Some(rb) = relay_bind {
        let addr = crate::relay::serve(rb, hosted.relays.clone(),
            std::sync::Arc::new(LedgerAccounts(hosted.ledger.clone())),
            RELAY_ORIGINS.iter().map(|o| o.to_string()).collect(), behind_proxy)?;
        eprintln!("relay doors on ws://{addr}");
    }
    if hosted.admin_token.is_none() {
        eprintln!("no OSC_BRIDGE_ADMIN_TOKEN (16+ characters): /admin endpoints disabled");
    }
    let index = build_index(&devices_dir, &opts);
    let server = tiny_http::Server::http(bind).map_err(|e| anyhow::anyhow!("bind HTTP {bind}: {e}"))?;
    eprintln!("MCP over HTTP on http://{}/mcp ({})", server.server_addr(), HOSTED_TOOLS.join(", "));
    let shared = std::sync::Arc::new((server, opts, index, RateLimiter::default(), hosted));
    let workers: Vec<_> = (0..HTTP_WORKERS).map(|_| {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let (server, opts, index, limiter, hosted) = &*shared;
            for mut req in server.incoming_requests() {
                if !limiter.allow(&client_key(&req, behind_proxy)) {
                    respond(req, HttpReply::new(429, Some("rate limit: 120 requests per minute\n".into())));
                    continue;
                }
                let reply = http_exchange(&read_request(&mut req), opts, index, hosted);
                if let Some(line) = &reply.usage {
                    hosted.usage.write(line);
                }
                respond(req, reply);
            }
        })
    }).collect();
    for w in workers { let _ = w.join(); }
    Ok(())
}

/// One HTTP request, read into memory so the exchange is a plain function.
struct HttpRequest {
    method: String,
    path: String,
    body: Result<String, String>,
    /// The account id the client presents: from a personal URL
    /// (`/u/<id>/mcp`) or the `Mcp-Session-Id` header (see `user_id`).
    /// Not yet checked against the ledger.
    user: Option<String>,
    /// The bearer token of an `Authorization` header.
    bearer: Option<String>,
    /// The client's `User-Agent` (`osc-bridge/0.14.0` for a local copy).
    client: Option<String>,
}

/// What the server sends back. `session` is the id to return in the
/// `Mcp-Session-Id` header, set when the server hands one out.
struct HttpReply {
    status: u16,
    body: Option<String>,
    session: Option<String>,
    /// The usage record of a tool call (`usage_record`), for the worker to
    /// write out.
    usage: Option<String>,
}

impl HttpReply {
    fn new(status: u16, body: Option<String>) -> Self {
        Self { status, body, session: None, usage: None }
    }
}

/// Where usage records go: always stderr, and appended to `usage.jsonl` in
/// the server's data directory when it has one, so they outlive the container.
struct UsageLog(Option<std::sync::Mutex<std::fs::File>>);

impl UsageLog {
    fn open(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else { return Ok(Self(None)) };
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        // Readable by the server's own user only.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(path)
            .map_err(|e| anyhow::anyhow!("open usage log {}: {e}", path.display()))?;
        Ok(Self(Some(std::sync::Mutex::new(file))))
    }

    fn write(&self, line: &str) {
        eprintln!("{line}");
        if let Some(file) = &self.0 {
            if let Ok(mut f) = file.lock() {
                let _ = writeln!(f, "{line}");
            }
        }
    }
}

use crate::online::is_user_id;

/// Accounts without sign-up: the server issues every id and keeps it in the
/// ledger. A client that initializes without one gets a new account, in
/// `Mcp-Session-Id`, which MCP clients repeat on each request of the
/// session. To stay the same account across sessions, a client uses its
/// personal endpoint `/u/<id>/mcp` (a fresh one from `GET /u/new`); that id
/// then serves as the session id too. Returns the path without the id
/// prefix, and the id when one was presented and well formed.
fn user_id(path: &str, session_header: Option<&str>) -> (String, Option<String>) {
    if let Some(rest) = path.strip_prefix("/u/") {
        if let Some((id, tail)) = rest.split_once('/') {
            if is_user_id(id) {
                return (format!("/{tail}"), Some(id.to_ascii_lowercase()));
            }
        }
    }
    (path.to_string(), session_header.filter(|h| is_user_id(h)).map(str::to_ascii_lowercase))
}

fn read_request(req: &mut tiny_http::Request) -> HttpRequest {
    use std::io::Read;
    let method = req.method().as_str().to_ascii_uppercase();
    let session = req.headers().iter().find(|h| h.field.equiv("Mcp-Session-Id"))
        .map(|h| h.value.as_str().trim().to_string());
    let (path, user) = user_id(req.url().split('?').next().unwrap_or(""), session.as_deref());
    let mut text = String::new();
    let body = match req.as_reader().take(MAX_HTTP_BODY as u64 + 1).read_to_string(&mut text) {
        Ok(n) if n > MAX_HTTP_BODY => Err("request body too large".into()),
        Ok(_) => Ok(text),
        Err(e) => Err(format!("read body: {e}")),
    };
    let bearer = req.headers().iter().find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(|t| t.trim().to_string()));
    let client = req.headers().iter().find(|h| h.field.equiv("User-Agent"))
        .map(|h| h.value.as_str().chars().take(60).collect());
    HttpRequest { method, path, body, user, bearer, client }
}

/// Compare a presented token with the expected one in time independent of
/// where they differ.
fn token_matches(given: &str, expected: &str) -> bool {
    given.len() == expected.len()
        && given.bytes().zip(expected.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// `/admin/*`: what waits for approval, and decisions on reports and
/// accounts. 404 without a configured token, 401 with a wrong one.
fn admin_exchange(req: &HttpRequest, hosted: &Hosted) -> HttpReply {
    use crate::reports::{ContributionState, Decision};
    let contributions = req.path == "/admin/contributions" || req.path.starts_with("/admin/contributions/");
    let accepted: Vec<&String> = hosted.admin_token.iter()
        .chain(hosted.sync_token.iter().filter(|_| contributions)).collect();
    if accepted.is_empty() { return HttpReply::new(404, None) }
    if !req.bearer.as_deref().is_some_and(|t| accepted.iter().any(|e| token_matches(t, e))) {
        return HttpReply::new(401, Some(json!({ "error": "admin token required" }).to_string()));
    }
    let error = |status: u16, msg: String| HttpReply::new(status, Some(json!({ "error": msg }).to_string()));
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/admin/contributions") => HttpReply::new(200, Some(json!({
            "contributions": hosted.ledger.pending_contributions() }).to_string())),
        ("POST", p) if p.starts_with("/admin/contributions/") => {
            let Some(id) = p.rsplit('/').next().and_then(|n| n.parse::<u64>().ok()) else {
                return error(404, "unknown contribution".into());
            };
            let Some(body) = req.body.as_deref().ok().and_then(|b| serde_json::from_str::<Value>(b).ok()) else {
                return error(400, "expected a JSON object".into());
            };
            let st = match body.get("state").and_then(Value::as_str) {
                Some("merged") => ContributionState::Merged,
                Some("rejected") => ContributionState::Rejected,
                _ => return error(400, "state must be merged or rejected".into()),
            };
            let text = |k: &str| body.get(k).and_then(Value::as_str).map(|s| s.chars().take(2000).collect());
            match hosted.ledger.settle_contribution(id, st, text("detail"), text("pr")) {
                Ok(()) => HttpReply::new(200, Some(json!({ "applied": true }).to_string())),
                Err(e) => error(400, e),
            }
        }
        ("GET", "/admin/pending") => HttpReply::new(200, Some(hosted.ledger.pending().to_string())),
        ("POST", "/admin/decide") => {
            let Ok(body) = req.body.as_deref().map(serde_json::from_str::<Value>) else {
                return error(413, "request body too large".into());
            };
            let Ok(body) = body else { return error(400, "expected a JSON object".into()) };
            let decision = match body.get("decision").and_then(Value::as_str) {
                Some("approve") => Decision::Approve,
                Some("reject") => Decision::Reject,
                Some("suspect") => Decision::Suspect,
                Some("clear") => Decision::Clear,
                _ => return error(400, "decision must be approve, reject, suspect or clear".into()),
            };
            let report = body.get("report").and_then(Value::as_u64);
            let user = body.get("user").and_then(Value::as_str);
            match hosted.ledger.decide(decision, report, user) {
                Ok(v) => HttpReply::new(200, Some(v.to_string())),
                Err(e) => error(400, e),
            }
        }
        _ => HttpReply::new(404, None),
    }
}

/// The reply to one HTTP request.
fn http_exchange(req: &HttpRequest, opts: &McpOptions, index: &DeviceIndex, hosted: &Hosted) -> HttpReply {
    let ledger = &hosted.ledger;
    let rpc_error = |code: i64, msg: String| json!({
        "jsonrpc": "2.0", "id": null, "error": { "code": code, "message": msg },
    }).to_string();
    match (req.method.as_str(), req.path.as_str()) {
        ("OPTIONS", _) => HttpReply::new(204, None),
        ("GET", "/") => HttpReply::new(200, Some(format!(
            "osc-bridge {SERVER_VERSION} MCP server (tools: {}). POST JSON-RPC to /mcp; \
             a personal endpoint that keeps your account: GET /u/new. \
             Trust of every reported driver: GET /v1/trust. \
             Docs: https://osc-bridge.roomi-fields.com/api/ \
             Source (AGPL-3.0-or-later): https://github.com/roomi-fields/osc-bridge\n", HOSTED_TOOLS.join(", ")))),
        ("GET", "/v1/trust") => HttpReply::new(200, Some(ledger.all().to_string())),
        ("GET", "/v1/catalogue") => {
            let files: Vec<Value> = hosted.catalogue().iter().map(|(p, h)| json!({ "path": p, "sha256": h })).collect();
            let user = req.user.as_deref().filter(|id| ledger.knows(id));
            HttpReply { usage: Some(json!({
                "event": "catalogue_sync",
                "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "user": user, "client": req.client,
            }).to_string()), ..HttpReply::new(200, Some(json!({ "version": SERVER_VERSION, "files": files }).to_string())) }
        }
        ("POST", "/v1/catalogue/files") => {
            let wanted = req.body.as_ref().ok().and_then(|b| serde_json::from_str::<Value>(b).ok())
                .and_then(|v| v["paths"].as_array().cloned()).unwrap_or_default();
            if wanted.len() > crate::online::FILES_PER_REQUEST {
                return HttpReply::new(400, Some(json!({ "error": format!("at most {} paths per request", crate::online::FILES_PER_REQUEST) }).to_string()));
            }
            let known = hosted.catalogue();
            let files: BTreeMap<&str, String> = wanted.iter().filter_map(Value::as_str)
                .filter(|p| known.contains_key(*p))
                .filter_map(|p| Some((p, std::fs::read_to_string(p.split('/').fold(hosted.catalogue_root.clone(), |d, c| d.join(c))).ok()?)))
                .collect();
            HttpReply::new(200, Some(json!({ "files": files }).to_string()))
        }
        ("GET", "/v1/learned") => HttpReply::new(200, Some(json!({ "mappings": ledger.learned(None) }).to_string())),
        (_, p) if p.starts_with("/admin/") => admin_exchange(req, hosted),
        ("GET", "/healthz") => HttpReply::new(200, Some("ok\n".into())),
        ("GET", "/u/new") => match ledger.issue_account() {
            Ok(id) => HttpReply::new(200, Some(json!({ "user_id": id, "mcp_path": format!("/u/{id}/mcp") }).to_string())),
            Err(e) => HttpReply::new(503, Some(json!({ "error": e }).to_string())),
        },
        // No server-initiated event stream (optional in the spec).
        ("GET", "/mcp") => HttpReply::new(405, None),
        ("POST", "/mcp") => {
            let body = match &req.body {
                Ok(b) => b,
                Err(e) => return HttpReply::new(413, Some(rpc_error(-32600, e.clone()))),
            };
            let msg = match serde_json::from_str::<Value>(body) {
                Ok(msg) if msg.is_object() => msg,
                Ok(_) => return HttpReply::new(400, Some(rpc_error(-32600, "expected one JSON-RPC object".into()))),
                Err(e) => return HttpReply::new(400, Some(rpc_error(-32700, format!("parse error: {e}")))),
            };
            let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
            // An id from the URL or the session header that the server never
            // issued: 404, so the client starts over (MCP spec for unknown
            // sessions). One in the arguments is simply ignored.
            if let Some(id) = &req.user {
                if !ledger.knows(id) {
                    return HttpReply::new(404, Some(rpc_error(-32001,
                        "unknown account: start a new session, or get one from GET /u/new".into())));
                }
            }
            let from_args = msg.pointer("/params/arguments/user_id").and_then(Value::as_str)
                .map(str::to_ascii_lowercase).filter(|id| ledger.knows(id));
            let user = match (req.user.clone().or(from_args), method) {
                (Some(id), _) => Some(id),
                (None, "initialize") => ledger.issue_account().ok(),
                (None, _) => None,
            };
            let usage = (method == "tools/call").then(|| usage_record(user.as_deref(), &msg));
            let session = if method == "initialize" { user.clone() } else { None };
            match handle_request(&msg, opts, index, hosted.caller(user.as_deref())) {
                Some(mut resp) => {
                    if let (Some(id), Some(result)) = (&session, resp.get_mut("result")) {
                        result["instructions"] = json!(keep_id_instructions(id));
                    }
                    HttpReply { status: 200, body: Some(resp.to_string()), session, usage }
                }
                None => HttpReply { status: 202, body: None, session, usage },
            }
        }
        _ => HttpReply::new(404, None),
    }
}

/// Sent with `initialize`: the client's anonymous id and how to keep it.
/// Nothing guarantees a client keeps it; this gives the ones that can (a
/// saved server URL, an agent with memory) the means to.
fn keep_id_instructions(id: &str) -> String {
    format!("osc-bridge catalogue of MIDI implementations (CC, NRPN, SysEx, drum maps) for about \
        980 synths, drum machines and DAWs. Start with list_devices (query: 'korg minilogue'), then \
        list_routes with the slug; get_trust tells how far other users confirmed a driver, per firmware. \
        After testing a driver on the real device, call report_result with the firmware and the \
        device's Identity Reply. This server issued you the account {id} (no sign-up, no personal \
        data); your reports belong to it. To keep it in later sessions, connect to the personal \
        endpoint /u/{id}/mcp on this server, or pass user_id: \"{id}\" in tool calls; keep it if \
        you can store it.")
}

/// One JSON line per tool call: when, which anonymous user, which tool and
/// the catalogue arguments. No address, no other request data.
fn usage_record(user: Option<&str>, msg: &Value) -> String {
    let params = msg.get("params");
    let args = params.and_then(|p| p.get("arguments"));
    let pick = |k: &str| args.and_then(|a| a.get(k)).cloned().unwrap_or(Value::Null);
    json!({
        "event": "tool_call",
        "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "user": user,
        "tool": params.and_then(|p| p.get("name")),
        "query": pick("query"), "vendor": pick("vendor"), "kind": pick("kind"), "slug": pick("slug"),
    }).to_string()
}

fn respond(req: tiny_http::Request, reply: HttpReply) {
    use tiny_http::{Header, Response};
    let HttpReply { status, body, session, .. } = reply;
    let header = |k: &str, v: &str| Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("static header");
    let kind = match &body {
        Some(b) if b.starts_with('{') => "application/json",
        _ => "text/plain; charset=utf-8",
    };
    let resp = Response::from_string(body.unwrap_or_default())
        .with_status_code(status)
        .with_header(header("Content-Type", kind))
        .with_header(header("Access-Control-Allow-Origin", "*"))
        .with_header(header("Access-Control-Allow-Methods", "POST, GET, OPTIONS"))
        .with_header(header("Access-Control-Allow-Headers", "content-type, mcp-protocol-version, mcp-session-id"))
        .with_header(header("Access-Control-Expose-Headers", "mcp-session-id"));
    let resp = match session {
        Some(id) => resp.with_header(header("Mcp-Session-Id", &id)),
        None => resp,
    };
    let _ = req.respond(resp);
}

fn handle_request(req: &Value, opts: &McpOptions, index: &DeviceIndex, caller: Caller) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");

    // Notifications carry no id and expect no response.
    if id.is_none() && method.starts_with("notifications/") {
        return None;
    }

    let result: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(init_result()),
        "tools/list" => Ok(tools_list(opts)),
        "tools/call" => match req.get("params").and_then(|p| Some((p.get("name")?.as_str()?, p.get("arguments").cloned().unwrap_or(json!({})))))
        {
            // An unknown tool is a protocol error; a tool that fails reports
            // it in its result, so the model can read it and react (MCP spec).
            Some((name, _)) if !tool_names(opts).contains(&name) => Err((-32602, format!("unknown tool: {name}"))),
            Some((name, args)) => Ok(call_tool(name, &args, opts, index, caller).unwrap_or_else(|e| json!({
                "content": [ { "type": "text", "text": e } ],
                "isError": true,
            }))),
            None => Err((-32602, "missing params.name".into())),
        },
        "ping" => Ok(json!({})),
        _ => Err((-32601, format!("method not found: {method}"))),
    };

    match result {
        Ok(value) => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": value,
        })),
        Err((code, msg)) => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": msg },
        })),
    }
}

fn init_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION,
        }
    })
}

fn tools_list(opts: &McpOptions) -> Value {
    let mut all = all_tools();
    if let Some(tools) = all["tools"].as_array_mut() {
        let offered = tool_names(opts);
        tools.retain(|t| t["name"].as_str().is_some_and(|n| offered.contains(&n)));
        if opts.hosted {
            for t in tools.iter_mut() {
                t["inputSchema"]["properties"]["user_id"] = json!({
                    "type": "string",
                    "description": "Your anonymous osc-bridge id, if you kept it from an earlier session (see the server instructions). Optional.",
                });
            }
        }
    }
    all
}

fn all_tools() -> Value {
    json!({
        "tools": [
            {
                "name": "list_devices",
                "description": "Find device drivers in the catalogue (about 980 synths, drum machines, controllers and DAWs). Without arguments it lists them all; `query` keeps drivers whose vendor, name or slug contain every word (e.g. 'korg minilogue'). Returns name, vendor, OSC prefix (slug), kind, source tier and firmware version when the source states one. Pass the slug to list_routes for the full CC/NRPN/SysEx map.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Words that must all appear in the vendor, name or slug (case-insensitive)." },
                        "vendor": { "type": "string", "description": "Exact vendor name, case-insensitive (e.g. 'Moog')." },
                        "kind": { "type": "string", "enum": ["hardware", "software"] },
                        "limit": { "type": "integer", "minimum": 1, "description": "Return at most this many drivers." }
                    },
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "count": { "type": "integer", "description": "Number of matching drivers, before `limit`." },
                        "devices": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "Driver id (path in the catalogue), e.g. 'korg/minilogue'." },
                                    "usage_status": { "type": ["string", "null"], "description": "Hosted server only: trust from usage reports (see get_trust)." },
                                    "name": { "type": "string" },
                                    "vendor": { "type": "string" },
                                    "slug": { "type": "string", "description": "OSC prefix without the leading slash." },
                                    "kind": { "type": "string", "description": "'hardware' or 'software'." },
                                    "revision": { "type": "string" },
                                    "source_tier": { "type": "string" },
                                    "firmware": { "type": ["string", "null"], "description": "Firmware the source documents; null when it does not say." },
                                    "file": { "type": "string" }
                                },
                                "required": ["name", "vendor", "slug", "kind"]
                            }
                        }
                    },
                    "required": ["count", "devices"]
                },
                "annotations": {
                    "title": "List devices",
                    "readOnlyHint": true,
                    "idempotentHint": true,
                    "openWorldHint": false
                }
            },
            {
                "name": "get_device_docs",
                "description": "Fetch the markdown companion documentation for one device, if present. The slug is the device's OSC prefix without the leading slash (e.g. 'ableton' for /ableton).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "slug": { "type": "string", "description": "Device slug, e.g. 'ableton' or 'minilab3'" }
                    },
                    "required": ["slug"]
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "slug": { "type": "string" },
                        "has_docs": { "type": "boolean", "description": "False when no companion .md exists for the device." },
                        "markdown": { "type": "string", "description": "The companion documentation, or a not-found message when has_docs is false." }
                    },
                    "required": ["slug", "has_docs", "markdown"]
                },
                "annotations": {
                    "title": "Get device docs",
                    "readOnlyHint": true,
                    "idempotentHint": true,
                    "openWorldHint": false
                }
            },
            {
                "name": "list_routes",
                "description": "Return the OSC routes a device exposes: declarative commands (OSC paths it accepts), cc/nrpn params, sysex params, named note triggers (drum sounds), and reply patterns. Use this to discover what a device can do.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "slug": { "type": "string", "description": "Device slug, e.g. 'minilab3'" }
                    },
                    "required": ["slug"]
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string" },
                        "vendor": { "type": "string" },
                        "osc_prefix": { "type": "string" },
                        "kind": { "type": "string" },
                        "commands": {
                            "type": "array",
                            "description": "Declarative OSC paths the device accepts, with typed args and mode (forward/frame).",
                            "items": { "type": "object" }
                        },
                        "cc_params": {
                            "type": "array",
                            "description": "CC / NRPN parameters, each mapped to an OSC address. Every entry carries an 'expects' field (continuous/switch/momentary/trigger/clock/discrete) describing the device-side nature of the control — informative only, the signal realization is the caller's concern.",
                            "items": { "type": "object" }
                        },
                        "params": {
                            "type": "array",
                            "description": "SysEx parameters, each mapped to an OSC address. Each entry carries an 'expects' field — see cc_params.",
                            "items": { "type": "object" }
                        },
                        "triggers": {
                            "type": "array",
                            "description": "Named note-triggered sounds (drum machines, samplers). Send '<address> [velocity] [note] [channel]' to fire a Note On; 'note' is required when the entry has no natural note.",
                            "items": { "type": "object" }
                        },
                        "replies": {
                            "type": "array",
                            "description": "Reply patterns the device emits back onto the OSC surface.",
                            "items": { "type": "object" }
                        }
                    },
                    "required": ["device", "osc_prefix", "commands", "cc_params", "params", "triggers", "replies"]
                },
                "annotations": {
                    "title": "List device routes",
                    "readOnlyHint": true,
                    "idempotentHint": true,
                    "openWorldHint": false
                }
            },
            {
                "name": "send",
                "description": "Send a single OSC message to a running osc-bridge instance. Target defaults to the server's --default-target. Args are typed (int, float, string, bool).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "addr": { "type": "string", "description": "OSC address, e.g. /ableton/transport/play" },
                        "args": {
                            "type": "array",
                            "description": "Typed args. Each item is one of {int}, {float}, {string}, {bool}.",
                            "items": {}
                        },
                        "target": {
                            "type": "string",
                            "description": "Optional override host:port (e.g. 127.0.0.1:7777). Defaults to MCP server's --default-target."
                        }
                    },
                    "required": ["addr"]
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "sent": { "type": "boolean" },
                        "address": { "type": "string" },
                        "bytes": { "type": "integer", "description": "Encoded OSC packet size." },
                        "target": { "type": "string", "description": "host:port the packet was sent to." }
                    },
                    "required": ["sent", "address", "bytes", "target"]
                },
                "annotations": {
                    "title": "Send OSC message",
                    "readOnlyHint": false,
                    "destructiveHint": false,
                    "idempotentHint": false,
                    "openWorldHint": true
                }
            },
            {
                "name": "get_status",
                "description": "Send /bridge/status to a running osc-bridge and wait briefly for one reply per device (up to 500 ms).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "host:port to query, defaults to --default-target" }
                    }
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string" },
                        "replied": { "type": "boolean", "description": "False when no reply arrived within 500 ms." },
                        "replies": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "addr": { "type": "string" },
                                    "args": { "type": "array" }
                                },
                                "required": ["addr", "args"]
                            }
                        }
                    },
                    "required": ["target", "replied", "replies"]
                },
                "annotations": {
                    "title": "Get bridge status",
                    "readOnlyHint": true,
                    "idempotentHint": true,
                    "openWorldHint": true
                }
            },
            {
                "name": "backup_presets",
                "description": "Back up a hardware device's presets as a .syx file. Listens on the device's MIDI input and records every SysEx message it sends (a 'bulk dump'), stopping 3 s after the last one. Either the user starts the dump from the device's menu during the call, or pass request_hex (the device's dump-request message, from its manual or driver) with out_port. Use detect_hardware first to find the ports.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "in_port": { "type": "string", "description": "MIDI IN port of the device: index or name fragment (e.g. 'minilab')." },
                        "file": { "type": "string", "description": "Path of the .syx file to write." },
                        "request_hex": { "type": "string", "description": "Optional dump-request SysEx as hex, e.g. 'F0 43 20 00 F7'." },
                        "out_port": { "type": "string", "description": "MIDI OUT port for request_hex: index or name fragment." },
                        "max_seconds": { "type": "integer", "description": "Give up after this long, 1-600 (default 60)." }
                    },
                    "required": ["in_port", "file"]
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string" },
                        "messages": { "type": "integer" },
                        "bytes": { "type": "integer" }
                    },
                    "required": ["file", "messages", "bytes"]
                },
                "annotations": {
                    "title": "Back up device presets",
                    "readOnlyHint": false,
                    "destructiveHint": false,
                    "idempotentHint": false,
                    "openWorldHint": true
                }
            },
            {
                "name": "restore_presets",
                "description": "Send a .syx file to a hardware device, e.g. to restore a preset backup. This OVERWRITES the device's memory: confirm with the user first, then pass confirm_overwrite: true. Messages are paced (gap_ms) because older devices drop fast dumps.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "out_port": { "type": "string", "description": "MIDI OUT port of the device: index or name fragment." },
                        "file": { "type": "string", "description": "Path of the .syx file to send." },
                        "gap_ms": { "type": "integer", "description": "Pause between messages, 0-2000 ms (default 50)." },
                        "confirm_overwrite": { "type": "boolean", "description": "Must be true: the user agreed to overwrite the device's memory." }
                    },
                    "required": ["out_port", "file", "confirm_overwrite"]
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string" },
                        "messages": { "type": "integer" },
                        "bytes": { "type": "integer" }
                    },
                    "required": ["file", "messages", "bytes"]
                },
                "annotations": {
                    "title": "Restore device presets",
                    "readOnlyHint": false,
                    "destructiveHint": true,
                    "idempotentHint": true,
                    "openWorldHint": true
                }
            },
            {
                "name": "detect_hardware",
                "description": "Find the MIDI devices plugged into this computer and match them to catalogue drivers. Lists MIDI ports, sends the standard MIDI Identity Request (harmless: it changes nothing on the device) and reads each device's manufacturer, model and firmware. Returns, per device, the best matching drivers and a ready-to-run 'osc-bridge run' command with the right port numbers.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "probe": { "type": "boolean", "description": "Send the Identity Request (default true). False only lists ports and matches by port name." },
                        "timeout_ms": { "type": "integer", "description": "How long to wait for identity replies, 100-5000 ms (default 800)." }
                    }
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "probed": { "type": "boolean" },
                        "devices": {
                            "type": "array",
                            "description": "One entry per physical device (an output port paired with its input port by name).",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "port": { "type": "string" },
                                    "out_port": { "type": ["integer", "null"] },
                                    "in_port": { "type": ["integer", "null"] },
                                    "identity": { "type": ["object", "null"], "description": "Identity Reply, when the device answered." },
                                    "candidates": { "type": "array", "items": { "type": "object" } },
                                    "run_command": { "type": ["string", "null"] }
                                },
                                "required": ["port", "candidates"]
                            }
                        }
                    },
                    "required": ["probed", "devices"]
                },
                "annotations": {
                    "title": "Detect connected hardware",
                    "readOnlyHint": true,
                    "idempotentHint": true,
                    "openWorldHint": true
                }
            },
            {
                "name": "report_result",
                "description": "Tell the catalogue how a driver behaved on a real device, after testing it: 'works', 'fails' or 'partial', for the whole driver or one OSC address (from list_routes). Give the firmware version, and the device's reply to the MIDI Identity Request (F0 7E 7F 06 01 F7) as identity_hex when you can: trust is counted per firmware, so a failure on another firmware does not count against the driver, and a report of unknown firmware cannot be placed. Several accounts that agree on one firmware mark the driver usage-verified there; a report that several others contradict is rolled back and puts its account under review. Report only what was actually tested.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id (e.g. 'korg/minilogue') or slug, from list_devices." },
                        "outcome": { "type": "string", "enum": ["works", "fails", "partial"] },
                        "address": { "type": "string", "description": "One OSC address of the driver (e.g. '/minilogue/filter/cutoff'); omit to report on the whole driver." },
                        "firmware": { "type": "string", "description": "Firmware version of the tested device." },
                        "identity_hex": { "type": "string", "description": "The device's reply to the MIDI Identity Request, in hex (e.g. 'F0 7E 00 06 02 42 2C 01 00 00 02 01 00 00 F7'). Proves the device was connected and carries its firmware version." },
                        "note": { "type": "string", "description": "What happened, briefly (500 characters at most): wrong range, no effect, other parameter moved…" }
                    },
                    "required": ["device", "outcome"],
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "recorded": { "type": "boolean" },
                        "report": { "type": "integer", "description": "The report's id." },
                        "status": { "type": "string", "enum": ["accepted", "pending", "rolled-back", "rejected"], "description": "accepted: counts in the trust; pending: the account is under review; rolled-back: other accounts contradict it on this firmware." },
                        "trust": { "type": "object", "description": "The driver's trust after this report (see get_trust)." }
                    },
                    "required": ["recorded", "report", "status", "trust"]
                },
                "annotations": {
                    "title": "Report a test result",
                    "readOnlyHint": false,
                    "destructiveHint": false,
                    "idempotentHint": true,
                    "openWorldHint": false
                }
            },
            {
                "name": "get_trust",
                "description": "How far a driver has been confirmed by people who used it, per firmware: status (unreported, unconfirmed, usage-verified, firmware-dependent, failing, disputed), how many users report it works / fails / partly works, each firmware's own status, the device identities seen for each firmware, and the same per OSC address. needs_firmware: true means reports disagree and some have no firmware — identify the device before reporting (see learn). Check it before relying on a driver, together with its source_tier.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id (e.g. 'korg/minilogue') or slug, from list_devices." }
                    },
                    "required": ["device"],
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string" },
                        "overall": { "type": "object" },
                        "addresses": { "type": "object" }
                    },
                    "required": ["device", "overall", "addresses"]
                },
                "annotations": {
                    "title": "Get a driver's trust",
                    "readOnlyHint": true,
                    "idempotentHint": true,
                    "openWorldHint": false
                }
            },
            {
                "name": "report_mapping",
                "description": "Teach the catalogue which MIDI message a control really sends: after the user moved one control on the device (see relay_listen), give the driver address it is (from list_routes) — or, for a control the driver lacks, its panel label — and the CC, NRPN or note it sent, with the firmware and the device's Identity Reply when you have them. When several accounts observe the same thing on one firmware the mapping is learned; get_trust shows learned mappings and where they differ from the driver.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id or slug." },
                        "address": { "type": "string", "description": "The driver's OSC address for the control." },
                        "label": { "type": "string", "description": "The control's panel label, when the driver has no address for it." },
                        "cc": { "type": "integer", "minimum": 0, "maximum": 127 },
                        "nrpn_msb": { "type": "integer", "minimum": 0, "maximum": 127 },
                        "nrpn_lsb": { "type": "integer", "minimum": 0, "maximum": 127 },
                        "note": { "type": "integer", "minimum": 0, "maximum": 127 },
                        "channel": { "type": "integer", "minimum": 1, "maximum": 16 },
                        "firmware": { "type": "string" },
                        "identity_hex": { "type": "string", "description": "The device's reply to the MIDI Identity Request, in hex." }
                    },
                    "required": ["device"],
                    "additionalProperties": false
                },
                "annotations": { "title": "Report what a control sends", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
            },
            {
                "name": "submit_driver",
                "description": "Contribute a driver for a device the catalogue lacks (or a better one): the full driver file as a JSON object, in the osc-bridge device schema (device.name, device.vendor, device.osc_prefix, and cc_params / params / commands / triggers — see https://osc-bridge.roomi-fields.com/DEVICE_JSON_SCHEMA/). It is checked, then merged into the public catalogue by the merge job if it parses, lints and passes the tests, as its own file of source 'agent-contributed'. Say where the data comes from (manual page, tested on the device).",
                "inputSchema": { "type": "object", "properties": {
                    "driver": { "type": "object", "description": "The driver file." },
                    "firmware": { "type": "string", "description": "Firmware the driver was written or tested for." },
                    "source": { "type": "string", "description": "Where the mapping comes from (e.g. 'manual v1.2 p. 34, tested on the device')." },
                    "note": { "type": "string" } },
                    "required": ["driver"], "additionalProperties": false },
                "annotations": { "title": "Contribute a driver", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true }
            },
            {
                "name": "submit_fix",
                "description": "Contribute a corrected variant of a driver from what the catalogue learned: where learned mappings (get_trust → mappings, differs: true) disagree with the driver on one firmware, builds a copy of the driver with the learned CC / NRPN numbers, as a new file for that firmware, and queues it for the merge job.",
                "inputSchema": { "type": "object", "properties": {
                    "device": { "type": "string", "description": "Driver id or slug." },
                    "firmware": { "type": "string", "description": "Needed when learned mappings differ on several firmwares." } },
                    "required": ["device"], "additionalProperties": false },
                "annotations": { "title": "Contribute a fix from learned mappings", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true }
            },
            {
                "name": "get_contribution",
                "description": "Where a contribution stands: pending (waiting for the merge job), merged (with its pull request), rejected (with the reason), or held (its account is under review).",
                "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"], "additionalProperties": false },
                "annotations": { "title": "Contribution status", "readOnlyHint": true, "idempotentHint": true, "openWorldHint": false }
            },
            {
                "name": "relay_status",
                "description": "Check a relay: the user opened https://osc-bridge.roomi-fields.com/connect/ (or ran osc-bridge connect) and gave you its pairing code. Returns the MIDI output and input ports of their machine. Nothing is sent to the hardware.",
                "inputSchema": { "type": "object", "properties": { "code": { "type": "string", "description": "The pairing code the relay page or program shows (e.g. 'K7QM-2XPA')." } }, "required": ["code"], "additionalProperties": false },
                "annotations": { "title": "Relay status", "readOnlyHint": true, "idempotentHint": true, "openWorldHint": true }
            },
            {
                "name": "relay_detect",
                "description": "Find which devices are connected to the user's machine through a relay: sends the MIDI Identity Request (F0 7E 7F 06 01 F7, changes nothing on the device) to every output and matches the answers — manufacturer, model, firmware version — against the catalogue. Returns the best driver ids and identity_hex to pass to report_result. Devices that do not answer are only listed by port name.",
                "inputSchema": { "type": "object", "properties": { "code": { "type": "string", "description": "The pairing code the relay page or program shows (e.g. 'K7QM-2XPA')." },
                    "timeout_ms": { "type": "integer", "minimum": 100, "maximum": 5000, "description": "How long to wait for answers (default 1000)." } },
                    "required": ["code"], "additionalProperties": false },
                "annotations": { "title": "Detect devices through a relay", "readOnlyHint": true, "idempotentHint": true, "openWorldHint": true }
            },
            {
                "name": "relay_send",
                "description": "Play or set something on the user's hardware through a relay: give a driver (id or slug) and one of its OSC addresses from list_routes with its arguments (e.g. address '/filter/cutoff', args [64]; '/note/on', args [60, 100]) — the server turns it into the right CC, NRPN, note or SysEx message. Or give raw MIDI bytes as hex. Values are the device's own numbers, clamped to the parameter's range. Any SysEx but the Identity Request (patch dumps, stores, settings, preset restores) may overwrite the device's memory: the relay asks the user to accept it first and this call waits for the answer (up to 2 minutes), so tell the user what you are about to send.",
                "inputSchema": { "type": "object", "properties": { "code": { "type": "string", "description": "The pairing code the relay page or program shows (e.g. 'K7QM-2XPA')." },
                    "device": { "type": "string", "description": "Driver id (e.g. 'korg/minilogue') or slug." },
                    "address": { "type": "string", "description": "OSC address of the driver, with or without its prefix." },
                    "args": { "type": "array", "items": {}, "description": "OSC arguments (numbers, strings, booleans)." },
                    "hex": { "type": "string", "description": "Raw MIDI messages instead of device/address, e.g. 'B0 2B 40'." },
                    "port": { "type": "string", "description": "Output port name (or part of it); needed when the relay has several." },
                    "gap_ms": { "type": "integer", "minimum": 0, "maximum": 2000, "description": "Pause between the SysEx messages of a dump, for devices that write slowly (default 50)." } },
                    "required": ["code"], "additionalProperties": false },
                "annotations": { "title": "Send through a relay", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true }
            },
            {
                "name": "relay_listen",
                "description": "Listen to what the user's hardware sends through a relay for a few seconds: notes, CC, NRPN, SysEx, decoded. With a device, each CC or NRPN that moved is matched to the driver's OSC addresses — ask the user to move one control on the device while listening to learn which address it is, or to check that the driver's map is right (then record what you saw with report_mapping, so the catalogue learns it).",
                "inputSchema": { "type": "object", "properties": { "code": { "type": "string", "description": "The pairing code the relay page or program shows (e.g. 'K7QM-2XPA')." },
                    "seconds": { "type": "integer", "minimum": 1, "maximum": 15, "description": "How long to listen (default 3)." },
                    "device": { "type": "string", "description": "Driver id or slug, to match what moved to its addresses." } },
                    "required": ["code"], "additionalProperties": false },
                "annotations": { "title": "Listen through a relay", "readOnlyHint": true, "idempotentHint": false, "openWorldHint": true }
            }
        ]
    })
}

fn tool_names(opts: &McpOptions) -> Vec<&'static str> {
    if opts.hosted {
        return HOSTED_TOOLS.to_vec();
    }
    let local = ["list_devices", "get_device_docs", "list_routes", "send", "get_status",
         "detect_hardware", "backup_presets", "restore_presets"];
    let forwarded = if opts.online { crate::online::FORWARDED_TOOLS.as_slice() } else { &[] };
    local.iter().chain(forwarded).copied().collect()
}

fn call_tool(name: &str, args: &Value, opts: &McpOptions, index: &DeviceIndex, caller: Caller) -> Result<Value, String> {
    if let (None, Some(link)) = (caller.ledger, caller.online) {
        if crate::online::FORWARDED_TOOLS.contains(&name) {
            return link.forward(name, args);
        }
    }
    match name {
        "list_devices" => list_devices(index, &DeviceFilter::from_args(args), &|id| match (caller.ledger, caller.online) {
            (Some(l), _) => Some(l.status(id).to_string()),
            (None, Some(link)) => link.status(id),
            (None, None) => None,
        }),
        "report_result" => report_result(index, args, caller),
        "report_mapping" => report_mapping(index, args, caller),
        "submit_driver" => submit_driver(index, args, caller),
        "submit_fix" => submit_fix(index, args, caller),
        "get_contribution" => {
            let ledger = caller.ledger.ok_or("contributions go to the hosted server")?;
            let id = args.get("id").and_then(Value::as_u64).ok_or("missing id")?;
            let c = ledger.contribution(id).ok_or_else(|| format!("no contribution {id}"))?;
            Ok(content_structured(&serde_json::to_string_pretty(&c).unwrap_or_default(), c))
        }
        "relay_status" | "relay_detect" | "relay_send" | "relay_listen" => {
            let relays = caller.relays.ok_or("the relay runs on the hosted server only")?;
            let code = args.get("code").and_then(Value::as_str).ok_or("missing code (the pairing code shown by the door)")?;
            let session = relays.get(code).ok_or_else(|| format!(
                "no relay with code '{code}': open https://osc-bridge.roomi-fields.com/connect/ and read the code it shows"))?;
            session.claim(caller.user.ok_or("no account: initialize the session first")?)?;
            match name {
                "relay_status" => Ok(relay_status(&session)),
                "relay_detect" => relay_detect(index, &session, args, caller),
                "relay_send" => relay_send(index, &session, args),
                _ => relay_listen(index, &session, args),
            }
        }
        "get_trust" => {
            let ledger = caller.ledger.ok_or("trust is kept by the hosted server only")?;
            let device = args.get("device").and_then(Value::as_str).ok_or("missing device")?;
            let entry = index.find(device).ok_or_else(|| format!("no device '{device}'"))?;
            let mut trust = ledger.trust(&entry.id);
            trust["mappings"] = json!(mappings_against_driver(ledger, entry)?);
            Ok(content_structured(&serde_json::to_string_pretty(&trust).unwrap_or_default(), trust))
        }
        "get_device_docs" => {
            let slug = args.get("slug").and_then(Value::as_str).ok_or("missing slug")?;
            get_device_docs(index, slug)
        }
        "list_routes" => {
            let slug = args.get("slug").and_then(Value::as_str).ok_or("missing slug")?;
            list_routes(index, slug)
        }
        "send" => {
            let addr = args.get("addr").and_then(Value::as_str).ok_or("missing addr")?;
            let osc_args = args.get("args").cloned().unwrap_or(json!([]));
            let target = match args.get("target").and_then(Value::as_str) {
                Some(s) => s.parse::<SocketAddr>().map_err(|e| format!("invalid target: {e}"))?,
                None => opts.default_target,
            };
            send_osc_message(addr, &osc_args, target)
        }
        "get_status" => {
            let target = match args.get("target").and_then(Value::as_str) {
                Some(s) => s.parse::<SocketAddr>().map_err(|e| format!("invalid target: {e}"))?,
                None => opts.default_target,
            };
            get_status(target)
        }
        "backup_presets" => {
            let in_port = args.get("in_port").and_then(Value::as_str).ok_or("missing in_port")?;
            let file = args.get("file").and_then(Value::as_str).ok_or("missing file")?;
            let request = args.get("request_hex").and_then(Value::as_str);
            let out_port = args.get("out_port").and_then(Value::as_str);
            let max = args.get("max_seconds").and_then(Value::as_u64).unwrap_or(60).clamp(1, 600);
            backup_presets(in_port, Path::new(file), request, out_port, max)
        }
        "restore_presets" => {
            if args.get("confirm_overwrite").and_then(Value::as_bool) != Some(true) {
                return Err("restore overwrites the device's memory: ask the user, then pass confirm_overwrite: true".into());
            }
            let out_port = args.get("out_port").and_then(Value::as_str).ok_or("missing out_port")?;
            let file = args.get("file").and_then(Value::as_str).ok_or("missing file")?;
            let gap = args.get("gap_ms").and_then(Value::as_u64).unwrap_or(50).min(2000);
            restore_presets(out_port, Path::new(file), gap)
        }
        "detect_hardware" => {
            let probe = args.get("probe").and_then(Value::as_bool).unwrap_or(true);
            let wait = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(800).clamp(100, 5000);
            detect_hardware(index, probe, std::time::Duration::from_millis(wait))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// The hardware drivers of the catalogue, in the shape `detect::rank` scores.
fn hardware_candidates(index: &DeviceIndex) -> Vec<crate::detect::Candidate> {
    index.entries.iter().filter(|e| e.kind != "software").map(|e| crate::detect::Candidate {
        slug: e.slug.clone(),
        name: e.name.clone(),
        vendor: e.vendor.clone(),
        file: e.path.display().to_string(),
        tier: e.tier.clone(),
        manufacturer_id: e.manufacturer_id.clone(),
    }).collect()
}

fn detect_hardware(index: &DeviceIndex, probe: bool, wait: std::time::Duration) -> Result<Value, String> {
    let found = crate::detect::scan(probe, wait).map_err(|e| format!("MIDI scan: {e}"))?;
    let catalogue = hardware_candidates(index);
    let devices: Vec<Value> = found.iter().map(|d| {
        let ranked = crate::detect::rank(d.label(), d.identity.as_ref(), &catalogue, 3);
        let candidates: Vec<Value> = ranked.iter().map(|(score, i)| {
            let c = &catalogue[*i];
            json!({ "slug": c.slug, "name": c.name, "vendor": c.vendor, "file": c.file,
                    "source_tier": c.tier, "score": score })
        }).collect();
        let run_command = match (ranked.first(), d.out_index) {
            (Some((_, i)), Some(out)) => Some(format!("osc-bridge run --device {} --out-port {out}{}",
                catalogue[*i].file, d.in_index.map(|n| format!(" --in-port {n}")).unwrap_or_default())),
            _ => None,
        };
        json!({
            "port": d.label(),
            "out_port": d.out_index,
            "in_port": d.in_index,
            "identity": d.identity.as_ref().map(|id| json!({
                "manufacturer_id": id.manufacturer_hex(),
                "manufacturer": crate::detect::manufacturer_name(&id.manufacturer),
                "family": id.family,
                "model": id.model,
                "firmware": id.version.iter().map(u8::to_string).collect::<Vec<_>>().join("."),
            })),
            "candidates": candidates,
            "run_command": run_command,
        })
    }).collect();
    let structured = json!({ "probed": probe, "devices": devices });
    let text = serde_json::to_string_pretty(&structured).unwrap_or_default();
    Ok(content_structured(&text, structured))
}

fn backup_presets(in_port: &str, file: &Path, request: Option<&str>, out_port: Option<&str>, max_secs: u64) -> Result<Value, String> {
    use crate::librarian::{capture, parse_hex, write_syx};
    let req = match (request, out_port) {
        (Some(hex), Some(port)) => Some((port, parse_hex(hex).map_err(|e| e.to_string())?)),
        (Some(_), None) => return Err("request_hex needs out_port".into()),
        _ => None,
    };
    let cap = capture(in_port, req.as_ref().map(|(p, b)| (*p, b.as_slice())),
                      Duration::from_secs(3), Duration::from_secs(max_secs)).map_err(|e| e.to_string())?;
    if cap.messages.is_empty() {
        return Err(format!("no SysEx received within {max_secs} s — start the dump from the device's menu, or pass request_hex"));
    }
    write_syx(file, &cap.messages).map_err(|e| e.to_string())?;
    let structured = json!({ "file": file.display().to_string(), "messages": cap.messages.len(), "bytes": cap.bytes });
    Ok(content_structured(&format!("Saved {} SysEx message(s), {} bytes, to {}", cap.messages.len(), cap.bytes, file.display()), structured))
}

fn restore_presets(out_port: &str, file: &Path, gap_ms: u64) -> Result<Value, String> {
    use crate::librarian::{read_syx, send_messages};
    let msgs = read_syx(file).map_err(|e| e.to_string())?;
    let bytes: usize = msgs.iter().map(Vec::len).sum();
    send_messages(out_port, &msgs, Duration::from_millis(gap_ms)).map_err(|e| e.to_string())?;
    let structured = json!({ "file": file.display().to_string(), "messages": msgs.len(), "bytes": bytes });
    Ok(content_structured(&format!("Sent {} SysEx message(s), {} bytes, from {}", msgs.len(), bytes, file.display()), structured))
}

/// `osc-bridge detect`: the same report as the `detect_hardware` tool, for a
/// terminal.
pub fn print_detection(devices_dir: &Path, probe: bool) -> Result<()> {
    let index = DeviceIndex::build(&crate::catalogue::resolve(devices_dir));
    let report = detect_hardware(&index, probe, std::time::Duration::from_millis(800))
        .map_err(|e| anyhow::anyhow!(e))?;
    let devices = report["structuredContent"]["devices"].as_array().cloned().unwrap_or_default();
    if devices.is_empty() {
        println!("No MIDI port found.");
    }
    for d in devices {
        println!("{}  (out {}, in {})", d["port"].as_str().unwrap_or("?"),
            d["out_port"].as_u64().map(|n| n.to_string()).unwrap_or("-".into()),
            d["in_port"].as_u64().map(|n| n.to_string()).unwrap_or("-".into()));
        match d["identity"].as_object() {
            Some(id) => println!("  identity: {} [{}], family {}, model {}, firmware {}",
                id["manufacturer"].as_str().unwrap_or("unknown manufacturer"),
                id["manufacturer_id"].as_str().unwrap_or(""), id["family"], id["model"],
                id["firmware"].as_str().unwrap_or("")),
            None if probe => println!("  identity: no reply"),
            None => {}
        }
        for c in d["candidates"].as_array().into_iter().flatten() {
            println!("  match: {} — {} ({}, score {})", c["slug"].as_str().unwrap_or(""),
                c["file"].as_str().unwrap_or(""), c["source_tier"].as_str().unwrap_or(""), c["score"]);
        }
        if let Some(cmd) = d["run_command"].as_str() {
            println!("  run:   {cmd}");
        }
    }
    Ok(())
}

/// The `list_devices` arguments. Everything is optional; nothing given
/// keeps every driver.
#[derive(Default)]
struct DeviceFilter {
    words: Vec<String>,
    vendor: Option<String>,
    kind: Option<String>,
    limit: Option<usize>,
}

impl DeviceFilter {
    fn from_args(args: &Value) -> Self {
        let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_lowercase);
        Self {
            words: text("query").unwrap_or_default().split_whitespace().map(str::to_string).collect(),
            vendor: text("vendor"),
            kind: text("kind"),
            limit: args.get("limit").and_then(Value::as_u64).map(|n| n.max(1) as usize),
        }
    }

    fn keeps(&self, e: &DeviceIndexEntry) -> bool {
        let haystack = format!("{} {} {}", e.vendor, e.name, e.slug).to_lowercase();
        self.words.iter().all(|w| haystack.contains(w.as_str()))
            && self.vendor.as_ref().is_none_or(|v| e.vendor.to_lowercase() == *v)
            && self.kind.as_ref().is_none_or(|k| e.kind == *k)
    }
}

fn list_devices(index: &DeviceIndex, filter: &DeviceFilter, status: &dyn Fn(&str) -> Option<String>) -> Result<Value, String> {
    let matching: Vec<&DeviceIndexEntry> = index.entries.iter().filter(|e| filter.keeps(e)).collect();
    let count = matching.len();
    let devices: Vec<Value> = matching.into_iter().take(filter.limit.unwrap_or(usize::MAX)).map(|e| json!({
        "id": e.id,
        "usage_status": status(&e.id),
        "name": e.name,
        "vendor": e.vendor,
        "slug": e.slug,
        "kind": e.kind,
        "revision": e.revision,
        "source_tier": e.tier,
        "firmware": e.firmware,
        "file": e.path.display().to_string(),
    })).collect();
    let structured = json!({ "count": count, "devices": devices });
    let text = format!("{count} devices found.\n\n{}",
        serde_json::to_string_pretty(&structured["devices"]).unwrap_or_default());
    Ok(content_structured(&text, structured))
}

/// Every OSC address a driver answers to: commands, CC/NRPN and SysEx
/// parameters, drum sounds.
fn surface_addresses(dev: &Device) -> Vec<String> {
    let prefix = &dev.device.osc_prefix;
    let cc = dev.cc_params.iter().flat_map(|t| t.entries.iter().map(|e| e.osc.clone()));
    let params = dev.params.iter().flat_map(|t| t.entries.iter().map(|e| e.osc.clone()));
    let triggers = dev.triggers.iter().flat_map(|t| t.entries.iter().map(|e| e.osc.clone()));
    dev.commands.iter().map(|c| c.osc.clone()).chain(cc).chain(params).chain(triggers)
        .map(|a| format!("{prefix}{a}")).collect()
}

fn report_result(index: &DeviceIndex, args: &Value, caller: Caller) -> Result<Value, String> {
    use crate::reports::{DeviceIdentity, Outcome, Report, LEARN_FIRMWARE, MAX_NOTE};
    let ledger = caller.ledger.ok_or("reports go to the hosted server: https://mcp.osc-bridge.roomi-fields.com/mcp")?;
    let user = caller.user.ok_or("no account: initialize the session first, or pass the user_id the server gave you")?;
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let device = text("device").ok_or("missing device")?;
    let entry = index.find(device).ok_or_else(|| format!("no device '{device}' (use the id or slug from list_devices)"))?;
    let outcome = text("outcome").and_then(Outcome::parse).ok_or("outcome must be works, fails or partial")?;
    let address = match text("address") {
        Some(a) => {
            let dev = Device::load(&entry.path).map_err(|e| format!("load {}: {e}", entry.id))?;
            if !surface_addresses(&dev).iter().any(|s| s == a) {
                return Err(format!("'{a}' is not an address of {} (see list_routes)", entry.id));
            }
            Some(a.to_string())
        }
        None => None,
    };
    let identity = match text("identity_hex") {
        Some(hex) => {
            let bytes = crate::librarian::parse_hex(hex).map_err(|e| format!("identity_hex: {e}"))?;
            let id = crate::detect::parse_identity_reply(&bytes)
                .ok_or("identity_hex is not a MIDI Identity Reply (F0 7E <dev> 06 02 …)")?;
            if !entry.manufacturer_id.is_empty() && id.manufacturer != entry.manufacturer_id {
                return Err(format!("the device that answered is not from {}'s manufacturer", entry.id));
            }
            Some(DeviceIdentity::from(&id))
        }
        None => None,
    };
    let report = Report {
        id: 0,
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        user: user.to_string(),
        device: entry.id.clone(),
        address,
        outcome,
        firmware: text("firmware").map(|f| f.chars().take(40).collect()),
        identity,
        note: text("note").map(|n| n.chars().take(MAX_NOTE).collect()),
    };
    let unplaced = report.firmware.is_none() && report.identity.is_none();
    let (id, status) = ledger.add(report)?;
    let trust = ledger.trust(&entry.id);
    let mut text = format!("Report {id} recorded for {}: {}. Driver status: {}.", entry.id,
        serde_json::to_value(status).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
        trust["overall"]["status"].as_str().unwrap_or("?"));
    match status {
        crate::reports::ReportStatus::Pending => text.push_str(" Your account is under review: the report counts once approved."),
        crate::reports::ReportStatus::RolledBack => text.push_str(" Other accounts contradict it on this firmware, so it was rolled back and your account is under review."),
        _ => {}
    }
    if unplaced || trust["overall"]["needs_firmware"] == true {
        text.push_str(&format!("\n\n{LEARN_FIRMWARE}"));
    }
    Ok(content_structured(&text, json!({ "recorded": true, "report": id, "status": status, "trust": trust })))
}

fn relay_status(session: &crate::relay::Session) -> Value {
    let v = json!({
        "code": crate::relay::display_code(&session.code),
        "outputs": session.outputs(),
        "inputs": session.inputs(),
    });
    let text = format!("Relay {} connected. Outputs: {}. Inputs: {}.", v["code"].as_str().unwrap_or(""),
        session.outputs().join(", "), session.inputs().join(", "));
    content_structured(&text, v)
}

/// How long a relay call may wait for the hardware, in milliseconds.
fn wait_ms(args: &Value, key: &str, default: u64, max: u64) -> std::time::Duration {
    std::time::Duration::from_millis(args.get(key).and_then(Value::as_u64).unwrap_or(default).clamp(100, max))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// Ask every output of the door who is there (MIDI Identity Request), and
/// match each answer, with the name of the port it came on, against the
/// catalogue.
fn relay_detect(index: &DeviceIndex, session: &crate::relay::Session, args: &Value, caller: Caller) -> Result<Value, String> {
    use crate::detect::{parse_identity_reply, rank, IDENTITY_REQUEST};
    let outputs = session.outputs();
    if outputs.is_empty() {
        return Err("the relay has no MIDI output: plug the device in, or allow MIDI access on the page".into());
    }
    let since = std::time::Instant::now();
    for out in &outputs {
        session.send(out, &[IDENTITY_REQUEST.to_vec()], 0)?;
    }
    std::thread::sleep(wait_ms(args, "timeout_ms", 1000, 5000));
    let candidates = hardware_candidates(index);
    let mut seen = std::collections::HashSet::new();
    let mut found: Vec<Value> = Vec::new();
    for msg in session.received_since(since) {
        let Some(id) = parse_identity_reply(&msg.data) else { continue };
        if !seen.insert((msg.port.clone(), msg.data.clone())) { continue; }
        let matches: Vec<Value> = rank(&msg.port, Some(&id), &candidates, 3).into_iter().filter_map(|(score, i)| {
            let entry = index.entries.iter().find(|e| e.path.display().to_string() == candidates[i].file)?;
            Some(json!({ "id": entry.id, "name": entry.name, "vendor": entry.vendor, "source_tier": entry.tier, "score": score }))
        }).collect();
        let identity = json!({
            "manufacturer": id.manufacturer_hex(),
            "vendor": crate::detect::manufacturer_name(&id.manufacturer),
            "family": id.family, "model": id.model, "version": hex(&id.version),
        });
        if session.shares() {
            if let Some(usage) = caller.usage {
                usage.write(&json!({
                    "event": "relay_identity",
                    "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "user": caller.user,
                    "identity": identity,
                    "best": matches.first().map(|m| m["id"].clone()),
                }).to_string());
            }
        }
        found.push(json!({
            "input": msg.port,
            "output": outputs.iter().find(|o| **o == msg.port).cloned(),
            "identity": identity,
            "identity_hex": hex(&msg.data),
            "matches": matches,
        }));
    }
    let text = if found.is_empty() {
        format!("No device answered the Identity Request on {} (many older devices do not). Ports: {}.",
            outputs.join(", "), session.inputs().join(", "))
    } else {
        format!("{} device(s) answered. Pass identity_hex to report_result after testing.\n\n{}",
            found.len(), serde_json::to_string_pretty(&found).unwrap_or_default())
    };
    Ok(content_structured(&text, json!({ "devices": found, "outputs": outputs, "inputs": session.inputs() })))
}

/// Send one named OSC address of a driver — or raw MIDI bytes — through the
/// door to the hardware.
fn relay_send(index: &DeviceIndex, session: &crate::relay::Session, args: &Value) -> Result<Value, String> {
    let port = session.output(args.get("port").and_then(Value::as_str))?;
    let (messages, what) = match args.get("hex").and_then(Value::as_str) {
        Some(raw) => {
            let bytes = crate::librarian::parse_hex(raw).map_err(|e| format!("hex: {e}"))?;
            (split_midi(&bytes)?, "raw MIDI".to_string())
        }
        None => {
            let device = args.get("device").and_then(Value::as_str).ok_or("give device and address, or hex")?;
            let entry = index.find(device).ok_or_else(|| format!("no device '{device}' (use the id or slug from list_devices)"))?;
            let dev = Device::load(&entry.path).map_err(|e| format!("load {}: {e}", entry.id))?;
            let address = args.get("address").and_then(Value::as_str).ok_or("missing address (from list_routes)")?;
            let prefix = dev.device.osc_prefix.clone();
            let full = if address.starts_with(&format!("{prefix}/")) { address.to_string() } else { format!("{prefix}{address}") };
            let osc_args = args.get("args").and_then(Value::as_array).map(|a| a.iter().map(json_to_osc).collect()).unwrap_or_default();
            let encoded = crate::runtime::encode(dev, rosc::OscMessage { addr: full.clone(), args: osc_args });
            if encoded.is_empty() {
                return Err(format!("{full} sends nothing on {}: check the address and its arguments with list_routes", entry.id));
            }
            (encoded, full)
        }
    };
    let gap = args.get("gap_ms").and_then(Value::as_u64).unwrap_or(50).min(2000);
    let delivery = session.send(&port, &messages, gap)?;
    let accepted = delivery == crate::relay::Delivery::Accepted;
    let sent: Vec<String> = messages.iter().map(|m| hex(m)).collect();
    let text = format!("{}Sent {what} to {port}: {}",
        if accepted { "The user accepted at the relay. " } else { "" }, sent.join(" | "));
    Ok(content_structured(&text, json!({ "port": port, "messages": sent, "confirmed_by_user": accepted })))
}

/// Split raw bytes into MIDI messages, each starting with its status byte
/// (no running status): channel messages by their length, SysEx up to F7.
fn split_midi(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let status = bytes[i];
        let len = match status {
            0xF0 => bytes[i..].iter().position(|&b| b == 0xF7).map(|p| p + 1)
                .ok_or("SysEx without its closing F7")?,
            0x80..=0xBF | 0xE0..=0xEF | 0xF2 => 3,
            0xC0..=0xDF | 0xF1 | 0xF3 => 2,
            0xF4..=0xFF => 1,
            _ => return Err(format!("byte {status:02X} at {i} is not a status byte")),
        };
        let msg = bytes.get(i..i + len).ok_or("message cut short")?;
        if msg[1..].iter().any(|&b| b >= 0x80 && b != 0xF7) {
            return Err(format!("data byte out of range in {}", hex(msg)));
        }
        out.push(msg.to_vec());
        i += len;
    }
    if out.is_empty() {
        return Err("no MIDI message".into());
    }
    Ok(out)
}

/// One MIDI message in words: what it is, on which channel, which numbers.
fn describe_midi(data: &[u8]) -> Value {
    let status = data.first().copied().unwrap_or(0);
    let ch = (status & 0x0F) + 1;
    let at = |i: usize| data.get(i).copied().unwrap_or(0);
    match status & 0xF0 {
        0x80 => json!({ "kind": "note_off", "channel": ch, "note": at(1), "velocity": at(2) }),
        0x90 if at(2) == 0 => json!({ "kind": "note_off", "channel": ch, "note": at(1), "velocity": 0 }),
        0x90 => json!({ "kind": "note_on", "channel": ch, "note": at(1), "velocity": at(2) }),
        0xA0 => json!({ "kind": "poly_aftertouch", "channel": ch, "note": at(1), "value": at(2) }),
        0xB0 => json!({ "kind": "cc", "channel": ch, "cc": at(1), "value": at(2) }),
        0xC0 => json!({ "kind": "program_change", "channel": ch, "program": at(1) }),
        0xD0 => json!({ "kind": "aftertouch", "channel": ch, "value": at(1) }),
        0xE0 => json!({ "kind": "pitchbend", "channel": ch, "value": (at(1) as u16) | ((at(2) as u16) << 7) }),
        _ if status == 0xF0 => json!({ "kind": "sysex", "length": data.len() }),
        _ => json!({ "kind": "system", "status": status }),
    }
}

/// Collect what the hardware sends through the door for a few seconds. With
/// a device, CC and NRPN numbers are matched to the driver's addresses —
/// ask the user to move one control, and see which address it is.
fn relay_listen(index: &DeviceIndex, session: &crate::relay::Session, args: &Value) -> Result<Value, String> {
    let since = std::time::Instant::now();
    let seconds = args.get("seconds").and_then(Value::as_u64).unwrap_or(3).clamp(1, 15);
    std::thread::sleep(std::time::Duration::from_secs(seconds));
    let device = match args.get("device").and_then(Value::as_str) {
        Some(d) => {
            let entry = index.find(d).ok_or_else(|| format!("no device '{d}'"))?;
            Some(Device::load(&entry.path).map_err(|e| format!("load {}: {e}", entry.id))?)
        }
        None => None,
    };
    let received = session.received_since(since);
    let messages: Vec<Value> = received.iter().filter(|m| m.data.first() != Some(&0xF8) && m.data.first() != Some(&0xFE))
        .take(500).map(|m| {
            let mut d = describe_midi(&m.data);
            d["port"] = json!(m.port);
            d["hex"] = json!(hex(&m.data));
            d
        }).collect();
    let mut controls: BTreeMap<u8, (u8, u8, u8, usize)> = BTreeMap::new();
    for m in &messages {
        if m["kind"] == "cc" {
            let (cc, v) = (m["cc"].as_u64().unwrap_or(0) as u8, m["value"].as_u64().unwrap_or(0) as u8);
            let e = controls.entry(cc).or_insert((m["channel"].as_u64().unwrap_or(1) as u8, v, v, 0));
            e.1 = e.1.min(v); e.2 = e.2.max(v); e.3 += 1;
        }
    }
    let nrpn = nrpn_numbers(&received);
    let mapped = |cc: Option<u8>, nrpn: Option<(u8, u8)>| -> Vec<String> {
        let Some(dev) = &device else { return Vec::new() };
        dev.cc_params.iter().flat_map(|t| t.entries.iter()).filter(|e| match nrpn {
            Some((m, l)) => e.nrpn_msb == Some(m) && e.nrpn_lsb == Some(l),
            None => e.cc == cc && e.nrpn_msb.is_none(),
        }).map(|e| format!("{}{}", dev.device.osc_prefix, e.osc)).collect()
    };
    let moved: Vec<Value> = controls.iter().filter(|(cc, _)| ![6u8, 38, 98, 99].contains(cc) || nrpn.is_empty())
        .map(|(cc, (ch, lo, hi, n))| json!({
            "cc": cc, "channel": ch, "min": lo, "max": hi, "messages": n,
            "driver_addresses": mapped(Some(*cc), None),
        }))
        .chain(nrpn.iter().map(|(m, l)| json!({ "nrpn": [m, l], "driver_addresses": mapped(None, Some((*m, *l))) })))
        .collect();
    let text = format!("{} message(s) received. Controls moved: {}",
        messages.len(), serde_json::to_string_pretty(&moved).unwrap_or_default());
    Ok(content_structured(&text, json!({ "messages": messages, "controls": moved })))
}

/// NRPN parameter numbers selected in a stream of messages (CC 99 then 98).
fn nrpn_numbers(received: &[crate::relay::Inbound]) -> Vec<(u8, u8)> {
    let mut out: Vec<(u8, u8)> = Vec::new();
    let mut msb: Option<u8> = None;
    for m in received {
        if let [s, cc, v] = m.data[..] {
            if s & 0xF0 != 0xB0 { continue; }
            match cc {
                99 => msb = Some(v),
                98 => if let Some(hi) = msb {
                    if !out.contains(&(hi, v)) { out.push((hi, v)); }
                },
                _ => {}
            }
        }
    }
    out
}

/// What a driver says a control sends: its CC / NRPN / note, None when it
/// does not map that address.
fn driver_sends(dev: &Device, control: &str) -> Option<crate::reports::Sends> {
    use crate::reports::Sends;
    let rel = control.strip_prefix(dev.device.osc_prefix.as_str())?;
    if let Some(e) = dev.cc_params.iter().flat_map(|t| t.entries.iter()).find(|e| e.osc == rel) {
        return match (e.nrpn_msb, e.nrpn_lsb, e.cc) {
            (Some(msb), Some(lsb), _) => Some(Sends::Nrpn { msb, lsb }),
            (_, _, Some(cc)) => Some(Sends::Cc { cc }),
            _ => None,
        };
    }
    let t = dev.triggers.iter().flat_map(|t| t.entries.iter()).find(|e| e.osc == rel)?;
    t.note.or(Some(t.note_range[0])).map(|note| Sends::Note { note })
}

/// The ledger's learned mappings of a driver, each with what the driver
/// itself says and whether they differ.
fn mappings_against_driver(ledger: &crate::reports::Ledger, entry: &DeviceIndexEntry) -> Result<Vec<Value>, String> {
    let dev = Device::load(&entry.path).map_err(|e| format!("load {}: {e}", entry.id))?;
    Ok(ledger.learned(Some(&entry.id)).into_iter().map(|mut m| {
        let driver = m["control"].as_str().and_then(|c| driver_sends(&dev, c));
        m["driver"] = json!(driver);
        m["differs"] = json!(!m["learned"].is_null() && serde_json::to_value(&driver).ok() != Some(m["learned"].clone()));
        m
    }).collect())
}

/// Record which MIDI message a control sent while the user moved it (seen
/// with `relay_listen`, or any other way).
fn report_mapping(index: &DeviceIndex, args: &Value, caller: Caller) -> Result<Value, String> {
    use crate::reports::{DeviceIdentity, Observation, Sends};
    let ledger = caller.ledger.ok_or("mappings go to the hosted server: https://mcp.osc-bridge.roomi-fields.com/mcp")?;
    let user = caller.user.ok_or("no account: initialize the session first, or pass the user_id the server gave you")?;
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let num = |k: &str| args.get(k).and_then(Value::as_u64).filter(|n| *n < 128).map(|n| n as u8);
    let device = text("device").ok_or("missing device")?;
    let entry = index.find(device).ok_or_else(|| format!("no device '{device}' (use the id or slug from list_devices)"))?;
    let dev = Device::load(&entry.path).map_err(|e| format!("load {}: {e}", entry.id))?;
    let control = match (text("address"), text("label")) {
        (Some(a), _) => {
            let full = if a.starts_with(&format!("{}/", dev.device.osc_prefix)) { a.to_string() } else { format!("{}{a}", dev.device.osc_prefix) };
            if !surface_addresses(&dev).contains(&full) {
                return Err(format!("'{a}' is not an address of {}; for a control the driver lacks, give its panel label instead", entry.id));
            }
            full
        }
        (None, Some(l)) => format!("label:{}", l.chars().take(60).collect::<String>()),
        (None, None) => return Err("give the control's address (from list_routes) or, if the driver lacks it, its panel label".into()),
    };
    let sends = match (num("cc"), num("nrpn_msb"), num("nrpn_lsb"), num("note")) {
        (Some(cc), None, None, None) => Sends::Cc { cc },
        (None, Some(msb), Some(lsb), None) => Sends::Nrpn { msb, lsb },
        (None, None, None, Some(note)) => Sends::Note { note },
        _ => return Err("give exactly one of: cc, nrpn_msb + nrpn_lsb, note (0-127)".into()),
    };
    let identity = match text("identity_hex") {
        Some(h) => {
            let bytes = crate::librarian::parse_hex(h).map_err(|e| format!("identity_hex: {e}"))?;
            let id = crate::detect::parse_identity_reply(&bytes).ok_or("identity_hex is not a MIDI Identity Reply")?;
            if !entry.manufacturer_id.is_empty() && id.manufacturer != entry.manufacturer_id {
                return Err(format!("the device that answered is not from {}'s manufacturer", entry.id));
            }
            Some(DeviceIdentity::from(&id))
        }
        None => None,
    };
    ledger.observe(Observation {
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        user: user.to_string(),
        device: entry.id.clone(),
        control: control.clone(),
        sends,
        channel: num("channel").filter(|c| (1..=16).contains(c)),
        firmware: text("firmware").map(|f| f.chars().take(40).collect()),
        identity,
    })?;
    let mappings: Vec<Value> = mappings_against_driver(ledger, entry)?.into_iter().filter(|m| m["control"] == control.as_str()).collect();
    let text = format!("Observation recorded for {} {control}.\n\n{}", entry.id, serde_json::to_string_pretty(&mappings).unwrap_or_default());
    Ok(content_structured(&text, json!({ "recorded": true, "mappings": mappings })))
}

/// Largest driver file accepted as a contribution.
const MAX_CONTRIBUTION: usize = 256 * 1024;

/// `Moog Music` → `moog_music`: lower case, other characters as `sep`.
fn slugify(s: &str, sep: char) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() { out.push(c.to_ascii_lowercase()); }
        else if !out.ends_with(sep) && !out.is_empty() { out.push(sep); }
    }
    out.trim_end_matches(sep).to_string()
}

/// The catalogue folder of a vendor: the one its drivers already use, else
/// a new one from its name.
fn vendor_dir(index: &DeviceIndex, vendor: &str) -> String {
    index.entries.iter().find(|e| e.vendor.eq_ignore_ascii_case(vendor))
        .and_then(|e| e.id.split('/').next().map(str::to_string))
        .unwrap_or_else(|| slugify(vendor, '-'))
}

fn queue_contribution(caller: Caller, path: String, doc: &Value, kind: &str, note: Option<String>) -> Result<Value, String> {
    let ledger = caller.ledger.ok_or("contributions go to the hosted server: https://mcp.osc-bridge.roomi-fields.com/mcp")?;
    let user = caller.user.ok_or("no account: initialize the session first, or pass the user_id the server gave you")?;
    let content = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())? + "\n";
    if content.len() > MAX_CONTRIBUTION {
        return Err(format!("driver file too large ({} bytes, {MAX_CONTRIBUTION} at most)", content.len()));
    }
    let id = ledger.contribute(crate::reports::Contribution {
        id: 0,
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        user: user.to_string(), path: path.clone(), content, kind: kind.to_string(), note,
    })?;
    let text = format!("Contribution {id} queued as {path}. The catalogue's merge job checks it (parse, lint, tests) \
        and merges it within the hour if it passes; check with get_contribution.");
    Ok(content_structured(&text, json!({ "id": id, "path": path, "state": "pending" })))
}

/// A new driver file from an agent: checked here as a driver, then queued.
fn submit_driver(index: &DeviceIndex, args: &Value, caller: Caller) -> Result<Value, String> {
    let mut doc = args.get("driver").cloned().filter(Value::is_object).ok_or("missing driver (the driver file, as a JSON object)")?;
    let dev: Device = serde_json::from_value(doc.clone()).map_err(|e| format!("not a valid driver: {e} (see DEVICE_JSON_SCHEMA)"))?;
    let (name, vendor, prefix) = (&dev.device.name, &dev.device.vendor, &dev.device.osc_prefix);
    if name.trim().is_empty() || vendor.trim().is_empty() || !prefix.starts_with('/') || prefix.len() < 2 {
        return Err("device.name, device.vendor and device.osc_prefix ('/slug') are required".into());
    }
    let has_surface = !dev.commands.is_empty() || dev.cc_params.as_ref().is_some_and(|t| !t.entries.is_empty())
        || dev.params.as_ref().is_some_and(|t| !t.entries.is_empty()) || dev.triggers.as_ref().is_some_and(|t| !t.entries.is_empty());
    if !has_surface {
        return Err("the driver maps nothing: add cc_params, params, commands or triggers".into());
    }
    let firmware = args.get("firmware").and_then(Value::as_str).map(str::trim).filter(|f| !f.is_empty()).unwrap_or("unknown");
    let note = args.get("note").and_then(Value::as_str).map(|n| n.chars().take(500).collect::<String>());
    let source = json!({
        "type": "agent-contributed",
        "submitted": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "firmware": firmware,
        "how": args.get("source").and_then(Value::as_str).map(|s| s.chars().take(300).collect::<String>()),
    });
    doc["_sources"] = json!([source]);
    let path = format!("devices/{}/{}.agent-contributed.fw-{}.json", vendor_dir(index, vendor), slugify(name, '_'), slugify(firmware, '_'));
    queue_contribution(caller, path, &doc, "driver", note)
}

/// A corrected variant of a driver, built from the mappings the catalogue
/// learned for one firmware where they differ from it.
fn submit_fix(index: &DeviceIndex, args: &Value, caller: Caller) -> Result<Value, String> {
    use crate::reports::Sends;
    let ledger = caller.ledger.ok_or("contributions go to the hosted server")?;
    let device = args.get("device").and_then(Value::as_str).ok_or("missing device")?;
    let entry = index.find(device).ok_or_else(|| format!("no device '{device}'"))?;
    let wanted = args.get("firmware").and_then(Value::as_str);
    let differing: Vec<Value> = mappings_against_driver(ledger, entry)?.into_iter()
        .filter(|m| m["differs"] == true && wanted.is_none_or(|w| m["firmware"] == w)).collect();
    let firmwares: std::collections::BTreeSet<&str> = differing.iter().filter_map(|m| m["firmware"].as_str()).collect();
    let firmware = match firmwares.len() {
        0 => return Err(format!("no learned mapping differs from {} yet (see get_trust)", entry.id)),
        1 => firmwares.into_iter().next().unwrap_or_default().to_string(),
        _ => return Err(format!("learned mappings differ on several firmwares ({}): pass firmware", firmwares.into_iter().collect::<Vec<_>>().join(", "))),
    };
    let raw = std::fs::read_to_string(&entry.path).map_err(|e| format!("read {}: {e}", entry.id))?;
    let mut doc: Value = serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", entry.id))?;
    let prefix = doc["device"]["osc_prefix"].as_str().unwrap_or("").to_string();
    let mut applied = Vec::new();
    for m in differing.iter().filter(|m| m["firmware"] == firmware.as_str()) {
        let Ok(sends) = serde_json::from_value::<Sends>(m["learned"].clone()) else { continue };
        let control = m["control"].as_str().unwrap_or("");
        let entries = doc["cc_params"]["entries"].as_array_mut();
        let target: Option<&mut Value> = match (control.strip_prefix(&prefix), entries) {
            (Some(rel), Some(list)) => list.iter_mut().find(|e| e["osc"] == rel),
            (None, Some(list)) if control.starts_with("label:") => {
                let label = &control["label:".len()..];
                list.push(json!({ "osc": format!("/learned/{}", slugify(label, '_')), "label": label, "range": [0, 127] }));
                list.last_mut()
            }
            _ => None,
        };
        let Some(e) = target else { continue };
        let obj = e.as_object_mut().ok_or("bad cc_params entry")?;
        match sends {
            Sends::Cc { cc } => { obj.insert("cc".into(), json!(cc)); obj.remove("nrpn_msb"); obj.remove("nrpn_lsb"); }
            Sends::Nrpn { msb, lsb } => { obj.insert("nrpn_msb".into(), json!(msb)); obj.insert("nrpn_lsb".into(), json!(lsb)); obj.remove("cc"); }
            Sends::Note { .. } => continue,
        }
        applied.push(control.to_string());
    }
    if applied.is_empty() {
        return Err("the differing mappings are on controls this driver cannot express as CC or NRPN".into());
    }
    let original = doc["_sources"].get(0).cloned().unwrap_or(json!({}));
    doc["_sources"] = json!([{
        "type": "agent-contributed",
        "submitted": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "firmware": firmware,
        "derived_from": entry.id,
        "how": format!("{} with the mappings learned from usage on firmware {firmware}: {}", entry.id, applied.join(", ")),
        "license": original.get("license"),
        "attribution": original.get("attribution"),
    }]);
    let stem = entry.id.rsplit('/').next().unwrap_or(&entry.id).split('.').next().unwrap_or("driver").to_string();
    let path = format!("devices/{}/{stem}.agent-contributed.fw-{}.json", entry.id.split('/').next().unwrap_or("misc"), slugify(&firmware, '_'));
    queue_contribution(caller, path, &doc, "fix", Some(format!("learned mappings: {}", applied.join(", "))))
}

fn load_device_by_slug(index: &DeviceIndex, slug: &str) -> Result<Device, String> {
    let path = index.find_path(slug)
        .ok_or_else(|| format!("no device with slug '{slug}'"))?;
    Device::load(path).map_err(|e| format!("load {}: {e}", path.display()))
}

fn get_device_docs(index: &DeviceIndex, slug: &str) -> Result<Value, String> {
    let dev = load_device_by_slug(index, slug)?;
    let (has_docs, markdown) = match dev.docs.as_deref() {
        Some(md) => (true, md.to_string()),
        None => (false, format!("No companion .md found for device '{slug}'.")),
    };
    let structured = json!({ "slug": slug, "has_docs": has_docs, "markdown": markdown });
    Ok(content_structured(&markdown, structured))
}

fn list_routes(index: &DeviceIndex, slug: &str) -> Result<Value, String> {
    let dev = load_device_by_slug(index, slug)?;
    let prefix = dev.device.osc_prefix.clone();
    let mut cmds: Vec<Value> = Vec::new();
    for c in &dev.commands {
        let mode = if c.forward.is_some() { "forward (OSC)" } else { "frame (MIDI/SysEx)" };
        cmds.push(json!({
            "address": format!("{prefix}{}", c.osc),
            "args": c.args.iter().map(|a| json!({"name": a.name, "type": a.ty})).collect::<Vec<_>>(),
            "mode": mode,
        }));
    }
    let cc_params: Vec<Value> = dev.cc_params.as_ref().map(|t| t.entries.iter().map(|e| json!({
        "address": format!("{prefix}{}", e.osc),
        "cc": e.cc,
        "cc_lsb": e.cc_lsb,
        "nrpn_msb": e.nrpn_msb,
        "nrpn_lsb": e.nrpn_lsb,
        "range": e.range,
        "expects": e.expects,
    })).collect()).unwrap_or_default();
    let params: Vec<Value> = dev.params.as_ref().map(|t| t.entries.iter().map(|e| json!({
        "address": format!("{prefix}{}", e.osc),
        "range": e.range,
        "expects": e.expects,
    })).collect()).unwrap_or_default();
    let triggers: Vec<Value> = dev.triggers.as_ref().map(|t| t.entries.iter().map(|e| json!({
        "address": format!("{prefix}{}", e.osc),
        "sound": e.sound,
        "section": e.section,
        "channel": e.channel,
        "channel_mode": e.channel_mode,
        "note_range": e.note_range,
        "note": e.note,
        "velocity_usage": e.velocity_usage,
    })).collect()).unwrap_or_default();
    let replies: Vec<Value> = dev.replies.iter().map(|r| json!({
        "match_osc": r.match_osc,
        "match_frame_len": r.match_frame.len(),
        "emit_osc": r.emit_osc,
    })).collect();
    let payload = json!({
        "device": dev.device.name,
        "vendor": dev.device.vendor,
        "osc_prefix": prefix,
        "kind": dev.device.kind,
        "commands": cmds,
        "cc_params": cc_params,
        "params": params,
        "triggers": triggers,
        "replies": replies,
    });
    let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
    Ok(content_structured(&text, payload))
}

fn send_osc_message(addr: &str, args: &Value, target: SocketAddr) -> Result<Value, String> {
    use rosc::{OscMessage, OscPacket, OscType, encoder};
    let osc_args: Vec<OscType> = args.as_array().map(|a| a.iter().map(json_to_osc).collect()).unwrap_or_default();
    let pkt = OscPacket::Message(OscMessage { addr: addr.to_string(), args: osc_args });
    let bytes = encoder::encode(&pkt).map_err(|e| format!("osc encode: {e:?}"))?;
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.send_to(&bytes, target).map_err(|e| e.to_string())?;
    let structured = json!({
        "sent": true,
        "address": addr,
        "bytes": bytes.len(),
        "target": target.to_string(),
    });
    Ok(content_structured(&format!("Sent {addr} ({} bytes) to {target}.", bytes.len()), structured))
}

fn get_status(target: SocketAddr) -> Result<Value, String> {
    use rosc::{OscMessage, OscPacket, encoder};
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(Duration::from_millis(500))).map_err(|e| e.to_string())?;
    let pkt = OscPacket::Message(OscMessage { addr: "/bridge/status".into(), args: vec![] });
    let bytes = encoder::encode(&pkt).map_err(|e| format!("osc encode: {e:?}"))?;
    sock.send_to(&bytes, target).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 65536];
    let mut replies: Vec<Value> = Vec::new();
    while let Ok((n, _)) = sock.recv_from(&mut buf) {
        if let Ok((_, pkt)) = rosc::decoder::decode_udp(&buf[..n]) {
            collect_messages(pkt, &mut replies);
        }
    }
    let replied = !replies.is_empty();
    let text = if replied {
        serde_json::to_string_pretty(&replies).unwrap_or_default()
    } else {
        format!("No reply from {target} within 500 ms. Is osc-bridge running on that port?")
    };
    let structured = json!({ "target": target.to_string(), "replied": replied, "replies": replies });
    Ok(content_structured(&text, structured))
}

fn collect_messages(pkt: rosc::OscPacket, out: &mut Vec<Value>) {
    use rosc::OscPacket;
    match pkt {
        OscPacket::Message(m) => out.push(json!({
            "addr": m.addr,
            "args": m.args.iter().map(osc_to_json).collect::<Vec<_>>(),
        })),
        OscPacket::Bundle(b) => for p in b.content { collect_messages(p, out); }
    }
}

fn json_to_osc(v: &Value) -> rosc::OscType {
    use rosc::OscType;
    match v {
        Value::Bool(b) => OscType::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i32::MIN as i64 <= i && i <= i32::MAX as i64 {
                    OscType::Int(i as i32)
                } else {
                    OscType::Long(i)
                }
            } else if let Some(f) = n.as_f64() {
                OscType::Float(f as f32)
            } else {
                OscType::Nil
            }
        }
        Value::String(s) => OscType::String(s.clone()),
        _ => OscType::Nil,
    }
}

fn osc_to_json(v: &rosc::OscType) -> Value {
    use rosc::OscType;
    match v {
        OscType::Int(i) => json!(i),
        OscType::Long(i) => json!(i),
        OscType::Float(f) => json!(f),
        OscType::Double(d) => json!(d),
        OscType::Bool(b) => json!(b),
        OscType::String(s) => json!(s),
        other => json!(format!("{other:?}")),
    }
}

/// A tool result carrying both a human-readable text block and the
/// machine-readable `structuredContent` its `outputSchema` describes. The
/// text block is the spec-recommended fallback for clients that don't read
/// `structuredContent`.
fn content_structured(text: &str, structured: Value) -> Value {
    json!({
        "content": [ { "type": "text", "text": text } ],
        "structuredContent": structured,
    })
}

fn walk_json<F: FnMut(&Path, &Value)>(dir: &Path, f: &mut F) -> std::io::Result<()> {
    if dir.is_file() {
        if dir.extension().and_then(|s| s.to_str()) == Some("json") {
            if let Ok(txt) = std::fs::read_to_string(dir) {
                if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                    f(dir, &v);
                }
            }
        }
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_json(&path, f)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
            if let Ok(txt) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                    f(&path, &v);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty index — these handshake/dispatch tests don't touch devices.
    fn empty_index() -> DeviceIndex {
        DeviceIndex { entries: Vec::new(), by_slug: HashMap::new() }
    }

    fn hosted_for_tests() -> Hosted {
        Hosted {
            ledger: std::sync::Arc::new(crate::reports::Ledger::open(None).unwrap()),
            relays: std::sync::Arc::new(crate::relay::Relays::default()),
            usage: UsageLog::open(None).unwrap(),
            admin_token: Some("x".repeat(16)),
            sync_token: Some("s".repeat(16)),
            catalogue_root: PathBuf::from("devices"),
            catalogue: std::sync::OnceLock::new(),
        }
    }

    fn test_opts() -> McpOptions {
        McpOptions {
            devices_dir: PathBuf::from("devices"),
            default_target: "127.0.0.1:7777".parse().unwrap(),
            hosted: false,
            online: false,
        }
    }

    #[test]
    fn list_devices_filters_by_words_vendor_kind_and_limit() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let run = |args: Value| list_devices(&idx, &DeviceFilter::from_args(&args), &|_| None).unwrap()["structuredContent"].clone();
        let all = run(json!({}));
        assert_eq!(all["count"].as_u64().unwrap() as usize, idx.entries.len());
        let mini = run(json!({"query": "KORG minilogue"}));
        let names: Vec<&str> = mini["devices"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("minilogue")), "{names:?}");
        assert!(names.iter().all(|n| n.to_lowercase().contains("minilogue")), "{names:?}");
        let moog = run(json!({"vendor": "moog", "limit": 2}));
        assert!(moog["count"].as_u64().unwrap() > 2);
        assert_eq!(moog["devices"].as_array().unwrap().len(), 2);
        let software = run(json!({"kind": "software"}));
        assert!(software["devices"].as_array().unwrap().iter().all(|d| d["kind"] == "software"));
    }

    #[test]
    fn hosted_server_offers_and_runs_only_hosted_tools() {
        let opts = McpOptions { hosted: true, ..test_opts() };
        let listed: Vec<String> = tools_list(&opts)["tools"].as_array().unwrap().iter()
            .map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(listed, HOSTED_TOOLS);
        let resp = handle_request(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "send", "arguments": {"addr": "/x"}}}), &opts, &empty_index(), Caller::default()).unwrap();
        assert_eq!(resp["error"]["code"], json!(-32602));
    }

    #[test]
    fn http_exchange_answers_posts_notifications_and_rejects_the_rest() {
        let opts = McpOptions { hosted: true, ..test_opts() };
        let idx = empty_index();
        let store = hosted_for_tests();
        let req = |method: &str, path: &str, body: &str| HttpRequest {
            method: method.into(), path: path.into(), body: Ok(body.into()), user: None, bearer: None, client: None,
        };
        let r = http_exchange(&req("POST", "/mcp", r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#), &opts, &idx, &store);
        assert_eq!(r.status, 200);
        let v: Value = serde_json::from_str(&r.body.unwrap()).unwrap();
        assert_eq!(v["id"], json!(7));
        assert_eq!(v["result"]["tools"].as_array().unwrap().len(), HOSTED_TOOLS.len());
        let r = http_exchange(&req("POST", "/mcp", r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), &opts, &idx, &store);
        assert_eq!((r.status, r.body), (202, None));
        let status = |m: &str, p: &str, b: &str| http_exchange(&req(m, p, b), &opts, &idx, &store).status;
        assert_eq!(status("POST", "/mcp", "not json"), 400);
        assert_eq!(status("GET", "/mcp", ""), 405);
        assert_eq!(status("GET", "/", ""), 200);
        assert_eq!(status("GET", "/healthz", ""), 200);
        assert_eq!(status("POST", "/other", ""), 404);
        let call = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_devices","arguments":{"query":"moog"}}}"#;
        let usage: Value = serde_json::from_str(&http_exchange(&req("POST", "/mcp", call), &opts, &idx, &store).usage.unwrap()).unwrap();
        assert_eq!((usage["tool"].as_str(), usage["query"].as_str()), (Some("list_devices"), Some("moog")));
        assert!(http_exchange(&req("POST", "/mcp", r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#), &opts, &idx, &store).usage.is_none());
    }

    /// One request to the hosted server, as an HTTP exchange.
    #[allow(clippy::too_many_arguments)]
    fn exchange(opts: &McpOptions, idx: &DeviceIndex, hosted: &Hosted, method: &str, path: &str,
                header: Option<&str>, bearer: Option<&str>, body: &str) -> HttpReply {
        let (path, user) = user_id(path, header);
        http_exchange(&HttpRequest { method: method.into(), path, body: Ok(body.into()), user,
            bearer: bearer.map(str::to_string), client: None }, opts, idx, hosted)
    }

    #[test]
    fn the_server_issues_accounts_and_refuses_ids_it_did_not_issue() {
        let opts = McpOptions { hosted: true, ..test_opts() };
        let idx = empty_index();
        let hosted = hosted_for_tests();
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let fresh = exchange(&opts, &idx, &hosted, "POST", "/mcp", None, None, init).session.expect("initialize issues an account");
        assert!(is_user_id(&fresh) && hosted.ledger.knows(&fresh));
        assert_ne!(exchange(&opts, &idx, &hosted, "POST", "/mcp", None, None, init).session.unwrap(), fresh, "ids are random");
        // The same account comes back through its personal endpoint or the session header.
        assert_eq!(exchange(&opts, &idx, &hosted, "POST", &format!("/u/{fresh}/mcp"), None, None, init).session.unwrap(), fresh);
        assert_eq!(exchange(&opts, &idx, &hosted, "POST", "/mcp", Some(&fresh), None, init).session.unwrap(), fresh);
        let body: Value = serde_json::from_str(&exchange(&opts, &idx, &hosted, "POST", "/mcp", None, None, init).body.unwrap()).unwrap();
        assert!(body["result"]["instructions"].as_str().unwrap().contains("/u/"));
        // An id the server never issued is refused, whatever the way.
        let forged = "0123456789abcdef0123456789abcdef";
        assert_eq!(exchange(&opts, &idx, &hosted, "POST", &format!("/u/{forged}/mcp"), None, None, init).status, 404);
        assert_eq!(exchange(&opts, &idx, &hosted, "POST", "/mcp", Some(forged), None, init).status, 404);
        let with_arg = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"arguments":{{"user_id":"{forged}"}}}}}}"#);
        let r = exchange(&opts, &idx, &hosted, "POST", "/mcp", None, None, &with_arg);
        assert_ne!(r.session.unwrap(), forged, "a forged id in the arguments is ignored");
        let with_arg = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"arguments":{{"user_id":"{fresh}"}}}}}}"#);
        assert_eq!(exchange(&opts, &idx, &hosted, "POST", "/mcp", None, None, &with_arg).session.unwrap(), fresh);
        let schema = &tools_list(&opts)["tools"][0]["inputSchema"]["properties"]["user_id"];
        assert_eq!(schema["type"], "string");
        assert_eq!(user_id("/u/not-an-id/mcp", None), ("/u/not-an-id/mcp".to_string(), None));
        let new: Value = serde_json::from_str(&exchange(&opts, &idx, &hosted, "GET", "/u/new", None, None, "").body.unwrap()).unwrap();
        assert!(hosted.ledger.knows(new["user_id"].as_str().unwrap()));
    }

    #[test]
    fn reports_over_http_build_trust_roll_back_and_reach_the_administrator() {
        let opts = McpOptions { hosted: true, ..test_opts() };
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let hosted = hosted_for_tests();
        let call = |user: &str, tool: &str, args: Value| {
            let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}});
            let r = exchange(&opts, &idx, &hosted, "POST", "/mcp", Some(user), None, &body.to_string());
            serde_json::from_str::<Value>(&r.body.unwrap()).unwrap()["result"].clone()
        };
        let users: Vec<String> = (0..4).map(|_| hosted.ledger.issue_account().unwrap()).collect();
        let works = json!({"device": "korg/minilogue", "outcome": "works", "firmware": "2.1",
            "identity_hex": "F0 7E 00 06 02 42 2C 01 00 00 02 01 00 00 F7"});
        for u in &users[..3] {
            let r = call(u, "report_result", works.clone());
            assert_eq!(r["structuredContent"]["status"], "accepted", "{r}");
        }
        let trust = call(&users[0], "get_trust", json!({"device": "minilogue"}));
        assert_eq!(trust["structuredContent"]["overall"]["status"], "usage-verified");
        let bad = call(&users[0], "report_result", json!({"device": "korg/minilogue", "outcome": "works", "address": "/minilogue/nope"}));
        assert_eq!(bad["isError"], true);
        let liar = call(&users[3], "report_result", json!({"device": "korg/minilogue", "outcome": "fails", "firmware": "2.1"}));
        assert_eq!(liar["structuredContent"]["status"], "rolled-back");
        assert_eq!(liar["structuredContent"]["trust"]["overall"]["status"], "usage-verified");
        let listed = call(&users[0], "list_devices", json!({"query": "korg minilogue", "limit": 50}));
        let entry = listed["structuredContent"]["devices"].as_array().unwrap().iter()
            .find(|d| d["id"] == "korg/minilogue").cloned().unwrap();
        assert_eq!(entry["usage_status"], "usage-verified");
        let all: Value = serde_json::from_str(&exchange(&opts, &idx, &hosted, "GET", "/v1/trust", None, None, "").body.unwrap()).unwrap();
        assert_eq!(all["devices"]["korg/minilogue"]["rolled_back"], 1);
        // The administrator sees the suspect account and clears it.
        let token = "x".repeat(16);
        assert_eq!(exchange(&opts, &idx, &hosted, "GET", "/admin/pending", None, None, "").status, 401);
        assert_eq!(exchange(&opts, &idx, &hosted, "GET", "/admin/pending", None, Some("wrong-token-wrong"), "").status, 401);
        let pending: Value = serde_json::from_str(&exchange(&opts, &idx, &hosted, "GET", "/admin/pending", None, Some(&token), "").body.unwrap()).unwrap();
        assert_eq!(pending["suspect_accounts"][0], users[3].as_str());
        let clear = json!({"decision": "clear", "user": users[3]}).to_string();
        assert_eq!(exchange(&opts, &idx, &hosted, "POST", "/admin/decide", None, Some(&token), &clear).status, 200);
        let nonsense = json!({"decision": "approve", "user": users[3]}).to_string();
        assert_eq!(exchange(&opts, &idx, &hosted, "POST", "/admin/decide", None, Some(&token), &nonsense).status, 400);
        let off = Hosted { admin_token: None, ..hosted_for_tests() };
        assert_eq!(exchange(&opts, &idx, &off, "GET", "/admin/pending", None, Some(&token), "").status, 404);
    }

    #[test]
    fn an_agent_detects_plays_and_listens_through_a_relay_door() {
        use tungstenite::Message;
        let opts = McpOptions { hosted: true, ..test_opts() };
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let hosted = hosted_for_tests();
        let addr = crate::relay::serve("127.0.0.1:0", hosted.relays.clone(),
            std::sync::Arc::new(LedgerAccounts(hosted.ledger.clone())), vec![], false).unwrap();
        // A door that owns a minilogue: answers the Identity Request, and
        // turns the cutoff knob when asked to play anything.
        let (mut ws, _) = tungstenite::connect(format!("ws://{addr}/relay")).unwrap();
        let hello: Value = match ws.read().unwrap() { Message::Text(t) => serde_json::from_str(&t).unwrap(), m => panic!("{m:?}") };
        let code = hello["code"].as_str().unwrap().to_string();
        assert!(hosted.ledger.knows(hello["user"].as_str().unwrap()), "the door gets an issued account");
        ws.send(Message::Text(json!({"type": "ports", "outputs": ["minilogue SOUND"], "inputs": ["minilogue SOUND"]}).to_string().into())).unwrap();
        let door = std::thread::spawn(move || {
            let mut played = Vec::new();
            while let Ok(m) = ws.read() {
                let Message::Text(t) = m else { continue };
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] != "midi_out" { continue; }
                let data: Vec<u8> = v["data"].as_array().unwrap().iter().map(|b| b.as_u64().unwrap() as u8).collect();
                let reply = if data == crate::detect::IDENTITY_REQUEST {
                    vec![0xF0, 0x7E, 0x00, 0x06, 0x02, 0x42, 0x2C, 0x01, 0x00, 0x00, 0x02, 0x01, 0x00, 0x00, 0xF7]
                } else {
                    played.push(data);
                    vec![0xB0, 43, 90]
                };
                ws.send(Message::Text(json!({"type": "midi_in", "port": "minilogue SOUND", "data": reply}).to_string().into())).unwrap();
                if played.len() == 2 { return played; }
            }
            played
        });
        let user = hosted.ledger.issue_account().unwrap();
        let call = |tool: &str, args: Value| {
            let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}});
            let r = exchange(&opts, &idx, &hosted, "POST", "/mcp", Some(&user), None, &body.to_string());
            serde_json::from_str::<Value>(&r.body.unwrap()).unwrap()["result"].clone()
        };
        let status = (0..300).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let r = call("relay_status", json!({"code": code.to_lowercase()}));
            (r["structuredContent"]["outputs"][0] == "minilogue SOUND").then_some(r)
        }).expect("the door's ports reach the server");
        assert_eq!(status["structuredContent"]["inputs"][0], "minilogue SOUND");
        let found = call("relay_detect", json!({"code": code, "timeout_ms": 300}));
        let device = &found["structuredContent"]["devices"][0];
        assert_eq!(device["identity"]["version"], "02 01 00 00", "{found}");
        assert_eq!(device["matches"][0]["id"], "korg/minilogue", "{found}");
        let sent = call("relay_send", json!({"code": code, "device": "korg/minilogue", "address": "/filter/cutoff", "args": [45]}));
        assert_eq!(sent["structuredContent"]["messages"][0], "B0 2B 2D", "{sent}");
        let listen_args = json!({"code": code, "seconds": 1, "device": "korg/minilogue"});
        let listening = std::thread::scope(|s| {
            let h = s.spawn(|| call("relay_listen", listen_args));
            std::thread::sleep(std::time::Duration::from_millis(200));
            call("relay_send", json!({"code": code, "hex": "90 3C 64"}));
            h.join().unwrap()
        });
        let moved = &listening["structuredContent"]["controls"][0];
        assert_eq!(moved["cc"], 43, "{listening}");
        assert_eq!(moved["driver_addresses"][0], "/minilogue/filter/cutoff");
        assert_eq!(door.join().unwrap(), vec![vec![0xB0, 43, 45], vec![0x90, 0x3C, 0x64]]);
        let mapped = call("report_mapping", json!({"device": "korg/minilogue", "address": "/filter/cutoff", "cc": 43, "firmware": "2.1"}));
        assert_eq!(mapped["structuredContent"]["mappings"][0]["driver"], json!({"kind": "cc", "cc": 43}), "{mapped}");
        assert_eq!(mapped["structuredContent"]["mappings"][0]["differs"], false);
        let unmapped = call("report_mapping", json!({"device": "korg/minilogue", "address": "/nope", "cc": 1}));
        assert_eq!(unmapped["isError"], true);
        let other = hosted.ledger.issue_account().unwrap();
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "relay_status", "arguments": {"code": code}}});
        let stolen: Value = serde_json::from_str(&exchange(&opts, &idx, &hosted, "POST", "/mcp", Some(&other), None, &body.to_string()).body.unwrap()).unwrap();
        assert_eq!(stolen["result"]["isError"], true, "another agent cannot drive a paired door");
        let gone = call("relay_status", json!({"code": "ZZZZ-ZZZZ"}));
        assert_eq!(gone["isError"], true);
        assert!(split_midi(&[0xB0, 0x2B]).is_err());
        assert_eq!(split_midi(&[0xF0, 0x7E, 0xF7, 0xC0, 5]).unwrap(), vec![vec![0xF0, 0x7E, 0xF7], vec![0xC0, 5]]);
    }

    #[test]
    fn agents_contribute_drivers_and_fixes_and_the_merge_job_settles_them() {
        let opts = McpOptions { hosted: true, ..test_opts() };
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let hosted = hosted_for_tests();
        let users: Vec<String> = (0..3).map(|_| hosted.ledger.issue_account().unwrap()).collect();
        let call = |user: &str, tool: &str, args: Value| {
            let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}});
            let r = exchange(&opts, &idx, &hosted, "POST", "/mcp", Some(user), None, &body.to_string());
            serde_json::from_str::<Value>(&r.body.unwrap()).unwrap()["result"].clone()
        };
        let driver = json!({
            "device": {"name": "Box 1", "vendor": "Moog", "revision": "1", "osc_prefix": "/box1", "manufacturer_id": [4], "device_id": []},
            "sysex": {"header": [], "footer": []},
            "cc_params": {"channel": 0, "entries": [{"osc": "/filter/cutoff", "cc": 19, "range": [0, 127]}]}
        });
        let r = call(&users[0], "submit_driver", json!({"driver": driver, "firmware": "1.0", "source": "manual p. 3"}));
        assert_eq!(r["structuredContent"]["path"], "devices/moog/box_1.agent-contributed.fw-1_0.json", "{r}");
        let empty = call(&users[0], "submit_driver", json!({"driver": {"device": {"name": "X", "vendor": "Y", "osc_prefix": "/x"}}}));
        assert_eq!(empty["isError"], true, "{empty}");
        // Learned: the minilogue's cutoff sends CC 74 on firmware 2.1, not 43.
        for u in &users {
            call(u, "report_mapping", json!({"device": "korg/minilogue", "address": "/filter/cutoff", "cc": 74, "firmware": "2.1"}));
        }
        let fix = call(&users[1], "submit_fix", json!({"device": "korg/minilogue"}));
        assert_eq!(fix["structuredContent"]["path"], "devices/korg/minilogue.agent-contributed.fw-2_1.json", "{fix}");
        // The merge job takes them with its token, and reports back.
        let sync = "s".repeat(16);
        assert_eq!(exchange(&opts, &idx, &hosted, "GET", "/admin/contributions", None, None, "").status, 401);
        assert_eq!(exchange(&opts, &idx, &hosted, "GET", "/admin/pending", None, Some(&sync), "").status, 401, "the sync token opens contributions only");
        let pending: Value = serde_json::from_str(&exchange(&opts, &idx, &hosted, "GET", "/admin/contributions", None, Some(&sync), "").body.unwrap()).unwrap();
        let list = pending["contributions"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        let fixed: Value = serde_json::from_str(list.iter().find(|c| c["kind"] == "fix").unwrap()["content"].as_str().unwrap()).unwrap();
        let cutoff = fixed["cc_params"]["entries"].as_array().unwrap().iter().find(|e| e["osc"] == "/filter/cutoff").unwrap();
        assert_eq!(cutoff["cc"], 74);
        assert_eq!(fixed["_sources"][0]["derived_from"], "korg/minilogue");
        let id = list[0]["id"].as_u64().unwrap();
        let settle = json!({"state": "merged", "pr": "https://github.com/roomi-fields/osc-bridge/pull/9"}).to_string();
        assert_eq!(exchange(&opts, &idx, &hosted, "POST", &format!("/admin/contributions/{id}"), None, Some(&sync), &settle).status, 200);
        let status = call(&users[0], "get_contribution", json!({"id": id}));
        assert_eq!(status["structuredContent"]["state"], "merged");
    }

    /// The hosted server, for real, on a free local port, serving the
    /// catalogue under `root`.
    fn serve_hosted(root: &Path) -> String {
        let hosted = Hosted { catalogue_root: root.to_path_buf(), ..hosted_for_tests() };
        let opts = McpOptions { hosted: true, devices_dir: root.to_path_buf(), ..test_opts() };
        let index = DeviceIndex::build(root);
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let reply = http_exchange(&read_request(&mut req), &opts, &index, &hosted);
                respond(req, reply);
            }
        });
        base
    }

    #[test]
    fn a_local_copy_mirrors_the_server_catalogue_and_forwards_reports() {
        let tmp = std::env::temp_dir().join(format!("osc-bridge-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (served, bundled, cache) = (tmp.join("served"), tmp.join("bundled"), tmp.join("cache"));
        for (dir, files) in [(&served, &["korg/minilogue.json", "examples/test_synth.md", "examples/test_synth.json"][..]),
                             (&bundled, &["korg/minilogue.json"][..])] {
            for f in files {
                std::fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
                std::fs::copy(Path::new("devices").join(f), dir.join(f)).unwrap();
            }
        }
        std::fs::create_dir_all(cache.join("old")).unwrap();
        std::fs::write(cache.join("old/gone.json"), "{}").unwrap();
        let link = Link {
            client: crate::online::Online::keeping_account_in(&serve_hosted(&served), Some(tmp.join("user"))),
            ..Link::new("unused")
        };
        let user = link.user(false).unwrap();
        assert_eq!(std::fs::read_to_string(tmp.join("user")).unwrap(), user, "the account is kept");
        let done = link.client.sync_catalogue(&user, &bundled, &cache).unwrap();
        assert_eq!(done, crate::online::Synced { files: 3, copied: 1, downloaded: 2, removed: 1 });
        assert_eq!(crate::online::catalogue_files(&cache), crate::online::catalogue_files(&served));
        let again = link.client.sync_catalogue(&user, &bundled, &cache).unwrap();
        assert!(!again.changed(), "nothing to do the second time");

        let opts = McpOptions { online: true, ..test_opts() };
        let index = DeviceIndex::build(&cache);
        let caller = Caller { online: Some(&link), ..Caller::default() };
        let reported = call_tool("report_result", &json!({"device": "korg/minilogue", "outcome": "works", "firmware": "2.10"}),
            &opts, &index, caller).unwrap();
        assert_ne!(reported["isError"], json!(true), "{reported}");
        if let Ok(mut t) = link.trust.write() { *t = link.client.trust().unwrap(); }
        let listed = call_tool("list_devices", &json!({"query": "minilogue"}), &opts, &index, caller).unwrap();
        assert_eq!(listed["structuredContent"]["devices"][0]["usage_status"], "unconfirmed");
        assert!(tool_names(&opts).contains(&"submit_fix") && !tool_names(&test_opts()).contains(&"submit_fix"));

        std::fs::write(tmp.join("user"), "0".repeat(32)).unwrap();
        let relinked = Link { client: crate::online::Online::keeping_account_in(link.client.base(), Some(tmp.join("user"))), ..Link::new("unused") };
        let trust = call_tool("get_trust", &json!({"device": "korg/minilogue"}), &opts, &index,
            Caller { online: Some(&relinked), ..Caller::default() }).unwrap();
        assert_ne!(trust["isError"], json!(true), "an account the server forgot is replaced: {trust}");
        assert_ne!(std::fs::read_to_string(tmp.join("user")).unwrap(), "0".repeat(32));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn report_tools_refuse_without_the_hosted_store() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let err = call_tool("report_result", &json!({"device": "korg/minilogue", "outcome": "works"}), &test_opts(), &idx, Caller::default()).unwrap_err();
        assert!(err.contains("hosted"), "{err}");
    }

    #[test]
    fn rate_limiter_caps_each_client_separately() {
        let limiter = RateLimiter::default();
        assert!((0..RATE_LIMIT).all(|_| limiter.allow("a")));
        assert!(!limiter.allow("a"));
        assert!(limiter.allow("b"));
    }

    #[test]
    fn device_index_builds_from_devices_dir() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        assert!(idx.entries.len() > 100, "expected the full catalogue, got {}", idx.entries.len());
        // The Ableton driver has a stable slug — lookup must be O(1) and hit.
        let p = idx.find_path("ableton").expect("/ableton slug should resolve");
        assert!(p.to_string_lossy().contains("ableton"));
        // Unknown slug resolves to nothing rather than walking the tree.
        assert!(idx.find_path("does-not-exist").is_none());
    }

    #[test]
    fn tools_list_has_eight_tools() {
        let t = tools_list(&test_opts());
        let arr = t.get("tools").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 8);
        let names: Vec<&str> = arr.iter().map(|t| t.get("name").unwrap().as_str().unwrap()).collect();
        for expected in ["list_devices", "get_device_docs", "list_routes", "send", "get_status",
                         "detect_hardware", "backup_presets", "restore_presets"] {
            assert!(names.contains(&expected), "tool {expected} missing");
        }
    }

    #[test]
    fn every_tool_declares_schema_and_annotations() {
        let t = tools_list(&test_opts());
        for tool in t.get("tools").unwrap().as_array().unwrap() {
            let name = tool.get("name").and_then(Value::as_str).unwrap();
            assert!(tool.get("inputSchema").map(Value::is_object).unwrap_or(false),
                "tool {name} missing inputSchema");
            assert!(tool.get("outputSchema").map(Value::is_object).unwrap_or(false),
                "tool {name} missing outputSchema");
            let ann = tool.get("annotations").and_then(Value::as_object);
            assert!(ann.is_some(), "tool {name} missing annotations");
            assert!(ann.unwrap().contains_key("title"), "tool {name} annotations missing title");
        }
    }

    #[test]
    fn list_devices_result_carries_structured_content() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let res = list_devices(&idx, &DeviceFilter::default(), &|_| None).unwrap();
        let sc = res.get("structuredContent").expect("structuredContent present");
        assert!(sc.get("count").and_then(Value::as_u64).unwrap() > 100);
        assert!(sc.get("devices").and_then(Value::as_array).unwrap().len() > 100);
        // The text fallback block is still there for non-structured clients.
        assert!(res.get("content").and_then(Value::as_array).is_some());
    }

    #[test]
    fn list_routes_exposes_expects_on_surface_entries() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        // Find any device with a non-empty cc_params surface and check that
        // every entry carries an `expects` string (defaulted to "continuous").
        let mut checked = false;
        for e in &idx.entries {
            let res = match list_routes(&idx, &e.slug) { Ok(r) => r, Err(_) => continue };
            let sc = res.get("structuredContent").unwrap();
            let cc = sc.get("cc_params").and_then(Value::as_array).unwrap();
            if cc.is_empty() { continue; }
            for entry in cc {
                let nature = entry.get("expects").and_then(Value::as_str);
                assert!(nature.is_some(), "cc_param on {} missing expects", e.slug);
                assert!(!nature.unwrap().is_empty());
            }
            checked = true;
            break;
        }
        assert!(checked, "expected at least one device with cc_params");
    }

    #[test]
    fn shared_slug_resolves_to_the_most_trustworthy_variant() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let mut by_slug: HashMap<&str, Vec<&DeviceIndexEntry>> = HashMap::new();
        for e in &idx.entries { by_slug.entry(e.slug.as_str()).or_default().push(e); }
        let mut checked = 0;
        for (slug, variants) in by_slug.iter().filter(|(s, v)| !s.is_empty() && v.len() > 1) {
            let best = variants.iter().map(|e| tier_rank(&e.tier)).min().unwrap();
            let chosen = idx.find_path(slug).unwrap();
            let chosen_tier = &variants.iter().find(|e| e.path == chosen).unwrap().tier;
            assert_eq!(tier_rank(chosen_tier), best, "{slug} resolved to a weaker variant");
            checked += 1;
        }
        assert!(checked > 0, "catalogue should contain shared slugs");
    }

    #[test]
    fn tool_failure_is_a_result_and_unknown_tool_a_protocol_error() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let opts = test_opts();
        let call = |name: &str| handle_request(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": {"slug": "no-such-device"}}}), &opts, &idx, Caller::default()).unwrap();
        let failed = call("list_routes");
        assert_eq!(failed["result"]["isError"], json!(true));
        assert!(failed["result"]["content"][0]["text"].as_str().unwrap().contains("no-such-device"));
        let unknown = call("no_such_tool");
        assert_eq!(unknown["error"]["code"], json!(-32602));
        let listed: Vec<String> = tools_list(&test_opts())["tools"].as_array().unwrap().iter()
            .map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(listed.len(), tool_names(&test_opts()).len());
        assert!(tool_names(&test_opts()).iter().all(|n| listed.iter().any(|l| l == n)));
    }

    #[test]
    fn restore_refuses_without_explicit_confirmation() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let opts = test_opts();
        let err = call_tool("restore_presets", &json!({"out_port": "0", "file": "x.syx"}), &opts, &idx, Caller::default()).unwrap_err();
        assert!(err.contains("confirm_overwrite"), "{err}");
    }

    #[test]
    fn real_port_names_resolve_to_their_catalogue_driver() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let cat = hardware_candidates(&idx);
        for (port, want) in [("Minilab3 MIDI", "minilab3"), ("TR-8S", "tr_8s"), ("Digitakt II", "digitakt_ii")] {
            let best = crate::detect::rank(port, None, &cat, 1);
            assert!(!best.is_empty(), "{port}: no candidate");
            assert_eq!(cat[best[0].1].slug, want, "{port}");
        }
    }

    #[test]
    fn init_advertises_tools_capability() {
        let r = init_result();
        assert!(r.get("capabilities").and_then(|c| c.get("tools")).is_some());
        assert_eq!(r.get("protocolVersion").and_then(Value::as_str), Some(PROTOCOL_VERSION));
    }

    #[test]
    fn handle_initialize() {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let opts = test_opts();
        let resp = handle_request(&req, &opts, &empty_index(), Caller::default()).unwrap();
        assert_eq!(resp.get("id"), Some(&json!(1)));
        assert!(resp.get("result").is_some());
    }

    #[test]
    fn handle_initialized_notification_returns_nothing() {
        let req = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let opts = test_opts();
        assert!(handle_request(&req, &opts, &empty_index(), Caller::default()).is_none());
    }

    #[test]
    fn handle_unknown_method_returns_error() {
        let req = json!({"jsonrpc":"2.0","id":99,"method":"foo/bar","params":{}});
        let opts = test_opts();
        let resp = handle_request(&req, &opts, &empty_index(), Caller::default()).unwrap();
        assert!(resp.get("error").is_some());
    }
}
