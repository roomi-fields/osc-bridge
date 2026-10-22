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
//!
//! Two transports. On stdio (`run`) the server reads one JSON-RPC request per
//! line from stdin and writes one response per line to stdout; logging goes
//! to stderr. Over HTTP (`run_http`, MCP "streamable HTTP" without the
//! optional event stream) each POST carries one request and gets its
//! response as JSON; only the catalogue tools are offered there, since the
//! others act on the machine hosting the server, not the caller's.

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
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
    /// Offer only the read-only catalogue tools (`CATALOGUE_TOOLS`).
    pub catalogue_only: bool,
}

/// The tools that read the catalogue and nothing else — the whole surface
/// of a server reached over the network.
const CATALOGUE_TOOLS: [&str; 3] = ["list_devices", "get_device_docs", "list_routes"];

/// One lightweight catalogue entry. The full `Device` is loaded on demand
/// (per `get_device_docs` / `list_routes` call) — only this metadata is held
/// resident.
struct DeviceIndexEntry {
    path: PathBuf,
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
}

/// Lower is more trustworthy: tested on the device, then the vendor's own
/// documentation, then curated OSC surfaces, then community sources.
fn tier_rank(tier: &str) -> u8 {
    match tier {
        "hardware-verified" | "software-verified" => 0,
        "vendor-doc" | "vendor" => 1,
        "vendor-osc-api" | "third-party-osc" => 2,
        "electra-preset" => 3,
        "pencilresearch" => 4,
        _ => 5,
    }
}

/// Build the catalogue index once — every device tool reads from it
/// instead of re-walking the tree.
fn build_index(opts: &McpOptions) -> DeviceIndex {
    let devices_dir = crate::catalogue::resolve(&opts.devices_dir);
    eprintln!("MCP server starting (devices_dir={}, default_target={})",
        devices_dir.display(), opts.default_target);
    let index = DeviceIndex::build(&devices_dir);
    eprintln!("indexed {} device file(s)", index.entries.len());
    index
}

