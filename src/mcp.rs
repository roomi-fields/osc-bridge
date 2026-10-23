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
//!  - `report_result`, `report_mapping`, `get_trust`, `submit_driver`,
//!    `submit_fix`, `get_contribution` — run on the hosted osc-bridge server:
//!    a local copy is linked to it unless offline (`crate::online`), mirrors
//!    its catalogue, shows each driver's trust, and forwards these there.
//!
//! The server reads one JSON-RPC request per line from stdin and writes one
//! response per line to stdout; logging goes to stderr.
//!
//! The hosted server is a separate program built on this module: it serves
//! the catalogue tools (`catalogue_tool`), the schemas of `tool_schemas` and
//! the same JSON-RPC framing (`rpc_reply`) over HTTP.

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::device::Device;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "osc-bridge";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct McpOptions {
    pub devices_dir: PathBuf,
    pub default_target: SocketAddr,
    /// A local copy linked to the hosted server (`crate::online`): mirror its
    /// catalogue (when `devices_dir` is the default one) and forward the
    /// report and contribution tools to it.
    pub online: bool,
    /// Devices the user keeps private on this machine (ids, slugs or
    /// `vendor/*`, see `online::Settings`): nothing about them is sent.
    pub private: Vec<String>,
}

/// Tools a private device keeps on the machine: refused before anything is
/// sent. `get_trust` on a private device is answered from the trust fetched
/// at startup instead.
const PRIVATE_REFUSED: [&str; 4] = ["report_result", "report_mapping", "submit_fix", "submit_driver"];

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

/// One lightweight catalogue entry. The full `Device` is loaded on demand
/// (per `get_device_docs` / `list_routes` call) — only this metadata is held
/// resident.
pub struct DeviceIndexEntry {
    pub path: PathBuf,
    /// Path under the catalogue root without `.json` (`korg/minilogue`),
    /// the same id as the site's API.
    pub id: String,
    pub slug: String,
    pub name: String,
    pub vendor: String,
    pub kind: String,
    pub revision: String,
    pub tier: String,
    pub firmware: Option<String>,
    pub manufacturer_id: Vec<u8>,
}

/// Built once at startup. Without it, every device tool call would re-walk
/// and re-parse the whole `devices/` tree (~900 files); with it, `list_devices`
/// is a cheap serialize and slug lookup is O(1).
pub struct DeviceIndex {
    pub entries: Vec<DeviceIndexEntry>,
    by_slug: HashMap<String, usize>,
}