pub fn run(opts: McpOptions) -> Result<()> {
    let index = build_index(&opts);

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
        if let Some(resp) = handle_request(&req, &opts, &index) {
            let s = serde_json::to_string(&resp)?;
            writeln!(out, "{s}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Largest request body accepted over HTTP. A tool call is a few hundred
/// bytes; this only stops a client from making the server buffer garbage.
const MAX_HTTP_BODY: usize = 64 * 1024;

/// Worker threads answering HTTP requests. Calls are short reads of the
/// in-memory index and of one driver file.
const HTTP_WORKERS: usize = 4;

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

/// Who is asking: the TCP peer, or behind a reverse proxy the first address
/// of `X-Forwarded-For` (the peer is then the proxy itself).
fn client_key(req: &tiny_http::Request, behind_proxy: bool) -> String {
    let forwarded = behind_proxy.then(|| req.headers().iter()
        .find(|h| h.field.equiv("X-Forwarded-For"))
        .and_then(|h| h.value.as_str().split(',').next().map(|v| v.trim().to_string())))
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
/// overwrites that header is the sole way in.
pub fn run_http(opts: McpOptions, bind: &str, behind_proxy: bool) -> Result<()> {
    let opts = McpOptions { catalogue_only: true, ..opts };
    let index = build_index(&opts);
    let server = tiny_http::Server::http(bind).map_err(|e| anyhow::anyhow!("bind HTTP {bind}: {e}"))?;
    eprintln!("MCP over HTTP on http://{}/mcp (catalogue tools only)", server.server_addr());
    let shared = std::sync::Arc::new((server, opts, index, RateLimiter::default()));
    let workers: Vec<_> = (0..HTTP_WORKERS).map(|_| {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let (server, opts, index, limiter) = &*shared;
            for mut req in server.incoming_requests() {
                if !limiter.allow(&client_key(&req, behind_proxy)) {
                    respond(req, HttpReply::new(429, Some("rate limit: 120 requests per minute\n".into())));
                    continue;
                }
                let reply = http_exchange(&read_request(&mut req), opts, index);
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
    /// The anonymous user id the client presents: from a personal URL
    /// (`/u/<id>/mcp`) or the `Mcp-Session-Id` header (see `user_id`).
    user: Option<String>,
}

/// What the server sends back. `session` is the id to return in the
/// `Mcp-Session-Id` header, set when the server hands one out.
struct HttpReply {
    status: u16,
    body: Option<String>,
    session: Option<String>,
}

impl HttpReply {
    fn new(status: u16, body: Option<String>) -> Self {
        Self { status, body, session: None }
    }
}

/// A fresh anonymous user id: 128 random bits, hex.
fn new_user_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn is_user_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Anonymous users, no account. Every client that initializes gets an id
/// back in `Mcp-Session-Id`, which MCP clients repeat on each request of the
/// session. To stay the same user across sessions, a client uses a personal
/// endpoint `/u/<id>/mcp` (a fresh one from `GET /u/new`); that id then
/// serves as the session id too. Returns the path without the id prefix,
/// and the id when one was presented and well formed.
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
    HttpRequest { method, path, body, user }
}

/// The reply to one HTTP request.
fn http_exchange(req: &HttpRequest, opts: &McpOptions, index: &DeviceIndex) -> HttpReply {
    let rpc_error = |code: i64, msg: String| json!({
        "jsonrpc": "2.0", "id": null, "error": { "code": code, "message": msg },
    }).to_string();
    match (req.method.as_str(), req.path.as_str()) {
        ("OPTIONS", _) => HttpReply::new(204, None),
        ("GET", "/") => HttpReply::new(200, Some(format!(
            "osc-bridge {SERVER_VERSION} MCP server (catalogue tools: {}). POST JSON-RPC to /mcp; \
             a personal endpoint that keeps your anonymous id: GET /u/new. \
             Docs: https://osc-bridge.roomi-fields.com/api/\n", CATALOGUE_TOOLS.join(", ")))),
        ("GET", "/healthz") => HttpReply::new(200, Some("ok\n".into())),
        ("GET", "/u/new") => {
            let id = new_user_id();
            HttpReply::new(200, Some(json!({ "user_id": id, "mcp_path": format!("/u/{id}/mcp") }).to_string()))
        }
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
            let from_args = msg.pointer("/params/arguments/user_id").and_then(Value::as_str)
                .filter(|id| is_user_id(id)).map(str::to_ascii_lowercase);
            let user = match (req.user.clone().or(from_args), method) {
                (Some(id), _) => Some(id),
                (None, "initialize") => Some(new_user_id()),
                (None, _) => None,
            };
            if method == "tools/call" {
                log_usage(user.as_deref(), &msg);
            }
            let session = if method == "initialize" { user } else { None };
            match handle_request(&msg, opts, index) {
                Some(mut resp) => {
                    if let (Some(id), Some(result)) = (&session, resp.get_mut("result")) {
                        result["instructions"] = json!(keep_id_instructions(id));
                    }
                    HttpReply { status: 200, body: Some(resp.to_string()), session }
                }
                None => HttpReply { status: 202, body: None, session },
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
        list_routes with the slug. Your anonymous id is {id} (no account, no personal data). \
        To stay the same user in later sessions, connect to the personal endpoint /u/{id}/mcp \
        on this server, or pass user_id: \"{id}\" in tool calls; keep it if you can store it.")
}

/// One JSON line per tool call on stderr: when, which anonymous user, which
/// tool and the catalogue arguments. No address, no other request data.
fn log_usage(user: Option<&str>, msg: &Value) {
    let params = msg.get("params");
    let args = params.and_then(|p| p.get("arguments"));
    let pick = |k: &str| args.and_then(|a| a.get(k)).cloned().unwrap_or(Value::Null);
    eprintln!("{}", json!({
        "event": "tool_call",
        "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "user": user,
        "tool": params.and_then(|p| p.get("name")),
        "query": pick("query"), "vendor": pick("vendor"), "kind": pick("kind"), "slug": pick("slug"),
    }));
}

fn respond(req: tiny_http::Request, reply: HttpReply) {
    use tiny_http::{Header, Response};
    let HttpReply { status, body, session } = reply;
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

fn handle_request(req: &Value, opts: &McpOptions, index: &DeviceIndex) -> Option<Value> {
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
            Some((name, args)) => Ok(call_tool(name, &args, opts, index).unwrap_or_else(|e| json!({
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
        if opts.catalogue_only {
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
            }
        ]
    })
}

fn tool_names(opts: &McpOptions) -> Vec<&'static str> {
    if opts.catalogue_only {
        return CATALOGUE_TOOLS.to_vec();
    }
    vec!["list_devices", "get_device_docs", "list_routes", "send", "get_status",
         "detect_hardware", "backup_presets", "restore_presets"]
}

fn call_tool(name: &str, args: &Value, opts: &McpOptions, index: &DeviceIndex) -> Result<Value, String> {
    match name {
        "list_devices" => list_devices(index, &DeviceFilter::from_args(args)),
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

fn list_devices(index: &DeviceIndex, filter: &DeviceFilter) -> Result<Value, String> {
    let matching: Vec<&DeviceIndexEntry> = index.entries.iter().filter(|e| filter.keeps(e)).collect();
    let count = matching.len();
    let devices: Vec<Value> = matching.into_iter().take(filter.limit.unwrap_or(usize::MAX)).map(|e| json!({
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

    fn test_opts() -> McpOptions {
        McpOptions {
            devices_dir: PathBuf::from("devices"),
            default_target: "127.0.0.1:7777".parse().unwrap(),
            catalogue_only: false,
        }
    }

    #[test]
    fn list_devices_filters_by_words_vendor_kind_and_limit() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let run = |args: Value| list_devices(&idx, &DeviceFilter::from_args(&args)).unwrap()["structuredContent"].clone();
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
    fn catalogue_only_server_offers_and_runs_only_catalogue_tools() {
        let opts = McpOptions { catalogue_only: true, ..test_opts() };
        let listed: Vec<String> = tools_list(&opts)["tools"].as_array().unwrap().iter()
            .map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(listed, CATALOGUE_TOOLS);
        let resp = handle_request(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "send", "arguments": {"addr": "/x"}}}), &opts, &empty_index()).unwrap();
        assert_eq!(resp["error"]["code"], json!(-32602));
    }

    #[test]
    fn http_exchange_answers_posts_notifications_and_rejects_the_rest() {
        let opts = McpOptions { catalogue_only: true, ..test_opts() };
        let idx = empty_index();
        let req = |method: &str, path: &str, body: &str| HttpRequest {
            method: method.into(), path: path.into(), body: Ok(body.into()), user: None,
        };
        let r = http_exchange(&req("POST", "/mcp", r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#), &opts, &idx);
        assert_eq!(r.status, 200);
        let v: Value = serde_json::from_str(&r.body.unwrap()).unwrap();
        assert_eq!(v["id"], json!(7));
        assert_eq!(v["result"]["tools"].as_array().unwrap().len(), CATALOGUE_TOOLS.len());
        let r = http_exchange(&req("POST", "/mcp", r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), &opts, &idx);
        assert_eq!((r.status, r.body), (202, None));
        let status = |m: &str, p: &str, b: &str| http_exchange(&req(m, p, b), &opts, &idx).status;
        assert_eq!(status("POST", "/mcp", "not json"), 400);
        assert_eq!(status("GET", "/mcp", ""), 405);
        assert_eq!(status("GET", "/", ""), 200);
        assert_eq!(status("GET", "/healthz", ""), 200);
        assert_eq!(status("POST", "/other", ""), 404);
    }

    #[test]
    fn anonymous_users_get_an_id_and_keep_it_through_a_personal_path() {
        let opts = McpOptions { catalogue_only: true, ..test_opts() };
        let idx = empty_index();
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let post = |path: &str, header: Option<&str>| {
            let (path, user) = user_id(path, header);
            http_exchange(&HttpRequest { method: "POST".into(), path, body: Ok(init.into()), user }, &opts, &idx)
        };
        let fresh = post("/mcp", None).session.expect("initialize hands out an id");
        assert!(is_user_id(&fresh));
        assert_ne!(post("/mcp", None).session.unwrap(), fresh, "ids are random");
        let mine = "0123456789abcdef0123456789ABCDEF";
        assert_eq!(post(&format!("/u/{mine}/mcp"), None).session.unwrap(), mine.to_lowercase());
        assert_eq!(post("/mcp", Some(&fresh)).session.unwrap(), fresh);
        let body: Value = serde_json::from_str(&post("/mcp", None).body.unwrap()).unwrap();
        assert!(body["result"]["instructions"].as_str().unwrap().contains("/u/"));
        let with_arg = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"arguments":{{"user_id":"{fresh}"}}}}}}"#);
        let r = http_exchange(&HttpRequest { method: "POST".into(), path: "/mcp".into(), body: Ok(with_arg), user: None }, &opts, &idx);
        assert_eq!(r.session.unwrap(), fresh);
        let schema = &tools_list(&opts)["tools"][0]["inputSchema"]["properties"]["user_id"];
        assert_eq!(schema["type"], "string");
        assert_eq!(user_id("/u/not-an-id/mcp", None), ("/u/not-an-id/mcp".to_string(), None));
        let new = http_exchange(&HttpRequest { method: "GET".into(), path: "/u/new".into(), body: Ok(String::new()), user: None }, &opts, &idx);
        let v: Value = serde_json::from_str(&new.body.unwrap()).unwrap();
        assert!(is_user_id(v["user_id"].as_str().unwrap()));
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
        let res = list_devices(&idx, &DeviceFilter::default()).unwrap();
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
            "params": {"name": name, "arguments": {"slug": "no-such-device"}}}), &opts, &idx).unwrap();
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
        let err = call_tool("restore_presets", &json!({"out_port": "0", "file": "x.syx"}), &opts, &idx).unwrap_err();
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
        let resp = handle_request(&req, &opts, &empty_index()).unwrap();
        assert_eq!(resp.get("id"), Some(&json!(1)));
        assert!(resp.get("result").is_some());
    }

    #[test]
    fn handle_initialized_notification_returns_nothing() {
        let req = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let opts = test_opts();
        assert!(handle_request(&req, &opts, &empty_index()).is_none());
    }

    #[test]
    fn handle_unknown_method_returns_error() {
        let req = json!({"jsonrpc":"2.0","id":99,"method":"foo/bar","params":{}});
        let opts = test_opts();
        let resp = handle_request(&req, &opts, &empty_index()).unwrap();
        assert!(resp.get("error").is_some());
    }
}