impl DeviceIndex {
    /// Walk `devices/` once, parsing each JSON just enough to capture the
    /// catalogue metadata. When several files share an `osc_prefix` (the
    /// per-variant convention), the most trustworthy source wins the slug
    /// lookup (`tier_rank`), ties broken by path so the choice is stable.
    pub fn build(dir: &Path) -> Self {
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

    pub fn find_path(&self, slug: &str) -> Option<&Path> {
        self.by_slug.get(slug).map(|&i| self.entries[i].path.as_path())
    }

    /// A driver by its id (`korg/minilogue`) or, failing that, its slug.
    pub fn find(&self, id_or_slug: &str) -> Option<&DeviceIndexEntry> {
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
        if let Some(mut resp) = handle_request(&req, &opts, &current, link.as_deref()) {
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

fn handle_request(req: &Value, opts: &McpOptions, index: &DeviceIndex, link: Option<&Link>) -> Option<Value> {
    rpc_reply(req, &tool_names(opts), || tools_list(opts), |name, args| call_tool(name, args, opts, index, link))
}

/// The reply to one JSON-RPC message of an MCP session — `initialize`,
/// `tools/list` (from `tools`), `tools/call` of one of `offered` (run by
/// `call`), `ping` — or None for a notification.
pub fn rpc_reply(req: &Value, offered: &[&str], tools: impl FnOnce() -> Value,
                 call: impl FnOnce(&str, &Value) -> Result<Value, String>) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");

    // Notifications carry no id and expect no response.
    if id.is_none() && method.starts_with("notifications/") {
        return None;
    }

    let result: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(init_result()),
        "tools/list" => Ok(tools()),
        "tools/call" => match req.get("params").and_then(|p| Some((p.get("name")?.as_str()?, p.get("arguments").cloned().unwrap_or(json!({})))))
        {
            // An unknown tool is a protocol error; a tool that fails reports
            // it in its result, so the model can read it and react (MCP spec).
            Some((name, _)) if !offered.contains(&name) => Err((-32602, format!("unknown tool: {name}"))),
            Some((name, args)) => Ok(call(name, &args).unwrap_or_else(|e| json!({
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

pub fn init_result() -> Value {
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
    let mut all = tool_schemas();
    if let Some(tools) = all["tools"].as_array_mut() {
        let offered = tool_names(opts);
        tools.retain(|t| t["name"].as_str().is_some_and(|n| offered.contains(&n)));
    }
    all
}

/// The schema of every tool osc-bridge defines: the local ones, and those a
/// linked copy forwards to the hosted server (`online::FORWARDED_TOOLS`),
/// which serves them too.
pub fn tool_schemas() -> Value {
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
                "description": "Contribute a driver for a device the catalogue lacks (or a better one): the full driver file as a JSON object, in the osc-bridge device schema (device.name, device.vendor, device.osc_prefix, and cc_params / params / commands / triggers — see https://synthhub.roomi-fields.com/DEVICE_JSON_SCHEMA/). It is checked, then merged into the public catalogue by the merge job if it parses, lints and passes the tests, as its own file of source 'agent-contributed'. Say where the data comes from (manual page, tested on the device).",
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
            }

        ]
    })
}

fn tool_names(opts: &McpOptions) -> Vec<&'static str> {
    let local = ["list_devices", "get_device_docs", "list_routes", "send", "get_status",
         "detect_hardware", "backup_presets", "restore_presets"];
    let forwarded = if opts.online { crate::online::FORWARDED_TOOLS.as_slice() } else { &[] };
    local.iter().chain(forwarded).copied().collect()
}

fn call_tool(name: &str, args: &Value, opts: &McpOptions, index: &DeviceIndex, link: Option<&Link>) -> Result<Value, String> {
    if let Some(link) = link {
        if crate::online::FORWARDED_TOOLS.contains(&name) {
            if let Some(id) = private_device(index, opts, args) {
                if name == "get_trust" {
                    let trust = link.trust.read().map(|t| t["devices"][&id].clone()).unwrap_or(Value::Null);
                    let structured = json!({ "device": id, "private": true,
                        "overall": if trust.is_null() { json!({ "status": "unreported" }) } else { trust } });
                    return Ok(content_structured(&format!("{id} is private on this machine: trust as fetched when osc-bridge \
                        started, nothing asked of the server.\n{}", serde_json::to_string_pretty(&structured).unwrap_or_default()), structured));
                }
                if PRIVATE_REFUSED.contains(&name) {
                    return Err(format!("{id} is private on this machine: {name} is not sent to the osc-bridge server. \
                        To share it, remove it from the private list (osc-bridge privacy shows the settings file)."));
                }
            }
            return link.forward(name, args);
        }
    }
    if let Some(done) = catalogue_tool(name, args, index, &|id| link.and_then(|l| l.status(id))) {
        return done;
    }
    match name {
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

/// The catalogue tools — `list_devices`, `get_device_docs`, `list_routes` —
/// shared with the hosted server; None for any other tool. `status` gives a
/// driver's trust for `list_devices`, when known.
pub fn catalogue_tool(name: &str, args: &Value, index: &DeviceIndex, status: &dyn Fn(&str) -> Option<String>)
                      -> Option<Result<Value, String>> {
    let slug = || args.get("slug").and_then(Value::as_str).ok_or_else(|| "missing slug".to_string());
    Some(match name {
        "list_devices" => list_devices(index, &DeviceFilter::from_args(args), status),
        "get_device_docs" => slug().and_then(|s| get_device_docs(index, s)),
        "list_routes" => slug().and_then(|s| list_routes(index, s)),
        _ => return None,
    })
}

/// The driver a tool call is about, when the user keeps it private: the
/// `device` argument, or the device of a submitted driver. A private id also
/// covers the other variants of the same device (same slug).
fn private_device(index: &DeviceIndex, opts: &McpOptions, args: &Value) -> Option<String> {
    let private = &opts.private;
    let covers = |id: &str, slug: &str| crate::online::Settings::covers(private, id, slug)
        || private.iter().any(|p| index.find(p).is_some_and(|e| e.slug == slug));
    if let Some(wanted) = args.get("device").and_then(Value::as_str) {
        let entry = index.find(wanted)?;
        return covers(&entry.id, &entry.slug).then(|| entry.id.clone());
    }
    let dev = args.get("driver")?.get("device")?;
    let (vendor, name) = (dev.get("vendor")?.as_str()?, dev.get("name")?.as_str()?);
    let slug = dev.get("osc_prefix")?.as_str()?.trim_start_matches('/');
    let id = format!("{}/{}", vendor_dir(index, vendor), slugify(name, '_'));
    covers(&id, slug).then_some(id)
}

/// `osc-bridge privacy`: change the settings at `path` — `offline`, devices
/// made private or public again (ids or slugs of `devices_dir`) — then show
/// them. Returns what to print.
pub fn privacy(path: &Path, devices_dir: &Path, offline: Option<bool>, private: &[String], public: &[String]) -> Result<String> {
    let mut settings = crate::online::Settings::load(Some(path))?;
    let index = DeviceIndex::build(devices_dir);
    let id_of = |name: &str| match name.strip_suffix("/*") {
        Some(vendor) => index.entries.iter().any(|e| e.id.starts_with(&format!("{vendor}/"))).then(|| name.to_string())
            .ok_or_else(|| anyhow::anyhow!("no vendor '{vendor}' in the catalogue")),
        None => index.find(name).map(|e| e.id.clone())
            .ok_or_else(|| anyhow::anyhow!("no device '{name}' in the catalogue (use an id or slug from list_devices, or vendor/*)")),
    };
    let changed = offline.is_some() || !private.is_empty() || !public.is_empty();
    if let Some(o) = offline { settings.offline = o; }
    for name in private {
        let id = id_of(name)?;
        if !settings.private.contains(&id) { settings.private.push(id); }
    }
    for name in public {
        let id = id_of(name).unwrap_or_else(|_| name.clone());
        settings.private.retain(|p| *p != id && p != name);
    }
    if changed { settings.save(path)?; }
    let mut out = if settings.offline {
        "Offline: this copy does not contact the osc-bridge server (osc-bridge privacy --offline off to link it again).\n".to_string()
    } else {
        "Linked to the osc-bridge server: the catalogue stays up to date, and reports and fixes are shared \
         under an anonymous account (osc-bridge privacy --offline on to stop).\n".to_string()
    };
    if settings.private.is_empty() {
        out.push_str("Private devices: none.\n");
    } else {
        out.push_str("Private devices (nothing about them is sent to the server):\n");
        for id in &settings.private {
            let name = index.find(id).map(|e| if e.name.to_lowercase().starts_with(&e.vendor.to_lowercase()) {
                e.name.clone()
            } else {
                format!("{} {}", e.vendor, e.name)
            }).unwrap_or_else(|| if id.ends_with("/*") { "every device of this vendor".into() } else { String::new() });
            out.push_str(&format!("  {id}  {name}\n"));
        }
    }
    out.push_str(&format!("Settings: {}\n", path.display()));
    Ok(out)
}

/// The hardware drivers of the catalogue, in the shape `detect::rank` scores.
pub fn hardware_candidates(index: &DeviceIndex) -> Vec<crate::detect::Candidate> {
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
pub fn surface_addresses(dev: &Device) -> Vec<String> {
    let prefix = &dev.device.osc_prefix;
    let cc = dev.cc_params.iter().flat_map(|t| t.entries.iter().map(|e| e.osc.clone()));
    let params = dev.params.iter().flat_map(|t| t.entries.iter().map(|e| e.osc.clone()));
    let triggers = dev.triggers.iter().flat_map(|t| t.entries.iter().map(|e| e.osc.clone()));
    dev.commands.iter().map(|c| c.osc.clone()).chain(cc).chain(params).chain(triggers)
        .map(|a| format!("{prefix}{a}")).collect()
}

/// `Moog Music` → `moog_music`: lower case, other characters as `sep`.
pub fn slugify(s: &str, sep: char) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() { out.push(c.to_ascii_lowercase()); }
        else if !out.ends_with(sep) && !out.is_empty() { out.push(sep); }
    }
    out.trim_end_matches(sep).to_string()
}

/// The catalogue folder of a vendor: the one its drivers already use, else
/// a new one from its name.
pub fn vendor_dir(index: &DeviceIndex, vendor: &str) -> String {
    index.entries.iter().find(|e| e.vendor.eq_ignore_ascii_case(vendor))
        .and_then(|e| e.id.split('/').next().map(str::to_string))
        .unwrap_or_else(|| slugify(vendor, '-'))
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

pub fn json_to_osc(v: &Value) -> rosc::OscType {
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
pub fn content_structured(text: &str, structured: Value) -> Value {
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
    use std::collections::BTreeMap;

    /// An empty index — these handshake/dispatch tests don't touch devices.
    fn empty_index() -> DeviceIndex {
        DeviceIndex { entries: Vec::new(), by_slug: HashMap::new() }
    }

    fn test_opts() -> McpOptions {
        McpOptions {
            devices_dir: PathBuf::from("devices"),
            default_target: "127.0.0.1:7777".parse().unwrap(),
            online: false,
            private: Vec::new(),
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
    fn a_private_device_keeps_its_reports_on_the_machine() {
        let index = DeviceIndex::build(&PathBuf::from("devices"));
        let opts = McpOptions { online: true, private: vec!["korg/minilogue".into()], ..test_opts() };
        // Never reached: the refusal comes first.
        let link = Link::new("http://127.0.0.1:9");
        let caller = Some(&link);
        for (tool, args) in [("report_result", json!({"device": "korg/minilogue", "outcome": "works"})),
                             ("report_mapping", json!({"device": "minilogue", "address": "/filter/cutoff", "cc": 43})),
                             ("submit_fix", json!({"device": "korg/minilogue", "content": "{}"}))] {
            let err = call_tool(tool, &args, &opts, &index, caller).unwrap_err();
            assert!(err.contains("private on this machine") && err.contains("private list"), "{tool}: {err}");
        }
        let driver = json!({"driver": {"device": {"name": "minilogue", "vendor": "KORG", "osc_prefix": "/minilogue"}}});
        assert!(call_tool("submit_driver", &driver, &opts, &index, caller).unwrap_err().contains("private"));
        let trust = call_tool("get_trust", &json!({"device": "korg/minilogue"}), &opts, &index, caller).unwrap();
        assert_eq!(trust["structuredContent"]["private"], json!(true), "answered on the machine, the server is never asked");
        let vendor = McpOptions { private: vec!["korg/*".into()], ..opts };
        assert!(call_tool("report_result", &json!({"device": "korg/ms2000", "outcome": "works"}), &vendor, &index, caller)
            .unwrap_err().contains("private"));
        let other = call_tool("report_result", &json!({"device": "arturia/minilab3", "outcome": "works"}), &vendor, &index, caller).unwrap_err();
        assert!(other.contains("could not be reached"), "another device is shared: {other}");
    }

    #[test]
    fn the_privacy_command_records_choices_by_catalogue_id() {
        let path = std::env::temp_dir().join(format!("osc-bridge-privacy-{}/settings.toml", std::process::id()));
        let devices = Path::new("devices");
        let shown = privacy(&path, devices, None, &[], &[]).unwrap();
        assert!(shown.contains("Linked to the osc-bridge server") && shown.contains("Private devices: none"), "{shown}");
        assert!(!path.exists(), "showing writes nothing");
        let shown = privacy(&path, devices, Some(true), &["minilogue".into()], &[]).unwrap();
        assert!(shown.contains("Offline") && shown.contains("korg/minilogue"), "{shown}");
        assert_eq!(crate::online::Settings::load(Some(&path)).unwrap(),
            crate::online::Settings { offline: true, private: vec!["korg/minilogue".into()] });
        assert!(privacy(&path, devices, None, &["no-such-synth".into()], &[]).is_err());
        privacy(&path, devices, Some(false), &[], &["korg/minilogue".into()]).unwrap();
        assert_eq!(crate::online::Settings::load(Some(&path)).unwrap(), crate::online::Settings::default());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn an_offline_copy_does_not_offer_the_server_tools() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let resp = handle_request(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "report_result", "arguments": {"device": "korg/minilogue", "outcome": "works"}}}),
            &test_opts(), &idx, None).unwrap();
        assert_eq!(resp["error"]["code"], json!(-32602));
    }

    #[test]
    fn the_catalogue_tools_are_shared_with_the_hosted_server() {
        let idx = DeviceIndex::build(&PathBuf::from("devices"));
        let routes = catalogue_tool("list_routes", &json!({"slug": "minilogue"}), &idx, &|_| None).unwrap().unwrap();
        assert!(routes["structuredContent"].is_object());
        let listed = catalogue_tool("list_devices", &json!({"query": "minilogue", "limit": 1}), &idx, &|_| Some("unconfirmed".into()))
            .unwrap().unwrap();
        assert_eq!(listed["structuredContent"]["devices"][0]["usage_status"], "unconfirmed");
        assert!(catalogue_tool("send", &json!({}), &idx, &|_| None).is_none());
        let names: Vec<String> = tool_schemas()["tools"].as_array().unwrap().iter()
            .map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert!(crate::online::FORWARDED_TOOLS.iter().all(|f| names.iter().any(|n| n == f)), "{names:?}");
        assert!(!names.iter().any(|n| n.starts_with("relay_")), "relay tools belong to the hosted server");
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
            "params": {"name": name, "arguments": {"slug": "no-such-device"}}}), &opts, &idx, None).unwrap();
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
        let err = call_tool("restore_presets", &json!({"out_port": "0", "file": "x.syx"}), &opts, &idx, None).unwrap_err();
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
        let resp = handle_request(&req, &opts, &empty_index(), None).unwrap();
        assert_eq!(resp.get("id"), Some(&json!(1)));
        assert!(resp.get("result").is_some());
    }

    #[test]
    fn handle_initialized_notification_returns_nothing() {
        let req = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let opts = test_opts();
        assert!(handle_request(&req, &opts, &empty_index(), None).is_none());
    }

    #[test]
    fn handle_unknown_method_returns_error() {
        let req = json!({"jsonrpc":"2.0","id":99,"method":"foo/bar","params":{}});
        let opts = test_opts();
        let resp = handle_request(&req, &opts, &empty_index(), None).unwrap();
        assert!(resp.get("error").is_some());
    }

    /// A stand-in for the hosted server, on a free local port: it issues
    /// accounts (refusing ids it did not issue, like the real one), lists and
    /// serves the catalogue under `root`, gives a fixed trust list, and
    /// answers every forwarded tool call with the tool's name. The calls it
    /// received are in the returned list.
    fn fake_server(root: &Path) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (root, seen) = (root.to_path_buf(), calls.clone());
        std::thread::spawn(move || {
            let mut issued: Vec<String> = Vec::new();
            for mut req in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
                let url = req.url().to_string();
                let (status, reply) = match url.as_str() {
                    "/u/new" => {
                        let id = format!("{:032x}", issued.len() + 1);
                        issued.push(id.clone());
                        (200, json!({ "user_id": id }))
                    }
                    "/v1/trust" => (200, json!({ "devices": { "korg/minilogue": { "status": "unconfirmed" } } })),
                    "/v1/catalogue/files" => {
                        let paths: Value = serde_json::from_str(&body).unwrap();
                        let files: BTreeMap<String, String> = paths["paths"].as_array().unwrap().iter()
                            .map(|p| p.as_str().unwrap().to_string())
                            .map(|p| { let text = std::fs::read_to_string(root.join(&p)).unwrap(); (p, text) }).collect();
                        (200, json!({ "files": files }))
                    }
                    u if u.ends_with("/v1/catalogue") => {
                        let files: Vec<Value> = crate::online::catalogue_files(&root).into_iter()
                            .map(|(p, h)| json!({ "path": p, "sha256": h })).collect();
                        (200, json!({ "files": files }))
                    }
                    u if u.starts_with("/u/") && u.ends_with("/mcp") => {
                        let id = &u[3..u.len() - 4];
                        if issued.iter().any(|i| i == id) {
                            let rpc: Value = serde_json::from_str(&body).unwrap();
                            let name = rpc["params"]["name"].as_str().unwrap().to_string();
                            seen.lock().unwrap().push(name.clone());
                            (200, json!({ "jsonrpc": "2.0", "id": 1, "result": { "content": [{ "type": "text", "text": name }] } }))
                        } else {
                            (404, json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32001, "message": "unknown account" } }))
                        }
                    }
                    _ => (404, json!({})),
                };
                let _ = req.respond(tiny_http::Response::from_string(reply.to_string()).with_status_code(status));
            }
        });
        (base, calls)
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
        let (base, calls) = fake_server(&served);
        let link = Link { client: crate::online::Online::keeping_account_in(&base, Some(tmp.join("user"))), ..Link::new("unused") };
        let user = link.user(false).unwrap();
        assert_eq!(std::fs::read_to_string(tmp.join("user")).unwrap(), user, "the account is kept");
        let done = link.client.sync_catalogue(&user, &bundled, &cache).unwrap();
        assert_eq!(done, crate::online::Synced { files: 3, copied: 1, downloaded: 2, removed: 1 });
        assert_eq!(crate::online::catalogue_files(&cache), crate::online::catalogue_files(&served));
        assert!(!link.client.sync_catalogue(&user, &bundled, &cache).unwrap().changed(), "nothing to do the second time");

        let opts = McpOptions { online: true, ..test_opts() };
        let index = DeviceIndex::build(&cache);
        let reported = call_tool("report_result", &json!({"device": "korg/minilogue", "outcome": "works", "firmware": "2.10"}),
            &opts, &index, Some(&link)).unwrap();
        assert_eq!(reported["content"][0]["text"], "report_result", "{reported}");
        if let Ok(mut t) = link.trust.write() { *t = link.client.trust().unwrap(); }
        let listed = call_tool("list_devices", &json!({"query": "minilogue"}), &opts, &index, Some(&link)).unwrap();
        assert_eq!(listed["structuredContent"]["devices"][0]["usage_status"], "unconfirmed");
        assert!(tool_names(&opts).contains(&"submit_fix") && !tool_names(&test_opts()).contains(&"submit_fix"));

        std::fs::write(tmp.join("user"), "0".repeat(32)).unwrap();
        let relinked = Link { client: crate::online::Online::keeping_account_in(&base, Some(tmp.join("user"))), ..Link::new("unused") };
        let trust = call_tool("get_trust", &json!({"device": "korg/minilogue"}), &opts, &index, Some(&relinked)).unwrap();
        assert_eq!(trust["content"][0]["text"], "get_trust", "an account the server forgot is replaced: {trust}");
        assert_ne!(std::fs::read_to_string(tmp.join("user")).unwrap(), "0".repeat(32));
        assert_eq!(*calls.lock().unwrap(), ["report_result", "get_trust"]);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
