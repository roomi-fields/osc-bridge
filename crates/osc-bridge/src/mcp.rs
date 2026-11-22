//! MCP (Model Context Protocol) server — exposes the SynthHub catalogue and
//! OSC surface to LLMs via JSON-RPC 2.0 over stdio.
//!
//! Spec: <https://modelcontextprotocol.io/specification/2025-06-18>
//!
//! Tools exposed:
//!  - `list_devices`   — find drivers: in the SynthHub catalogue, or under
//!    `--devices-dir` (offline: the drivers cached on this machine)
//!  - `get_device_docs`— fetch the .md companion of one device
//!  - `list_routes`    — return the OSC routes (commands + cc/params) of one device
//!  - `export_device`  — one driver written as OSCQuery or MIDNAM (`crate::export`)
//!  - `send`           — emit an OSC message to a running osc-bridge bind port
//!  - `get_status`     — round-trip `/bridge/status` to a running bridge
//!  - `add_link`, `remove_link`, `list_links` — change or list the links of a
//!    running orchestrator, through its `/bridge/link/*` control addresses
//!
//!  - `detect_hardware`, `backup_presets`, `restore_presets` — the MIDI
//!    ports of the machine the server runs on
//!  - `report_result`, `report_mapping`, `get_trust`, `submit_driver`,
//!    `submit_fix`, `get_contribution` — run on the SynthHub server:
//!    a local copy is linked to it unless offline (`crate::online`), fetches
//!    the drivers it needs from it one at a time (`crate::catalogue`), shows
//!    each driver's trust, and forwards these there.
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

use osd::Device;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "osc-bridge";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct McpOptions {
    /// A folder of drivers the user keeps, used as it is. None: the drivers
    /// come from the SynthHub server, one at a time, kept in the cache
    /// (`online::cache_dir`), which is all there is offline.
    pub devices_dir: Option<PathBuf>,
    pub default_target: SocketAddr,
    /// A local copy linked to the hosted server (`crate::online`): fetch the
    /// drivers it needs (without `devices_dir`) and forward the report and
    /// contribution tools to it.
    pub online: bool,
    /// Devices the user keeps private on this machine (ids, slugs or
    /// `vendor/*`, see `online::Settings`): nothing about them is sent.
    pub private: Vec<String>,
}

/// Tools a private device keeps on the machine: refused before anything is
/// sent. `get_trust` on a private device is answered from the trust fetched
/// at startup instead.
const PRIVATE_REFUSED: [&str; 5] = ["report_result", "report_mapping", "submit_fix", "submit_driver", "share_preset"];

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
        let unreachable = |e: anyhow::Error| format!("{name} goes to the SynthHub server, which could not be reached: {e}");
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
    /// How the device is recognised (its own block, else the one of the
    /// file it shares its MIDI with).
    pub identity: Option<osd::Identity>,
    /// Slugs of the devices whose sounds this one loads as they are.
    pub loads_sounds_of: Vec<String>,
    /// The driver this one shares its MIDI with (`midi_of`), by id.
    pub midi_of: Option<String>,
}

/// Built once over a folder of drivers. Without it, every device tool call
/// would re-walk and re-parse the folder; with it, `list_devices` is a cheap
/// serialize and slug lookup is O(1).
pub struct DeviceIndex {
    pub entries: Vec<DeviceIndexEntry>,
    by_slug: HashMap<String, usize>,
}

impl DeviceIndex {
    /// Walk `dir` once, parsing each JSON just enough to capture the
    /// catalogue metadata. When several files share an `osc_prefix` (the
    /// per-variant convention), the most trustworthy source wins the slug
    /// lookup (`tier_rank`), ties broken by path so the choice is stable.
    pub fn build(dir: &Path) -> Self {
        let mut entries: Vec<DeviceIndexEntry> = Vec::new();
        let mut by_slug: HashMap<String, usize> = HashMap::new();
        let _ = walk_json(dir, &mut |path| {
            // A maker's profile is not a driver.
            if path.file_name().and_then(|f| f.to_str()).is_some_and(|f| f.starts_with('_')) { return; }
            // The file as written (what it says of itself), and the driver
            // as its firmware runs it: the MIDI it shares, and so the sounds
            // it loads, come from `osd`'s resolution.
            let Some(own) = crate::device::read_own(path) else { return };
            let shared = crate::device::load(path).ok();
            let body = shared.as_ref().unwrap_or(&own);
            let slug = own.device.osc_prefix.trim_start_matches('/').to_string();
            let source = own.sources.iter().flatten().next();
            let tier = source.map(|s| s.kind.clone()).unwrap_or_else(|| "unknown".into());
            let firmware = source.and_then(|s| s.firmware.clone()).filter(|f| f != "unknown");
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
                name: own.device.name.clone(),
                vendor: own.device.vendor.clone().unwrap_or_else(|| "?".into()),
                kind: crate::device::kind_name(own.device.kind).to_string(),
                revision: own.device.revision.clone().unwrap_or_default(),
                tier,
                firmware,
                manufacturer_id: body.device.manufacturer_id.clone().unwrap_or_default(),
                identity: body.identity.clone(),
                loads_sounds_of: body.loads_sounds_of.clone().unwrap_or_default(),
                midi_of: own.midi_of.clone(),
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
pub fn tier_rank(tier: &str) -> u8 {
    match tier {
        "hardware-verified" | "software-verified" => 0,
        "vendor-doc" | "vendor" => 1,
        "vendor-osc-api" | "third-party-osc" => 2,
        "electra-preset" => 3,
        "pencilresearch" | "knobkraft" => 4,
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
const LINKED_INSTRUCTIONS: &str = "osc-bridge with the SynthHub catalogue of MIDI implementations (CC, NRPN, \
    SysEx, drum maps) of synths, drum machines and DAWs: each driver is fetched from the SynthHub server when a \
    tool needs it, as other users and agents fixed and confirmed it. Start with list_devices (or detect_hardware), \
    then list_routes with the device; usage_status and get_trust tell how far other users confirmed a driver, per \
    firmware. After testing a driver on the real device, call report_result with the firmware and the device's \
    Identity Reply: it improves the driver for everyone.";

/// Fetch the trust of every reported driver from the hosted server, in the
/// background; the session waits for it when it ends, so nothing of it (the account it may
/// have delivered) is written after the session.
fn trust_in_background(link: std::sync::Arc<Link>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || match link.client.trust() {
        Ok(t) => if let Ok(mut g) = link.trust.write() { *g = t },
        Err(e) => eprintln!("SynthHub server unreachable ({e}): only the drivers cached on this machine"),
    })
}

/// The driver a tool call needs on this machine, by id or OSC prefix: the one it
/// reads, or the one it reports on (to tell whether it is private).
fn driver_wanted(req: &Value) -> Option<&str> {
    if req["method"] != "tools/call" { return None; }
    let (name, args) = (req["params"]["name"].as_str()?, &req["params"]["arguments"]);
    match name {
        "get_device_docs" | "list_routes" | "export_device" | "list_roles" | "set_by_role" | "get_by_role" => args["device"].as_str(),
        n if crate::online::FORWARDED_TOOLS.contains(&n) => args["device"].as_str(),
        _ => None,
    }
}

/// Every driver a tool call needs: `driver_wanted`, and for `install_export` both
/// its `device` and its `on`.
fn drivers_wanted(req: &Value) -> Vec<&str> {
    if req["method"] == "tools/call" && req["params"]["name"] == "install_export" {
        let args = &req["params"]["arguments"];
        return [args["device"].as_str(), args["on"].as_str()].into_iter().flatten().collect();
    }
    driver_wanted(req).into_iter().collect()
}

/// The MCP server over stdio, linked to `online::SERVER`, its drivers kept
/// in `online::cache_dir`.
pub fn run(opts: McpOptions) -> Result<()> {
    serve(opts, crate::online::SERVER, crate::online::cache_dir(), std::io::stdin().lock(), std::io::stdout().lock())
}

/// `serve_with_midi` on the machine's MIDI system (the guarded opening).
pub fn serve(opts: McpOptions, base: &str, cache: Option<PathBuf>, input: impl BufRead, out: impl Write) -> Result<()> {
    serve_with_midi(opts, base, cache, input, out, &crate::take::open_hardware)
}

/// One MCP session: a JSON-RPC message per line of `input`, each reply a
/// line of `out`. Linked (`opts.online`), it asks the server at `base` for
/// the drivers it needs and keeps them in `cache`, unless the user gave
/// `opts.devices_dir`. The tools that reach the MIDI system take it from `open`.
pub fn serve_with_midi(opts: McpOptions, base: &str, cache: Option<PathBuf>, input: impl BufRead, mut out: impl Write,
                       open: crate::take::Opener) -> Result<()> {
    use std::sync::{Arc, RwLock};
    let link = opts.online.then(|| Arc::new(Link::new(base)));
    // Drivers from the server are kept in the cache; a folder the user gave is theirs.
    let cache = if opts.devices_dir.is_some() { None } else { cache };
    let root = opts.devices_dir.clone().or_else(|| cache.clone()).unwrap_or_default();
    let index = RwLock::new(Arc::new(build_index(&root, &opts)));
    let trust = link.as_ref().map(|link| trust_in_background(link.clone()));
    let mut fetched = std::collections::HashSet::new();

    for line in input.lines() {
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
        // Once a session, the driver a call needs comes from the server; a
        // device the user keeps private by that very name is not asked for.
        if let (Some(link), Some(cache)) = (&link, &cache) {
            for name in drivers_wanted(&req) {
                if crate::online::Settings::covers(&opts.private, name, name) || !fetched.insert(name.to_string()) { continue; }
                match crate::catalogue::fetch(&link.client, name, cache) {
                    Ok(_) => if let Ok(mut g) = index.write() { *g = Arc::new(DeviceIndex::build(cache)) },
                    Err(e) => eprintln!("{name}: {e:#}"),
                }
            }
        }
        let current = index.read().map(|g| g.clone()).map_err(|_| anyhow::anyhow!("catalogue index poisoned"))?;
        if let Some(mut resp) = handle_request_with(&req, &opts, &current, link.as_deref(), open) {
            if let (true, Some(result)) = (link.is_some() && req["method"] == "initialize", resp.get_mut("result")) {
                result["instructions"] = json!(LINKED_INSTRUCTIONS);
            }
            // A newer osc-bridge the server announced is told once, in the next tool result.
            if let (true, Some(content)) = (req["method"] == "tools/call", resp.pointer_mut("/result/content").and_then(Value::as_array_mut)) {
                if let Some(notice) = crate::online::newer_notice_once() {
                    content.push(json!({ "type": "text", "text": notice }));
                }
            }
            let s = serde_json::to_string(&resp)?;
            writeln!(out, "{s}")?;
            out.flush()?;
        }
    }
    if let Some(trust) = trust { let _ = trust.join(); }
    Ok(())
}

#[cfg(test)]
fn handle_request(req: &Value, opts: &McpOptions, index: &DeviceIndex, link: Option<&Link>) -> Option<Value> {
    handle_request_with(req, opts, index, link, &crate::take::open_hardware)
}

fn handle_request_with(req: &Value, opts: &McpOptions, index: &DeviceIndex, link: Option<&Link>, open: crate::take::Opener) -> Option<Value> {
    rpc_reply(req, &tool_names(opts), || tools_list(opts), |name, args| call_tool_with(name, args, opts, index, link, open))
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
                "description": "Find device drivers in the catalogue of synths, drum machines, controllers and DAWs. Without arguments it lists them all; `query` keeps drivers whose vendor, name or OSC prefix contain every word (e.g. 'korg minilogue'). Returns device (the catalogue id), name, vendor, OSC prefix, kind, source tier and firmware version when the source states one. Pass device to list_routes for the full CC/NRPN/SysEx map.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Words that must all appear in the vendor, name or OSC prefix (case-insensitive)." },
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
                                    "device": { "type": "string", "description": "The device: its catalogue id, e.g. 'korg/minilogue'. Pass it as it is to list_routes and the other tools." },
                                    "usage_status": { "type": ["string", "null"], "description": "Hosted server only: trust from usage reports (see get_trust)." },
                                    "name": { "type": "string" },
                                    "vendor": { "type": "string" },
                                    "osc_prefix": { "type": "string", "description": "OSC prefix without the leading slash." },
                                    "kind": { "type": "string", "description": "'hardware' or 'software'." },
                                    "revision": { "type": "string" },
                                    "source_tier": { "type": "string" },
                                    "firmware": { "type": ["string", "null"], "description": "Firmware the source documents; null when it does not say." },
                                    "file": { "type": "string" }
                                },
                                "required": ["device", "name", "vendor", "osc_prefix", "kind"]
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
                "description": "Fetch the markdown companion documentation for one device, if present. The device is a catalogue id or an OSC prefix without the leading slash (e.g. 'ableton' for /ableton).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "The device: a catalogue id or OSC prefix, e.g. 'ableton' or 'minilab3'." }
                    },
                    "required": ["device"]
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "The device as the caller named it." },
                        "has_docs": { "type": "boolean", "description": "False when no companion .md exists for the device." },
                        "markdown": { "type": "string", "description": "The companion documentation, or a not-found message when has_docs is false." }
                    },
                    "required": ["device", "has_docs", "markdown"]
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
                        "device": { "type": "string", "description": "The device: a catalogue id or OSC prefix, e.g. 'minilab3'." }
                    },
                    "required": ["device"]
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
                "name": "export_device",
                "description": "Write one driver in a format other tools read, any of the formats of osd-formats: 'oscquery' gives its OSC namespace as OSCQuery JSON (every route with type tags, ranges and access, for OSC controllers and browsers); 'midnam' gives its MIDI names as a MIDI Name Document (controllers incl. NRPN/RPN, named values, drum notes per channel, for DAWs such as Ardour or Logic); 'deluge' gives the definition file of a Deluge; and the others in the enum. Each carries the credit and licence of the driver's sources. The document is the text content; an OSCQuery namespace is large (every address, each CC 0-127 included). It only reads: to put an export on a device, use install_export.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id (e.g. 'korg/minilogue') or OSC prefix, from list_devices." },
                        "format": { "type": "string", "enum": crate::export::format_names() }
                    },
                    "required": ["device", "format"],
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id." },
                        "format": { "type": "string" },
                        "media_type": { "type": "string", "description": "The media type of the format (e.g. 'application/json', 'application/xml')." },
                        "file_name": { "type": "string", "description": "A file name to save it under." },
                        "bytes": { "type": "integer", "description": "Size of the document, which is the text content." }
                    },
                    "required": ["device", "format", "media_type", "file_name", "bytes"]
                },
                "annotations": {
                    "title": "Export a driver",
                    "readOnlyHint": true,
                    "idempotentHint": true,
                    "openWorldHint": false
                }
            },
            {
                "name": "install_export",
                "description": "Put the export of one driver (device) on another device plugged into this computer (on), in one gesture the same for every target: the format the target receives, the path of the file and the way it is sent (a file written by SysEx, step by step) all come from the target's own driver, and a target that declares nothing it receives is refused. This may OVERWRITE a file on the device (it is not read first): without confirm_overwrite: true nothing is sent, and the call returns what it would write (format, way, path, size, what the format could not carry); confirm with the user, then call again with confirm_overwrite: true. A target that is not plugged in is refused before any message; a send that fails on the way says the step reached and the messages already sent, with no retry and no undo.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "The driver to export: id (e.g. 'korg/minilogue') or OSC prefix, from list_devices." },
                        "on": { "type": "string", "description": "The device that receives it: its catalogue id or OSC prefix; its driver says what it receives." },
                        "confirm_overwrite": { "type": "boolean", "description": "True once the user agreed that the file may be overwritten on the target; without it nothing is sent." }
                    },
                    "required": ["device", "on"],
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string" },
                        "on": { "type": "string" },
                        "format": { "type": "string", "description": "The format the target receives." },
                        "way": { "type": "string", "description": "How it is sent (sysex)." },
                        "path": { "type": "string", "description": "The path of the file on the target." },
                        "size": { "type": "integer", "description": "Bytes of the file." },
                        "lost": { "type": "array", "items": { "type": "object" }, "description": "What the format could not carry (place, reason)." },
                        "written": { "type": "boolean", "description": "False for a preview: nothing was sent." },
                        "messages": { "type": "integer", "description": "SysEx messages sent." }
                    },
                    "required": ["device", "on", "format", "way", "path", "size", "lost", "written", "messages"]
                },
                "annotations": {
                    "title": "Put an export on a device",
                    "readOnlyHint": false,
                    "destructiveHint": true,
                    "idempotentHint": true,
                    "openWorldHint": true
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
                        "reason": { "type": "string", "description": "Present when osc-bridge could not handle the message (sent is false): why." },
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
                "name": "list_roles",
                "description": "List the roles of a driver's controls (OSD paths such as filter/cutoff), most important first, each with its address, range, default and steps. A driver without roles gives an empty list.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id (see list_devices)." },
                        "limit": { "type": "integer", "description": "Keep only the first roles, up to this number." }
                    },
                    "required": ["device"]
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string" },
                        "roles": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "role": { "type": "string" },
                                    "address": { "type": "string" },
                                    "range": { "type": "array", "items": { "type": "integer" } },
                                    "default": { "type": "integer" },
                                    "step_count": { "type": "integer" },
                                    "priority": { "type": "integer", "description": "1 (essential) to 5 (hidden)." }
                                },
                                "required": ["role", "address", "range", "priority"]
                            }
                        }
                    },
                    "required": ["device", "roles"]
                },
                "annotations": {
                    "title": "List the roles of a driver",
                    "readOnlyHint": true,
                    "idempotentHint": true,
                    "openWorldHint": false
                }
            },
            {
                "name": "set_by_role",
                "description": "Set a control by its role (e.g. filter/cutoff) without knowing its address or numbers. value is from 0 to 1 on the range of the role; osc-bridge scales it on the range and steps of this driver. Sent like `send`. A role whose control writes the device's memory is refused: restore_presets (confirm_overwrite) does that.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id (see list_devices)." },
                        "role": { "type": "string", "description": "The role as listed by list_roles, e.g. filter/cutoff." },
                        "value": { "type": "number", "description": "From 0 to 1 on the range of the role." },
                        "target": { "type": "string", "description": "Optional override host:port. Defaults to MCP server's --default-target." }
                    },
                    "required": ["device", "role", "value"]
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "sent": { "type": "boolean" },
                        "address": { "type": "string" },
                        "reason": { "type": "string" },
                        "bytes": { "type": "integer" },
                        "target": { "type": "string" },
                        "role": { "type": "string", "description": "Present when the role is unknown." },
                        "nearest": { "type": "array", "items": { "type": "string" }, "description": "Present when the role is unknown: the nearest roles of the driver." }
                    }
                },
                "annotations": {
                    "title": "Set a control by its role",
                    "readOnlyHint": false,
                    "destructiveHint": false,
                    "idempotentHint": false,
                    "openWorldHint": true
                }
            },
            {
                "name": "get_by_role",
                "description": "Read a control's current value by its role: sends the read request the driver declares and gives the reply's value from 0 to 1 on the range of the role. A driver that declares no read for the control says it does not report it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id (see list_devices)." },
                        "role": { "type": "string", "description": "The role as listed by list_roles." },
                        "target": { "type": "string", "description": "Optional override host:port. Defaults to MCP server's --default-target." }
                    },
                    "required": ["device", "role"]
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string" },
                        "role": { "type": "string" },
                        "address": { "type": "string" },
                        "value": { "type": "number", "description": "From 0 to 1 on the range of the role." },
                        "raw": { "type": "number", "description": "The value the driver reported." },
                        "nearest": { "type": "array", "items": { "type": "string" }, "description": "Present when the role is unknown." }
                    }
                },
                "annotations": {
                    "title": "Read a control by its role",
                    "readOnlyHint": true,
                    "idempotentHint": true,
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
                "name": "add_link",
                "description": "Link two addresses of a running orchestrator, live: the next message at `from` also goes to `to`, with no restart. Optionally scale the first argument from an input range to an output range. Refused, with an error that names the address or the chain, when a device or an address is unknown to its driver, or when the chain of links would loop or pass four hops; nothing changes then.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "Source address, e.g. /keyboard/cc/1" },
                        "to": { "type": "string", "description": "Target address, e.g. /synth/filter/cutoff" },
                        "map": {
                            "type": "object",
                            "description": "Scaling of the first argument.",
                            "properties": {
                                "from": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Input range [lo, hi]." },
                                "to": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Output range [lo, hi]." }
                            },
                            "required": ["from", "to"]
                        },
                        "target": { "type": "string", "description": "host:port of the orchestrator, defaults to --default-target" }
                    },
                    "required": ["from", "to"]
                },
                "outputSchema": link_tool_output(),
                "annotations": {
                    "title": "Add a link",
                    "readOnlyHint": false,
                    "destructiveHint": false,
                    "idempotentHint": false,
                    "openWorldHint": true
                }
            },
            {
                "name": "remove_link",
                "description": "Remove a link of a running orchestrator, live: the next message no longer follows it. Refused, naming the link, when it does not exist.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "Source address of the link." },
                        "to": { "type": "string", "description": "Target address of the link." },
                        "target": { "type": "string", "description": "host:port of the orchestrator, defaults to --default-target" }
                    },
                    "required": ["from", "to"]
                },
                "outputSchema": link_tool_output(),
                "annotations": {
                    "title": "Remove a link",
                    "readOnlyHint": false,
                    "destructiveHint": true,
                    "idempotentHint": false,
                    "openWorldHint": true
                }
            },
            {
                "name": "list_links",
                "description": "List the links of a running orchestrator, those of its configuration file and those added live: one reply per link with its source, its target, its origin ('file' or 'live') and its scaling, then a final count.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "host:port of the orchestrator, defaults to --default-target" }
                    }
                },
                "outputSchema": link_tool_output(),
                "annotations": {
                    "title": "List the links",
                    "readOnlyHint": true,
                    "idempotentHint": true,
                    "openWorldHint": true
                }
            },
            {
                "name": "backup_presets",
                "description": "Back up a hardware device's presets as a .syx file. The device is named (device: a catalogue id or OSC prefix, see list_devices): its driver says which messages are its sounds, on any channel or device number, and how to ask for them. Listens on the device's MIDI input and records those messages (a 'bulk dump'), stopping 3 s after the last one. Either the user starts the dump from the device's menu during the call, or pass out_port and the driver's dump request is sent. Use detect_hardware first to find the ports.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "The device's catalogue id or OSC prefix (e.g. 'korg/minilogue'); a device the catalogue does not know has to be described first." },
                        "in_port": { "type": "string", "description": "MIDI IN port of the device: index or name fragment (e.g. 'minilab')." },
                        "file": { "type": "string", "description": "Path of the .syx file to write." },
                        "out_port": { "type": "string", "description": "MIDI OUT port: the driver's dump request is sent on it first (index or name fragment)." },
                        "max_seconds": { "type": "integer", "description": "Give up after this long, 1-600 (default 60)." }
                    },
                    "required": ["device", "in_port", "file"]
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
                "description": "Send a .syx file to a hardware device, e.g. to restore a preset backup. This OVERWRITES the device's memory: confirm with the user first, then pass confirm_overwrite: true. The device is named (device): its driver sets the pause between messages, refuses a bank that cannot be written before anything is sent, and the sounds are sent on the configured channel. For a sound that carries no place, pass bank and number.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "The device's catalogue id or OSC prefix (e.g. 'korg/minilogue'); a device the catalogue does not know has to be described first." },
                        "out_port": { "type": "string", "description": "MIDI OUT port of the device: index or name fragment." },
                        "file": { "type": "string", "description": "Path of the .syx file to send." },
                        "confirm_overwrite": { "type": "boolean", "description": "Must be true: the user agreed to overwrite the device's memory." },
                        "bank": { "type": "string", "description": "The bank to write into, by name or place (with number), for a sound that carries no place." },
                        "number": { "type": "integer", "description": "The program number to write into, as the device shows it (with bank)." }
                    },
                    "required": ["device", "out_port", "file", "confirm_overwrite"]
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
                "description": "Find the MIDI devices plugged into this computer and match them to catalogue drivers. Lists MIDI ports and sends each port the standard MIDI Identity Request (universal, read-only by the norm), then only the maker requests of the maker its USB id names (or, with no USB id read, of the drivers whose identity names its port name). A maker request may be a load for another maker's device, so every other one is held back, port by port: for a device nobody recognised only the universal request goes out, and each device lists held_back, the requests not sent. Consent adds them: pass consent_ports, the ports that may receive every maker request; the person must have been asked and agreed for that port (never pass it on your own). Reads each device's manufacturer, model and firmware. Reads the USB ids (vendor_id, product_id) of each USB device's card (Linux; absent for a virtual or DIN port). A driver whose identity block recognises the reply, the USB ids (usb) or the port name comes first, the variant for the firmware read from it above the others; of drivers matching equally, the one the others share their MIDI with comes first, and each lists the others in equally_matching. Returns, per device, the best matching drivers, each with its proofs (each proof has a nature, strong, medium or weak, and a kind: identity reply, usb id, maker, name) and its firmware when recognised. A candidate whose only proof is the name is 'possible, to confirm' and gets no command; a port with no other proof is 'unknown' and the learn mode takes over. A ready-to-run 'osc-bridge run' command with the right port numbers is given only for a candidate with a strong proof and no rival of equal weight.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "probe": { "type": "boolean", "description": "Send the Identity Request (default true). False only lists ports and matches by port name." },
                        "timeout_ms": { "type": "integer", "description": "How long to wait for identity replies, 100-5000 ms (default 800)." },
                        "consent_ports": { "type": "array", "items": { "type": "string" }, "description": "Ports (a part of the name, or the exact out_port index) that receive every maker request, the held back ones too. Ask the person first and pass a port only with their explicit agreement for that port: a request meant for another maker's device may overwrite its memory. A fragment naming several ports or none is refused, and nothing is sent." },
                        "learn": { "type": "boolean", "description": "Learn mode: instead of matching drivers, listen to each device and return its fingerprint (an OSD 0.6 fingerprint: usb, ports, identity_reply, sends, controls) with the measured delay of the identity reply and the period of what it sends alone, beside it. Sends nothing but the universal Identity Request; tell the person to move the device's controls while it listens." },
                        "listen_ms": { "type": "integer", "description": "Learn mode only: how long to listen, 1000-30000 ms (default 5000)." },
                        "share_fingerprint": { "type": "string", "description": "Learn mode only: the port (a part of its name) whose fingerprint is sent to SynthHub, so that everyone recognises this device. Giving it, with vendor, model and firmware, sends that device's fingerprint to SynthHub (POST /v1/fingerprints): the vendor, model and firmware, the USB vendor and product ids, the identity_reply, the ports (names without machine numbers), what the device sends and its controls; never a serial number, a host name, a measure or anything about the person. Nothing is sent for a private device or when offline. Ask the person first. Needs vendor, model and firmware, none empty." },
                        "vendor": { "type": "string", "description": "Learn mode only, with share_fingerprint: the maker of the device, as the person names it (60 characters at most). Required." },
                        "model": { "type": "string", "description": "Learn mode only, with share_fingerprint: the model of the device, as the person names it (60 characters at most). Required." },
                        "firmware": { "type": "string", "description": "Learn mode only, with share_fingerprint: the firmware version of the device (40 characters at most). Required: when the identity reply carries one, the report proposes it as proposed_firmware and the person confirms it; otherwise ask the person." }
                    }
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "probed": { "type": "boolean" },
                        "devices": {
                            "type": "array",
                            "description": "One entry per physical device (an output port paired with its input port by name); in learn mode, one entry per device with all its input and output ports.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "port": { "type": "string" },
                                    "out_port": { "type": ["integer", "null"] },
                                    "held_back": { "type": "array", "items": { "type": "string" }, "description": "The maker requests (hex) not sent to this port for want of the person's consent." },
                                    "in_port": { "type": ["integer", "null"] },
                                    "inputs": { "type": "array", "items": { "type": "string" }, "description": "Learn mode: the raw names of all the input ports of the device." },
                                    "outputs": { "type": "array", "items": { "type": "string" }, "description": "Learn mode: the raw names of all the output ports of the device." },
                                    "usb": { "type": "object", "description": "The USB ids of the device's card (Linux), absent when it has none.",
                                        "properties": { "vendor_id": { "type": "integer" }, "product_id": { "type": "integer" } } },
                                    "identity": { "type": ["object", "null"], "description": "Identity Reply, when the device answered." },
                                    "candidates": { "type": "array", "items": { "type": "object" }, "description": "The catalogue drivers matching the device; one being confirmed carries confirmations, state and state_label." },
                                    "unlisted": { "type": "array", "items": { "type": "object" }, "description": "Only when no candidate: the devices the service names for it without a card (vendor, model, firmware, confirmations, disputed, state, state_label); proposed to the person, never imposed." },
                                    "run_command": { "type": ["string", "null"] },
                                    "fingerprint": { "type": "object", "description": "Learn mode: the OSD 0.6 fingerprint of the device; a field it did not give is absent." },
                                    "measured": { "type": "object", "description": "Learn mode: local measures beside the fingerprint: identity_reply_ms with identity_reply_port, periods_ms (clock, active_sensing) with periods_port (the port each was measured on); absent when not observed." }
                                },
                                "required": ["port"]
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
                        "device": { "type": "string", "description": "Driver id (e.g. 'korg/minilogue') or OSC prefix, from list_devices." },
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
                        "device": { "type": "string", "description": "Driver id (e.g. 'korg/minilogue') or OSC prefix, from list_devices." }
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
                "description": "Teach the catalogue which MIDI message a control really sends: after the user moved one control on the device (see relay_listen), give the driver address it is (from list_routes) — or, for a control the driver lacks, its panel label — and the CC, NRPN or note it sent, with the firmware and the device's Identity Reply when you have them. When several accounts observe the same thing on one firmware the mapping is learned; get_trust shows learned mappings and where they differ from the driver. For a device with no driver, the first step of its first driver: after detect_hardware in learn mode (its next_step), the person gives its vendor, model and firmware (share_fingerprint), and you map its controls one by one with device set to its vendor/model id (the slugs of the vendor and the model, e.g. behringer/bcr-32); the mappings then make the first driver you send with submit_driver.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id or OSC prefix; for a device with no driver yet, its vendor/model id as its fingerprint was shared (e.g. behringer/bcr-32)." },
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
                "description": "Contribute a driver for a device the catalogue lacks (or a better one): the full driver file as a JSON object, in the Open Synth Descriptors format (device.name, device.vendor, device.osc_prefix, and cc_params / params / commands / triggers — see https://synthhub.roomi-fields.com/osd/). It is checked, then merged into the SynthHub catalogue by the merge job if it parses, lints and passes the tests, as its own file of source 'agent-contributed'. Say where the data comes from (manual page, tested on the device). A first driver for a device with no driver: build it from the mappings sent with report_mapping, naming the device by the vendor and model its fingerprint was shared with, and give its source and its licence (e.g. 'mapped on the device by report_mapping; license CC0-1.0').",
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
                    "device": { "type": "string", "description": "Driver id or OSC prefix." },
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
                "name": "share_preset",
                "description": "Share something the user made on a device with every SynthHub user, under CC BY-SA 4.0: a sound, a sequence or pattern, a kit, a project, a bank of their own sounds, or a MIDI clip any device can play. Its content: the device's own data (SysEx, e.g. from backup_presets), settings by driver address (from list_routes, e.g. {'/filter/cutoff': [90]}, which also works for devices without dumps), and/or a standard MIDI file. Only what the user made: never factory presets, nor sounds or sequences bought or downloaded from others — set own_sound only after asking the user. Made from someone else's shared preset? Pass derived_from: it credits them, as the licence requires. Describe it well (category, genres, tags, tempo, key, tips): that is how people find it. The same content shared twice returns the existing id; the author publishes new versions with update_preset and can withdraw it (withdraw_preset).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id (e.g. 'korg/minilogue') or OSC prefix." },
                        "name": { "type": "string", "description": "Its name (80 characters at most)." },
                        "own_sound": { "type": "boolean", "description": "Must be true: the user made it (not a factory preset, not someone else's)." },
                        "kind": { "type": "string", "enum": ["sound", "sequence", "kit", "project", "bank", "clip"], "description": "What it is (default sound)." },
                        "category": { "type": "string", "enum": ["bass", "lead", "pad", "pluck", "keys", "organ", "brass", "strings", "bell", "arp", "sequence", "drums", "percussion", "fx", "texture", "vocal", "other"], "description": "The kind of sound or part." },
                        "genres": { "type": "array", "items": { "type": "string" }, "description": "Musical styles: 'techno', 'ambient'… (8 at most)." },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Character and free words: 'warm', 'dark', 'evolving'… (8 at most)." },
                        "description": { "type": "string", "description": "What it sounds like (500 characters at most)." },
                        "tips": { "type": "string", "description": "How to play it: mod wheel, aftertouch, register… (500 characters at most)." },
                        "tempo": { "type": "number", "description": "BPM, for sequences and clips." },
                        "key": { "type": "string", "description": "Musical key, e.g. 'A minor'." },
                        "firmware": { "type": "string", "description": "Firmware version of the device it was made on." },
                        "derived_from": { "type": "string", "description": "Id of the shared preset it was made from." },
                        "hex": { "type": "string", "description": "The device data as SysEx, in hex (64 KB at most)." },
                        "file": { "type": "string", "description": "A .syx file on this machine (e.g. written by backup_presets), instead of hex." },
                        "settings": { "type": "object", "description": "Settings by driver address: {'/filter/cutoff': [90], '/amp/attack': [12]} (512 at most).", "additionalProperties": true },
                        "midi_file": { "type": "string", "description": "A standard MIDI file (.mid) on this machine." },
                        "audio_file": { "type": "string", "description": "A short recording of it (MP3, Ogg, M4A or WebM, 1 MB at most) on this machine, as its preview." },
                        "librarian": { "type": "string", "description": "The synth the librarian took it from (take_sound's librarian): loading it then goes through the same exchange, fitted to the receiving synth." }
                    },
                    "required": ["device", "name", "own_sound"],
                    "additionalProperties": false
                },
                "annotations": { "title": "Share a preset", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true }
            },
            {
                "name": "update_preset",
                "description": "Publish a new version of a preset this account shared: new content (hex, file, settings or midi_file) makes a new version, kept with a change note — earlier versions stay available; the description fields alone change it in place. Someone else's preset: share yours with derived_from instead.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "changelog": { "type": "string", "description": "What changed in this version (500 characters at most)." },
                        "name": { "type": "string" }, "description": { "type": "string" }, "tips": { "type": "string" },
                        "category": { "type": "string", "enum": ["bass", "lead", "pad", "pluck", "keys", "organ", "brass", "strings", "bell", "arp", "sequence", "drums", "percussion", "fx", "texture", "vocal", "other"] },
                        "genres": { "type": "array", "items": { "type": "string" } }, "tags": { "type": "array", "items": { "type": "string" } },
                        "tempo": { "type": "number" }, "key": { "type": "string" }, "firmware": { "type": "string" },
                        "hex": { "type": "string" }, "file": { "type": "string" },
                        "settings": { "type": "object", "additionalProperties": true }, "midi_file": { "type": "string" },
                        "audio_file": { "type": "string", "description": "A new preview recording for the current version." }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                },
                "annotations": { "title": "Update a preset", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true }
            },
            {
                "name": "find_presets",
                "description": "Search what SynthHub users shared (CC BY-SA 4.0): sounds, sequences, kits, projects, banks, MIDI clips. Filter by device, kind, category, genre, tag, author (pseudonym) or words; sort by newest, most stars, most loaded into a device or most trusted (confirmed on real devices). Returns, per preset, its id, name, kind, device, description, author, version, whether it has device data, settings, a MIDI file or an audio preview (URL), and its counts. Get one with get_preset.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id or OSC prefix (its other variants are included)." },
                        "kind": { "type": "string", "enum": ["sound", "sequence", "kit", "project", "bank", "clip"] },
                        "category": { "type": "string", "enum": ["bass", "lead", "pad", "pluck", "keys", "organ", "brass", "strings", "bell", "arp", "sequence", "drums", "percussion", "fx", "texture", "vocal", "other"] },
                        "genre": { "type": "string" },
                        "tag": { "type": "string" },
                        "author": { "type": "string", "description": "An author's pseudonym (e.g. 'u1a2b3c4d')." },
                        "query": { "type": "string", "description": "Words that must all appear in the name, description, tags, genres, category or device id ('dx7 piano')." },
                        "sort": { "type": "string", "enum": ["new", "stars", "loads", "trusted"], "description": "new (default), most stars, most loaded into a device, or most confirmed on real devices." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "At most this many (default 20)." },
                        "offset": { "type": "integer", "minimum": 0, "description": "Skip this many: the next page (the reply's total says how many match)." }
                    },
                    "additionalProperties": false
                },
                "annotations": { "title": "Find presets", "readOnlyHint": true, "idempotentHint": true, "openWorldHint": true }
            },
            {
                "name": "get_preset",
                "description": "One shared preset in full: its content (SysEx hex, settings by driver address, MIDI file), description and playing tips, versions with their change notes, what it was derived from and its derivatives (remixes), comments, preview URLs, author, licence and attribution line, trust and counts. Load it with load_preset; credit it as the attribution line says when you publish something made from it.",
                "inputSchema": { "type": "object", "properties": {
                    "id": { "type": "string", "description": "Preset id from find_presets (e.g. 'p3fa9c1d2e4b7')." },
                    "version": { "type": "integer", "minimum": 1, "description": "An earlier version (default the current one)." } },
                    "required": ["id"], "additionalProperties": false },
                "annotations": { "title": "Get a preset", "readOnlyHint": true, "idempotentHint": true, "openWorldHint": true }
            },
            {
                "name": "report_preset",
                "description": "Tell SynthHub how a shared preset behaved once loaded on a real device: 'works' (it loaded and sounds as described), 'fails' (refused, wrong sound), or 'factory' (it is a factory preset or someone else's sound: presets that several users flag are hidden until reviewed). Give the firmware.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "outcome": { "type": "string", "enum": ["works", "fails", "factory"] },
                        "firmware": { "type": "string" },
                        "note": { "type": "string", "description": "What happened, briefly (500 characters at most)." }
                    },
                    "required": ["id", "outcome"],
                    "additionalProperties": false
                },
                "annotations": { "title": "Report on a preset", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
            },
            {
                "name": "withdraw_preset",
                "description": "Withdraw a preset this account shared: it is no longer listed nor served. Copies others already made stay under CC BY-SA.",
                "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"], "additionalProperties": false },
                "annotations": { "title": "Withdraw a preset", "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false }
            },
            {
                "name": "comment_preset",
                "description": "Comment on a shared preset — how it sounds, how you used it, what you changed — optionally attaching your remix (a preset you shared with derived_from). Comments are public at once. Also: remove one of this account's comments (remove), or flag someone else's as abusive or off-topic (flag); comments several accounts flag are hidden until reviewed.",
                "inputSchema": { "type": "object", "properties": {
                    "id": { "type": "string", "description": "The preset to comment on." },
                    "text": { "type": "string", "description": "The comment (2000 characters at most)." },
                    "remix": { "type": "string", "description": "Id of your remix, to attach." },
                    "remove": { "type": "integer", "description": "Id of this account's comment to remove (instead of commenting)." },
                    "flag": { "type": "integer", "description": "Id of someone else's comment to flag (instead of commenting)." } },
                    "additionalProperties": false },
                "annotations": { "title": "Comment on a preset", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true }
            },
            {
                "name": "star_preset",
                "description": "Star a shared preset for this account, as on GitHub (on: false takes the star back). Without id, lists the presets this account starred.",
                "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }, "on": { "type": "boolean", "description": "Default true." } },
                    "additionalProperties": false },
                "annotations": { "title": "Star a preset", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
            },
            {
                "name": "preset_collection",
                "description": "Collections of shared presets, public, made by one account (e.g. 'Dark basses for minilogue'). action: create (name, description), rename (collection, name, description), add or remove (collection, id), delete (collection), get (collection: its presets), list (this account's collections, or author: someone's).",
                "inputSchema": { "type": "object", "properties": {
                    "action": { "type": "string", "enum": ["create", "rename", "add", "remove", "delete", "get", "list"] },
                    "collection": { "type": "string", "description": "Collection id (e.g. 'c1a2b3c4d5e6')." },
                    "name": { "type": "string" }, "description": { "type": "string" },
                    "id": { "type": "string", "description": "Preset id to add or remove." },
                    "author": { "type": "string", "description": "With list: an author's pseudonym." } },
                    "required": ["action"], "additionalProperties": false },
                "annotations": { "title": "Preset collections", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false }
            },
            {
                "name": "follow_author",
                "description": "Follow an author of shared presets (by pseudonym), or stop (on: false); with author alone and on omitted, shows their page: profile, presets, followers. Without author, lists the newest presets of the authors this account follows.",
                "inputSchema": { "type": "object", "properties": { "author": { "type": "string" }, "on": { "type": "boolean" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 } }, "additionalProperties": false },
                "annotations": { "title": "Follow an author", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
            },
            {
                "name": "set_profile",
                "description": "Set the public profile shown on this account's author page and with its shared presets and comments, instead of its pseudonym alone: display name, link (https), a few words, the presets put first, where they are, more links, their instruments and a README. A field left out stays as it is; an empty one is cleared. Ask the user what they want shown; all of it is public.",
                "inputSchema": { "type": "object", "properties": { "name": { "type": "string", "description": "40 characters at most." },
                    "link": { "type": "string", "description": "An https:// address: their site, SoundCloud, Bandcamp…" },
                    "bio": { "type": "string", "description": "A few words about them and their sounds, 600 characters at most." },
                    "featured": { "type": "array", "items": { "type": "string" }, "maxItems": 6,
                        "description": "Ids of their own listed presets, shown first on their page, in this order." },
                    "location": { "type": "string", "description": "Where they make music, 60 characters at most." },
                    "links": { "type": "array", "items": { "type": "string" }, "maxItems": 8,
                        "description": "More https:// pages of theirs: a channel, a shop, a band." },
                    "gear": { "type": "array", "items": { "type": "string" }, "maxItems": 20,
                        "description": "The instruments they play, e.g. 'Korg minilogue'." },
                    "about": { "type": "string", "description": "Their README in Markdown: who they are, how they work, what they offer; 8000 characters at most." } },
                    "additionalProperties": false },
                "annotations": { "title": "Set the public profile", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
            },
            {
                "name": "link_account",
                "description": "Link this machine's SynthHub account to the user's account on the site: returns a one-time address (valid 15 minutes) for the user to open and sign in (passkey, e-mail, Google or Discord). Afterwards what this copy shared appears under their profile, and they manage it — and their stars, collections, comments — from the site.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
                "annotations": { "title": "Link to the user's account", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true }
            },
            {
                "name": "take_sound",
                "description": "Take the sound a hardware synth is playing — or one of its stored programs — straight from the synth, with its name, and save it as a .syx file. The synth is found among those SynthHub's librarian can talk to (the KnobKraft Orm adaptations, about 90 synths): every MIDI port gets the identity and detect messages, which change nothing on the synth, then the found synth is asked for its sound. To share it: ask the user whether they made this sound themselves (never a factory preset, nor one bought or downloaded), then call share_preset with the device, name, file, librarian and firmware returned here, and own_sound: true. When several synths answer, call again with port.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "port": { "type": "string", "description": "MIDI port of the synth: index or name fragment (detect_hardware). Default: every port." },
                        "program": { "type": "integer", "minimum": 1, "description": "A stored program (from 1) instead of the sound being played." },
                        "file": { "type": "string", "description": "Where to save the .syx file (default: the system's temporary folder)." }
                    },
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "synth": { "type": "string" },
                        "librarian": { "type": "string" },
                        "device": { "type": ["string", "null"], "description": "The catalogue driver to share it under; null if the catalogue has none yet." },
                        "name": { "type": "string" },
                        "firmware": { "type": ["string", "null"] },
                        "port": { "type": "string" },
                        "file": { "type": "string" },
                        "messages": { "type": "integer" },
                        "bytes": { "type": "integer" },
                        "note": { "type": ["string", "null"], "description": "What the user should know about this synth's sounds." }
                    },
                    "required": ["synth", "librarian", "name", "port", "file", "messages", "bytes"]
                },
                "annotations": { "title": "Take a synth's sound", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true }
            },
            {
                "name": "load_preset",
                "description": "Load a shared preset (find_presets, get_preset) into a device plugged into this computer: its settings are sent by driver address, its patch data as SysEx. Patch data replaces the sound in the device's edit buffer or memory: ask the user, then pass confirm_overwrite: true. Afterwards, tell SynthHub how it went with report_preset.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "out_port": { "type": "string", "description": "MIDI OUT port of the device: index or name fragment (detect_hardware)." },
                        "gap_ms": { "type": "integer", "description": "Pause between messages, 0-2000 ms (default 50)." },
                        "confirm_overwrite": { "type": "boolean", "description": "True once the user agreed; needed when the preset carries patch data." }
                    },
                    "required": ["id", "out_port"],
                    "additionalProperties": false
                },
                "annotations": { "title": "Load a preset", "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": true }
            }

        ]
    })
}

/// The output of the three link tools: the orchestrator's replies, as `get_status` gives them.
fn link_tool_output() -> Value {
    json!({
        "type": "object",
        "properties": {
            "target": { "type": "string" },
            "replied": { "type": "boolean", "description": "False when no reply arrived within 500 ms." },
            "replies": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": { "addr": { "type": "string" }, "args": { "type": "array" } },
                    "required": ["addr", "args"]
                }
            }
        },
        "required": ["target", "replied", "replies"]
    })
}

fn tool_names(opts: &McpOptions) -> Vec<&'static str> {
    let local = ["list_devices", "get_device_docs", "list_routes", "export_device", "send", "get_status",
         "add_link", "remove_link", "list_links", "detect_hardware", "backup_presets", "restore_presets", "install_export",
         "list_roles", "set_by_role", "get_by_role"];
    // Taking and loading sounds needs the server's librarian, like the forwarded tools.
    let linked: &[&str] = if opts.online { &["take_sound", "load_preset"] } else { &[] };
    let forwarded = if opts.online { crate::online::FORWARDED_TOOLS.as_slice() } else { &[] };
    local.iter().chain(forwarded).chain(linked).copied().collect()
}

#[cfg(test)]
fn call_tool(name: &str, args: &Value, opts: &McpOptions, index: &DeviceIndex, link: Option<&Link>) -> Result<Value, String> {
    call_tool_with(name, args, opts, index, link, &crate::take::open_hardware)
}

fn call_tool_with(name: &str, args: &Value, opts: &McpOptions, index: &DeviceIndex, link: Option<&Link>,
                  open: crate::take::Opener) -> Result<Value, String> {
    if args.get("slug").is_some() { device_arg(args)?; }
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
                    return Err(format!("{id} is private on this machine: {name} is not sent to the SynthHub server. \
                        To share it, remove it from the private list (osc-bridge privacy shows the settings file)."));
                }
            }
            if name == "share_preset" || name == "update_preset" {
                return share_with_files(link, name, args);
            }
            return link.forward(name, args);
        }
        if name == "take_sound" {
            return take_sound(args, link, open);
        }
        if name == "load_preset" {
            return load_preset(args, link, open);
        }
    }
    // Drivers from the server: it finds them, and matches the ports found here.
    let server = link.filter(|_| opts.devices_dir.is_none());
    if let (Some(link), "list_devices") = (server, name) {
        return link.client.catalogue_call(name, args).map_err(|e| format!("list_devices asks the SynthHub server, \
            which could not be reached: {e}. Offline, it lists the drivers cached on this machine."));
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
        "list_roles" => roles::list_roles(index, args),
        "set_by_role" | "get_by_role" => {
            let target = match args.get("target").and_then(Value::as_str) {
                Some(s) => s.parse::<SocketAddr>().map_err(|e| format!("invalid target: {e}"))?,
                None => opts.default_target,
            };
            if name == "set_by_role" { roles::set_by_role(index, args, target) } else { roles::get_by_role(index, args, target) }
        }
        "get_status" => {
            let target = match args.get("target").and_then(Value::as_str) {
                Some(s) => s.parse::<SocketAddr>().map_err(|e| format!("invalid target: {e}"))?,
                None => opts.default_target,
            };
            get_status(target)
        }
        "add_link" | "remove_link" | "list_links" => {
            let target = match args.get("target").and_then(Value::as_str) {
                Some(s) => s.parse::<SocketAddr>().map_err(|e| format!("invalid target: {e}"))?,
                None => opts.default_target,
            };
            link_tool(name, args, target)
        }
        "backup_presets" => {
            let wanted = device_arg(args)?;
            let in_port = args.get("in_port").and_then(Value::as_str).ok_or("missing in_port")?;
            let file = args.get("file").and_then(Value::as_str).ok_or("missing file")?;
            let out_port = args.get("out_port").and_then(Value::as_str);
            let max = args.get("max_seconds").and_then(Value::as_u64).unwrap_or(60).clamp(1, 600);
            let dev = preset_device(index, wanted)?;
            backup_presets(&dev, in_port, Path::new(file), out_port, max)
        }
        "restore_presets" => {
            let wanted = device_arg(args)?;
            if args.get("confirm_overwrite").and_then(Value::as_bool) != Some(true) {
                return Err("restore overwrites the device's memory: ask the user, then pass confirm_overwrite: true".into());
            }
            let out_port = args.get("out_port").and_then(Value::as_str).ok_or("missing out_port")?;
            let file = args.get("file").and_then(Value::as_str).ok_or("missing file")?;
            let target = match (args.get("bank"), args.get("number").and_then(Value::as_i64)) {
                (Some(b), Some(n)) => Some((match b { Value::String(s) => s.clone(), v => v.to_string() }, n)),
                (None, None) => None,
                _ => return Err("bank and number go together: the place to write the sound".into()),
            };
            let dev = preset_device(index, wanted)?;
            restore_presets(&dev, out_port, Path::new(file), target.as_ref().map(|(b, n)| (b.as_str(), *n)))
        }
        "install_export" => {
            let wanted = device_arg(args)?;
            let on = args.get("on").and_then(Value::as_str).ok_or("missing on: the device that receives the export")?;
            let confirm = args.get("confirm_overwrite").and_then(Value::as_bool) == Some(true);
            install_export(&preset_device(index, wanted)?, &preset_device(index, on)?, wanted, on, confirm, open)
        }
        "detect_hardware" if args.get("learn").and_then(Value::as_bool) == Some(true) => {
            let text = |k: &str| args.get(k).and_then(Value::as_str);
            let share = match text("share_fingerprint") {
                None if ["vendor", "model", "firmware"].iter().any(|k| text(k).is_some()) =>
                    return Err("vendor, model and firmware go with share_fingerprint: the port whose fingerprint is shared".into()),
                None => None,
                Some(port) => {
                    let (vendor, model, firmware) = share_fields(text("vendor"), text("model"), text("firmware"), false)?;
                    Some(Share { port, vendor, model, firmware, command: false, client: link.map(|l| &l.client), private: &opts.private })
                }
            };
            learn_hardware(index, server.map(|l| &l.client), crate::detect::learn_listen(args.get("listen_ms").and_then(Value::as_u64))?, share)
        }
        "detect_hardware" => {
            let probe = args.get("probe").and_then(Value::as_bool).unwrap_or(true);
            let wait = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(800).clamp(100, 5000);
            let consent = consent_arg(args)?;
            detect_hardware(index, server.map(|l| &l.client), opts.devices_dir.is_some(), probe, std::time::Duration::from_millis(wait), None, &consent)
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// `consent_ports`: the ports the person agreed to send every maker request to, read like `out_port`.
fn consent_arg(args: &Value) -> Result<Vec<String>, String> {
    match args.get("consent_ports") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(str::to_string)
            .or_else(|| v.as_u64().map(|n| n.to_string())).ok_or_else(|| "consent_ports: each entry is a port name or index".to_string())).collect(),
        Some(_) => Err("consent_ports: a list of port names or indexes".into()),
    }
}

/// The one reading of the argument that names a device: `device`. The former
/// `slug`, alone or beside `device`, is refused before any device is read.
fn device_arg(args: &Value) -> Result<&str, String> {
    if args.get("slug").is_some() {
        return Err("`slug` no longer exists: name the device by `device` (a catalogue id or an OSC prefix, see list_devices)".into());
    }
    args.get("device").and_then(Value::as_str).ok_or_else(|| "missing device: name the device (a catalogue id or an OSC prefix, see list_devices)".to_string())
}

/// The catalogue tools — `list_devices`, `get_device_docs`, `list_routes`,
/// `export_device` — shared with the hosted server; None for any other
/// tool. `status` gives a driver's trust for `list_devices`, when known.
pub fn catalogue_tool(name: &str, args: &Value, index: &DeviceIndex, status: &dyn Fn(&str) -> Option<String>)
                      -> Option<Result<Value, String>> {
    Some(match name {
        "list_devices" => list_devices(index, &DeviceFilter::from_args(args), status),
        "get_device_docs" => device_arg(args).and_then(|d| get_device_docs(index, d)),
        "list_routes" => device_arg(args).and_then(|d| list_routes(index, d)),
        "export_device" => export_device(index, args),
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
        // Named as the private list names it, even when no driver of it is here.
        if covers(wanted, wanted) {
            return Some(index.find(wanted).map(|e| e.id.clone()).unwrap_or_else(|| wanted.to_string()));
        }
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
/// made private or public again (ids or slugs, found in `index`, else asked
/// of `server`) — then show them. Returns what to print.
pub fn privacy(path: &Path, index: &DeviceIndex, server: Option<&crate::online::Online>, offline: Option<bool>,
               private: &[String], public: &[String]) -> Result<String> {
    let mut settings = crate::online::Settings::load(Some(path))?;
    let id_of = |name: &str| match name.strip_suffix("/*") {
        Some(vendor) => crate::catalogue::is_slug(vendor).then(|| name.to_string())
            .ok_or_else(|| anyhow::anyhow!("'{vendor}' is not a vendor folder of the catalogue")),
        None => index.find(name).map(|e| e.id.clone())
            .or_else(|| {
                // An id the server knows is kept as it is; a slug stands for its best variant.
                let found = server?.variants(name).ok()?;
                let known = found["variants"].as_array()?.iter().any(|v| v["id"] == name);
                if known { Some(name.to_string()) } else { found["best"].as_str().map(str::to_string) }
            })
            .ok_or_else(|| anyhow::anyhow!("no device '{name}' in the catalogue (use an id or OSC prefix from list_devices, or vendor/*)")),
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
        "Offline: this copy does not contact the SynthHub server and uses only the drivers cached on this machine \
         (osc-bridge privacy --offline off to link it again).\n".to_string()
    } else {
        "Linked to the SynthHub server: drivers come from it one at a time under the anonymous account of this machine, and reports \
         and fixes are shared under that account (osc-bridge privacy --offline on to stop).\n".to_string()
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
        id: e.id.clone(),
        slug: e.slug.clone(),
        name: e.name.clone(),
        vendor: e.vendor.clone(),
        file: e.path.display().to_string(),
        tier: e.tier.clone(),
        manufacturer_id: e.manufacturer_id.clone(),
        identity: e.identity.clone(),
        firmware: e.firmware.clone(),
        midi_of: e.midi_of.clone(),
    }).collect()
}

/// The maker requests the identity blocks of `index` name, sent while
/// detecting after the Universal Identity Request.
pub fn detect_probes(index: &DeviceIndex) -> Vec<crate::detect::Probe> {
    detect_plan(index).probes
}

/// The requests of `detect_probes` with the faults of the identities set aside.
fn detect_plan(index: &DeviceIndex) -> crate::detect::Probes {
    crate::detect::probes(index.entries.iter().filter(|e| e.kind != "software")
        .filter_map(|e| e.identity.as_ref().map(|i| (e.name.as_str(), i))))
}

/// A fault as one line of text: the device, the field and the cause.
fn fault_line(f: &crate::device::DeviceFault) -> String {
    format!("{}: {}: {}", f.device, f.field, f.cause)
}

/// The text blocks `detect_hardware` adds after its report, one per fault of an identity set
/// aside: only the Universal Identity Request went out for it. Pure: no MIDI.
pub fn fault_blocks(faults: &[crate::device::DeviceFault]) -> Vec<String> {
    faults.iter().map(|f| format!(
        "Fault, {}: its own identity request was not sent, only the Universal Identity Request was.", fault_line(f))).collect()
}

/// The probes as the hosted server lists them (`GET /v1/detect`).
pub fn probes_json(probes: &[crate::detect::Probe]) -> Value {
    let one = |p: &crate::detect::Probe| {
        let mut o = json!({ "request_hex": hex_bytes(&p.request), "ports": p.ports });
        if !p.usb.is_empty() {
            o["usb"] = p.usb.iter().map(|(v, d)| json!({ "vendor_id": v, "product_id": d })).collect();
        }
        if !p.match_ports.is_empty() {
            o["match_ports"] = json!(p.match_ports);
        }
        o
    };
    json!({ "probes": probes.iter().map(one).collect::<Vec<_>>() })
}

/// Bytes as `F0 7E 7F …`.
pub fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// What a candidate resting on a name alone is called.
const POSSIBLE_LABEL: &str = "possible, to confirm";

/// The proofs of a candidate as published: `{nature, kind}` each.
fn proofs_json(proofs: &[crate::detect::Proof]) -> Value {
    json!(proofs.iter().map(|p| json!({ "nature": p.nature.as_str(), "kind": p.kind })).collect::<Vec<_>>())
}

/// Whether a published candidate holds a strong proof.
fn has_strong(c: &Value) -> bool {
    c["proofs"].as_array().is_some_and(|ps| ps.iter().any(|p| p["nature"] == "strong"))
}

/// The drivers of `catalogue` (`hardware_candidates`) that best match a MIDI
/// port, by its name and the SysEx the device sent back while detected
/// (`detect::rank`), best first: id, slug, name, vendor, source tier,
/// score, and, when its identity block recognised the device, how, the
/// firmware read from the reply and the drivers matching it equally. The hosted server answers
/// `POST /v1/detect` with them.
pub fn match_port(catalogue: &[crate::detect::Candidate], port: &str, usb: Option<(u16, u16)>,
                  replies: &[Vec<u8>]) -> Vec<Value> {
    crate::detect::rank(port, usb, replies, catalogue, 3).iter().map(|r| {
        let c = &catalogue[r.index];
        let mut out = json!({ "id": c.id, "slug": c.slug, "name": c.name, "vendor": c.vendor, "source_tier": c.tier, "score": r.score });
        out["proofs"] = proofs_json(&r.proofs);
        if !r.is_strong() {
            out["state"] = json!("possible");
            out["state_label"] = json!(POSSIBLE_LABEL);
        }
        if let Some(fw) = r.recognised.as_ref().and_then(|rec| rec.firmware.as_ref()) { out["firmware"] = json!(fw); }
        if !r.equally.is_empty() {
            out["equally_matching"] = json!(r.equally.iter().map(|&i| &catalogue[i].id).collect::<Vec<_>>());
        }
        out
    }).collect()
}

/// Find the MIDI devices plugged into this machine and their drivers: those
/// of `index`, or, given `server`, those of the SynthHub catalogue it
/// matches the ports against. The command to run names the driver by its
/// file when `by_file` (a folder the user keeps), else by its id.
fn detect_hardware(index: &DeviceIndex, server: Option<&crate::online::Online>, by_file: bool, probe: bool,
                   wait: std::time::Duration, write: Option<&Path>, consent: &[String]) -> Result<Value, String> {
    let mut faults = Vec::new();
    let probes = match server {
        Some(server) if probe => server.probes().map_err(|e| format!("detect_hardware asks the SynthHub server which \
            requests recognise devices, and it could not be reached: {e}. Offline, the ports are matched against the drivers cached here."))?,
        _ => { let plan = detect_plan(index); faults = plan.faults; plan.probes }
    };
    let capture = crate::detect::observe(probe, wait, &probes, consent).map_err(|e| format!("MIDI scan: {e}"))?;
    if let Some(path) = write {
        capture.write(path).map_err(|e| e.to_string())?;
    }
    let plan = if probe { held_plan(&capture, &probes, consent)? } else { Vec::new() };
    detection_report(index, server, by_file, probe, crate::detect::replay(&capture), faults, &plan)
}

/// What each output port of `capture` received of `probes`, and what was held back.
fn held_plan(capture: &crate::capture::Capture, probes: &[crate::detect::Probe], consent: &[String]) -> Result<Vec<crate::detect::PortSends>, String> {
    let consented = crate::detect::consent_indexes(&capture.outputs, consent)?;
    Ok(crate::detect::send_plan(&crate::detect::output_usbs(capture), probes, &consented))
}

/// The devices a capture shows, matched against the drivers of `devices_dir`: the report of
/// `detect_hardware`, with no device plugged in and nothing sent.
pub fn detect_capture(devices_dir: &Path, capture: &crate::capture::Capture) -> Result<Vec<Value>, String> {
    let index = DeviceIndex::build(devices_dir);
    let report = detection_report(&index, None, true, false, crate::detect::replay(capture), Vec::new(), &[])?;
    Ok(report["structuredContent"]["devices"].as_array().cloned().unwrap_or_default())
}

/// `detect_capture` with the requests `probes` would go out as, given the `consent` of the person:
/// each device lists the maker requests held back for its output port.
pub fn detect_capture_held(devices_dir: &Path, capture: &crate::capture::Capture, probes: &[crate::detect::Probe],
                           consent: &[String]) -> Result<Vec<Value>, String> {
    let index = DeviceIndex::build(devices_dir);
    let plan = held_plan(capture, probes, consent)?;
    let report = detection_report(&index, None, true, true, crate::detect::replay(capture), Vec::new(), &plan)?;
    Ok(report["structuredContent"]["devices"].as_array().cloned().unwrap_or_default())
}

/// The candidates of the ports of one device as one list: a driver named on several ports is one
/// candidate, scored at its best port, with the proofs of every port (each nature and kind once,
/// naming the port that gave it); the entry of a port with a strong proof leads, so it beats the
/// name alone of the others. Best score first.
fn merge_candidates(found: &[crate::detect::Probed], per_port: Vec<(usize, Vec<Value>)>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::new();
    for (i, candidates) in per_port {
        let port = found[i].label();
        for mut c in candidates {
            let proofs: Vec<Value> = c["proofs"].as_array().cloned().unwrap_or_default().into_iter()
                .map(|mut p| { p["port"] = json!(port); p }).collect();
            c["proofs"] = json!(proofs);
            let Some(at) = merged.iter().position(|m| m["id"] == c["id"]) else { merged.push(c); continue };
            let mut base = std::mem::replace(&mut merged[at], Value::Null);
            let mut all = base["proofs"].as_array().cloned().unwrap_or_default();
            let score = base["score"].as_u64().max(c["score"].as_u64());
            for p in c["proofs"].as_array().cloned().unwrap_or_default() {
                if !all.iter().any(|q| q["nature"] == p["nature"] && q["kind"] == p["kind"]) { all.push(p); }
            }
            if has_strong(&c) && !has_strong(&base) { std::mem::swap(&mut base, &mut c); }
            base["proofs"] = json!(all);
            base["score"] = json!(score);
            if has_strong(&base) && base["state"] == "possible" {
                if let Some(o) = base.as_object_mut() { o.remove("state"); o.remove("state_label"); }
            }
            merged[at] = base;
        }
    }
    // A device recognised by a strong proof is not also offered as a guess on a name.
    if merged.iter().any(has_strong) {
        merged.retain(has_strong);
    }
    merged.sort_by(|a, b| b["score"].as_u64().cmp(&a["score"].as_u64()));
    merged
}

/// What a port of a program keeps of the candidates: those whose card the local catalogue knows,
/// by id, as a software. A card it does not know, or of another kind, is no candidate for a program.
fn software_cards_only(index: &DeviceIndex, candidates: Vec<Value>) -> Vec<Value> {
    candidates.into_iter().filter(|c| c["id"].as_str()
        .and_then(|id| index.find(id)).is_some_and(|e| e.kind == "software")).collect()
}

/// The report of `detect_hardware` over the devices `found`.
fn detection_report(index: &DeviceIndex, server: Option<&crate::online::Online>, by_file: bool, probe: bool,
                    found: Vec<crate::detect::Probed>, faults: Vec<crate::device::DeviceFault>,
                    plan: &[crate::detect::PortSends]) -> Result<Value, String> {
    let mut matched: Vec<Vec<Value>> = match server {
        Some(server) => {
            let ports: Vec<Value> = found.iter().map(|d| detect_port(d.label(),
                d.replies.iter().map(|r| hex_bytes(r)).collect(), d.usb)).collect();
            let answers = server.detect(&ports).map_err(|e| format!("detect_hardware asks the SynthHub server to match \
                the ports, and it could not be reached: {e}. Offline, the ports are matched against the drivers cached here."))?;
            (0..found.len()).map(|i| answers.get(i).map(port_answer).unwrap_or_default()).collect()
        }
        None => {
            let catalogue = hardware_candidates(index);
            found.iter().map(|d| match_port(&catalogue, d.label(), d.usb, &d.replies).into_iter().map(|mut c| { c["kind"] = json!("candidate"); c }).collect()).collect()
        }
    };
    // One answer per device: the ports of a device are judged together, a candidate keeping the
    // proofs of every port, each naming the port that gave it.
    let devices: Vec<Value> = crate::detect::group_probed(&found).into_iter().map(|group| {
        let mut per_port: Vec<(usize, Vec<Value>)> = Vec::new();
        let mut unlisted: Vec<Value> = Vec::new();
        for &i in &group {
            let (mut candidates, mut more): (Vec<Value>, Vec<Value>) =
                std::mem::take(&mut matched[i]).into_iter().partition(|c| c["kind"] == "candidate");
            if found[i].client_kind == crate::detect::ClientKind::User {
                candidates = software_cards_only(index, candidates);
                more.clear();
            }
            for u in more { if !unlisted.contains(&u) { unlisted.push(u); } }
            per_port.push((i, candidates));
        }
        let mut candidates = merge_candidates(&found, per_port);
        for c in candidates.iter_mut() {
            c["device"] = c["id"].clone();
            if let Some(e) = c["device"].as_str().and_then(|id| index.find(id)) {
                c["file"] = json!(e.path.display().to_string());
            }
        }
        // The port that carries the answer: the one of the first strong proof of the first
        // candidate, else the first port of the device.
        let proof_port = candidates.first().and_then(|c| c["proofs"].as_array()
            .and_then(|ps| ps.iter().find(|p| p["nature"] == "strong").and_then(|p| p["port"].as_str())));
        let main = proof_port.and_then(|n| group.iter().copied().find(|&i| found[i].label() == n)).unwrap_or(group[0]);
        let d = &found[main];
        // A run command needs a strong proof, and no other candidate of equal weight holding one.
        let driver = candidates.first().filter(|first| has_strong(first)
            && !candidates[1..].iter().any(|o| has_strong(o) && o["score"] == first["score"]))
            .and_then(|c| if by_file { c["file"].as_str() } else { c["device"].as_str() });
        let run_command = match (driver, d.out_index) {
            (Some(driver), Some(out)) => Some(format!("osc-bridge run --device {driver} --out-port {out}{}",
                d.in_index.map(|n| format!(" --in-port {n}")).unwrap_or_default())),
            _ => None,
        };
        let identity = d.identity.as_ref().or_else(|| group.iter().find_map(|&i| found[i].identity.as_ref()));
        let kind = if group.iter().any(|&i| found[i].client_kind == crate::detect::ClientKind::Kernel) {
            crate::detect::ClientKind::Kernel
        } else { d.client_kind };
        let mut device = json!({
            "port": d.label(),
            "out_port": d.out_index,
            "in_port": d.in_index,
            "ports": group.iter().map(|&i| json!({
                "port": found[i].label(), "out_port": found[i].out_index, "in_port": found[i].in_index,
            })).collect::<Vec<_>>(),
            "client_kind": kind.word(),
            "identity": identity.map(|id| json!({
                "manufacturer_id": id.manufacturer_hex(),
                "manufacturer": crate::detect::manufacturer_name(&id.manufacturer),
                "family": id.family,
                "model": id.model,
                "firmware": id.version.iter().map(u8::to_string).collect::<Vec<_>>().join("."),
            })),
            "candidates": candidates,
            "run_command": run_command,
        });
        if !unlisted.is_empty() {
            device["unlisted"] = json!(unlisted);
        }
        let mut held: Option<Vec<String>> = None;
        for sends in group.iter().filter_map(|&i| found[i].out_index.and_then(|o| plan.get(o))) {
            let h = held.get_or_insert_with(Vec::new);
            for m in sends.held.iter().map(|m| hex_bytes(m)) { if !h.contains(&m) { h.push(m); } }
        }
        if let Some(held) = held {
            device["held_back"] = json!(held);
        }
        if let Some((vendor_id, product_id)) = group.iter().find_map(|&i| found[i].usb) {
            device["usb"] = json!({ "vendor_id": vendor_id, "product_id": product_id });
        }
        device
    }).collect();
    let structured = json!({ "probed": probe, "devices": devices });
    let text = serde_json::to_string_pretty(&structured).unwrap_or_default();
    let mut out = content_structured(&text, structured);
    // Each identity set aside is told in a text block of its own, after the report: only the
    // universal request went out for it. The report's JSON is unchanged.
    if let Some(blocks) = out["content"].as_array_mut() {
        for text in fault_blocks(&faults) {
            blocks.push(json!({ "type": "text", "text": text }));
        }
    }
    Ok(out)
}

/// `detect_hardware` in learn mode: a fingerprint per device, and its measures beside it. The
/// report's text tells the person how long it listens and to move the controls.
fn learn_hardware(index: &DeviceIndex, server: Option<&crate::online::Online>, listen: std::time::Duration, share: Option<Share>) -> Result<Value, String> {
    let learned = crate::detect::learn_all(listen).map_err(|e| format!("MIDI scan: {e}"))?;
    let sharing = share.map(|s| share_learned(&s, &learned)).transpose()?;
    let service = learn_service_matches(server, &learned);
    let mut report = learn_report(&learned, index, listen, &service);
    if let Some(said) = sharing {
        report["structuredContent"]["sharing"] = json!(said);
        let text = format!("{}\nSharing: {said}", report["content"][0]["text"].as_str().unwrap_or_default());
        report["content"][0]["text"] = json!(text);
    }
    Ok(report)
}

/// What the service said of the devices of a learn mode.
#[derive(Debug, Clone)]
pub enum ServiceMatches {
    /// The copy is not linked: only the local catalogue is looked at.
    NotLinked,
    /// Linked, but the service did not answer.
    Unreachable,
    /// What it names for each device, in the order of the devices the learn mode reports: its
    /// catalogue candidates, else its devices without a card (see `port_answer`).
    Answered(Vec<Vec<Value>>),
}

/// The devices the learn mode reports: the ports of the system are no device.
fn learn_devices(learned: &[(crate::detect::Unit, crate::detect::Learned)]) -> Vec<&(crate::detect::Unit, crate::detect::Learned)> {
    learned.iter().filter(|(u, _)| !crate::detect::is_system_unit(u)).collect()
}

/// Asks the service to match the ports and identity reply of each device of `learned` (the
/// system ports left out). Never fails: an unreachable service is `Unreachable`.
pub fn learn_service_matches(server: Option<&crate::online::Online>, learned: &[(crate::detect::Unit, crate::detect::Learned)]) -> ServiceMatches {
    let Some(server) = server else { return ServiceMatches::NotLinked };
    let devices = learn_devices(learned);
    if devices.is_empty() { return ServiceMatches::Answered(Vec::new()); }
    let ports: Vec<Value> = devices.iter().map(|(d, l)| detect_port(d.label(),
        l.fingerprint["identity_reply"].as_str().into_iter().map(str::to_string).collect(), d.usb)).collect();
    match server.detect(&ports) {
        Ok(answers) => ServiceMatches::Answered((0..devices.len()).map(|i| answers.get(i).map(port_answer).unwrap_or_default()).collect()),
        Err(_) => ServiceMatches::Unreachable,
    }
}

/// One port of the detection request: its name, its identity replies and, when read, its USB ids
/// (`usb: {vendor_id, product_id}`, numbers); a port without ids has no `usb` field.
fn detect_port(port: &str, replies_hex: Vec<String>, usb: Option<(u16, u16)>) -> Value {
    let mut p = json!({ "port": port, "replies_hex": replies_hex });
    if let Some((vendor_id, product_id)) = usb {
        p["usb"] = json!({ "vendor_id": vendor_id, "product_id": product_id });
    }
    p
}

/// What the service says of one port, as one list read by the service's own lists: each entry of
/// `candidates` (marked `kind: candidate`), carrying the confirmations and state the service gives
/// for it in `learned`; else, when there is no candidate, each entry of `unlisted` that names a
/// vendor or a model (marked `kind: unlisted`). A device has one state: a candidate hides them.
fn port_answer(answer: &Value) -> Vec<Value> {
    let list = |k: &str| answer[k].as_array().cloned().unwrap_or_default();
    let mut candidates = list("candidates");
    if candidates.is_empty() {
        return list("unlisted").into_iter().filter(|u| u["vendor"].is_string() || u["model"].is_string())
            .map(|mut u| { u["kind"] = json!("unlisted"); u }).collect();
    }
    let learned = list("learned");
    for c in candidates.iter_mut() {
        c["kind"] = json!("candidate");
        let Some(l) = learned.iter().find(|l| l["id"] == c["id"]) else { continue };
        for k in ["confirmations", "disputed", "state", "state_label"] {
            if let Some(v) = l.get(k) { c[k] = v.clone(); }
        }
    }
    candidates
}

/// The ask that ends the next step of a device nobody knows: name it, then map its controls.
const NEXT_STEP_ASK: &str = "Ask the person to name it: its vendor (the maker), its model and its firmware version; \
    share_fingerprint needs the three, none empty.";

/// What the person is asked for the firmware: the version the identity reply carries, to
/// confirm, or nothing to propose.
fn firmware_ask(proposed: Option<&str>) -> String {
    match proposed {
        Some(v) => format!(" The identity reply carries the firmware version {v}: propose it and have the person confirm it as the firmware."),
        None => " The device sent no firmware version: ask the person for the firmware.".into(),
    }
}

/// The next step of an unknown device, worded by what was looked at.
fn next_step(service: &ServiceMatches, proposed_firmware: Option<&str>) -> String {
    let ask = format!("{NEXT_STEP_ASK}{} Then map its controls one by one with report_mapping, device being its vendor/model id; a first driver built from the mappings goes to submit_driver.",
        firmware_ask(proposed_firmware));
    match service {
        ServiceMatches::Answered(_) => format!("Neither the local catalogue nor SynthHub knows this device. {ask}"),
        _ => format!("Only the local catalogue was looked at, and no driver in it matches this device. {ask}"),
    }
}

/// A device without a card in words, with the fields the service gave and no other: the maker and
/// model, then the firmware and the confirmations.
fn unlisted_text(u: &Value) -> String {
    let name = [u["vendor"].as_str(), u["model"].as_str()].into_iter().flatten().collect::<Vec<_>>().join(" ");
    let mut text = name;
    if let Some(f) = u["firmware"].as_str() { text.push_str(&format!(", firmware {f}")); }
    if let Some(n) = u["confirmations"].as_u64() { text.push_str(&format!(", {n} confirmation(s)")); }
    text
}

/// The next step of a device the service names without a card: it is proposed to the person to
/// confirm, never named again, and nothing is sent to the device on its account without consent.
fn being_confirmed_step(unlisted: &[Value]) -> String {
    let named: Vec<String> = unlisted.iter().map(unlisted_text).collect();
    let said = unlisted.iter().find_map(|u| u["state_label"].as_str()).unwrap_or("being confirmed");
    format!("SynthHub names this device without a card: {}; {said}. Ask the person to confirm which it is, and do not ask them to name it again. \
        Nothing is sent to the device on this account without the person's consent.", named.join(" or "))
}

/// The next step once a fingerprint is shared: the mappings go to `report_mapping` under the
/// device id `vendor/model`, the firmware not being part of it.
pub fn shared_next_step(vendor: &str, model: &str) -> String {
    format!("Map its controls one by one with report_mapping (device: {}); a first driver built from the mappings goes to submit_driver.",
        device_id(vendor, model))
}

/// The id of a device the catalogue does not describe: its vendor and model, slugged.
pub fn device_id(vendor: &str, model: &str) -> String {
    format!("{}/{}", slugify(vendor, '-'), slugify(model, '-'))
}

/// The firmware version of a universal Identity Reply (`F0 7E id 06 02 maker family model
/// version F7`: a one- or three-byte maker, two bytes of family, two of model, four of version),
/// as its bytes joined by dots; none when the reply is too short.
fn identity_firmware(hex: &str) -> Option<String> {
    let bytes: Vec<u8> = hex.split_whitespace().map(|b| u8::from_str_radix(b, 16).ok()).collect::<Option<_>>()?;
    if bytes.len() < 6 || bytes[..2] != [0xF0, 0x7E] || bytes[3..5] != [0x06, 0x02] { return None; }
    let body = bytes[5..].strip_suffix(&[0xF7]).unwrap_or(&bytes[5..]);
    let maker = if body.first() == Some(&0) { 3 } else { 1 };
    let version = body.get(maker + 4..maker + 8)?;
    Some(version.iter().map(u8::to_string).collect::<Vec<_>>().join("."))
}

/// The vendor, model and firmware of a sharing, none empty, as the person gave them; the
/// refusal names every missing one in the words of the command (`--vendor`) or the tool (`vendor`).
fn share_fields<'a>(vendor: Option<&'a str>, model: Option<&'a str>, firmware: Option<&'a str>, command: bool)
    -> Result<(&'a str, &'a str, &'a str), String> {
    let given = |v: Option<&'a str>| v.map(str::trim).filter(|v| !v.is_empty());
    let (v, m, f) = (given(vendor), given(model), given(firmware));
    if let (Some(v), Some(m), Some(f)) = (v, m, f) { return Ok((v, m, f)); }
    let missing: Vec<String> = [("vendor", v), ("model", m), ("firmware", f)].into_iter()
        .filter(|(_, x)| x.is_none()).map(|(k, _)| if command { format!("--{k}") } else { k.to_string() }).collect();
    Err(format!("nothing is sent: a fingerprint is shared with three fields, none empty; missing: {}. Ask the person.", missing.join(", ")))
}

/// A refusal of the service that names the vendor, model or firmware, in the words of the
/// command typed (`--vendor`) when `command`, else as it is (the tool's parameters are named
/// so); any other text is returned as it is.
pub fn refusal_in_words(said: &str, command: bool) -> String {
    if !command || !said.contains("device:") { return said.to_string(); }
    said.split(' ').map(|w| {
        let bare = w.trim_matches(|c: char| !c.is_ascii_alphabetic());
        if ["vendor", "model", "firmware"].contains(&bare) { w.replacen(bare, &format!("--{bare}"), 1) } else { w.to_string() }
    }).collect::<Vec<_>>().join(" ")
}

/// The learn mode's result for `learned`: the structured devices and their text. A device the
/// service or `catalogue` recognises carries its `driver` (the service's first) and no
/// `next_step`; an unknown one carries a `next_step` worded by what was looked at. A port of the
/// system is left out. Pure: no MIDI.
pub fn learn_report(learned: &[(crate::detect::Unit, crate::detect::Learned)], catalogue: &DeviceIndex, listen: std::time::Duration,
                    service: &ServiceMatches) -> Value {
    let candidates = hardware_candidates(catalogue);
    let devices: Vec<Value> = learn_devices(learned).into_iter().enumerate().map(|(at, (d, l))| {
        let replies: Vec<Vec<u8>> = l.fingerprint["identity_reply"].as_str().map(|h| {
            h.split_whitespace().filter_map(|b| u8::from_str_radix(b, 16).ok()).collect()
        }).into_iter().collect();
        // Only a strong proof names a driver; a name alone leaves the device unknown.
        let program = d.client_kind == crate::detect::ClientKind::User;
        let local = d.inputs.iter().chain(&d.outputs).filter(|_| !program)
            .find_map(|p| crate::detect::rank(p, d.usb, &replies, &candidates, 1).into_iter().find(|r| r.is_strong()))
            .map(|r| (&candidates[r.index], proofs_json(&r.proofs)));
        let named: Vec<Value> = match service {
            ServiceMatches::Answered(all) => all.get(at).cloned().unwrap_or_default(),
            _ => Vec::new(),
        };
        let named = if program { software_cards_only(catalogue, named) } else { named };
        let first = named.first();
        let remote = first.and_then(|c| {
            let mut driver = json!({ "id": c["id"].as_str()?, "name": c["name"].as_str().unwrap_or_default() });
            for k in ["confirmations", "disputed", "state", "state_label"] {
                if let Some(v) = c.get(k) { driver[k] = v.clone(); }
            }
            Some(driver)
        });
        let unlisted: Vec<Value> = if remote.is_none() { named.iter().filter(|u| u["kind"] == "unlisted").cloned().collect() } else { Vec::new() };
        // The local driver is the one its slug resolves to (the most trustworthy file of the
        // device), not a firmware variant that only ranked higher by recognising the port.
        let local = local.map(|(c, proofs)| catalogue.find(&c.slug).map(|e| json!({ "id": e.id, "name": e.name, "proofs": proofs }))
            .unwrap_or_else(|| json!({ "id": c.id, "name": c.name, "proofs": proofs })));
        let driver = remote.or(local);
        let mut out = json!({
            "port": d.label(),
            "inputs": d.inputs,
            "outputs": d.outputs,
            "fingerprint": l.fingerprint,
            "measured": l.measured,
        });
        match driver {
            Some(driver) => out["driver"] = driver,
            None if !unlisted.is_empty() => {
                out["next_step"] = json!(being_confirmed_step(&unlisted));
                out["unlisted"] = json!(unlisted);
            }
            None => {
                let proposed = l.fingerprint["identity_reply"].as_str().and_then(identity_firmware);
                if let Some(v) = &proposed { out["proposed_firmware"] = json!(v); }
                out["next_step"] = json!(next_step(service, proposed.as_deref()));
            }
        }
        out
    }).collect();
    let mut notice = crate::detect::learn_notice(listen);
    if matches!(service, ServiceMatches::Unreachable) {
        notice.push_str(" The SynthHub service did not answer: only the local catalogue was looked at.");
    }
    let structured = json!({ "probed": true, "learn": true, "listen_ms": listen.as_millis() as u64,
        "notice": notice, "devices": devices });
    let text = format!("{notice}\n{}", serde_json::to_string_pretty(&structured).unwrap_or_default());
    content_structured(&text, structured)
}

/// The fingerprint to share, as `detect_hardware` is told: the port, the name the person gave
/// and the link and private list that decide whether anything goes out.
const SENT_FIELDS: &str = "sent: the vendor, model and firmware, USB vendor and product ids, identity_reply, ports, what it sends, its controls; no serial number, host name or measure";

struct Share<'a> {
    port: &'a str,
    vendor: &'a str,
    model: &'a str,
    firmware: &'a str,
    /// Typed on the command line: a refusal is worded with its flags.
    command: bool,
    client: Option<&'a crate::online::Online>,
    private: &'a [String],
}

/// Shares the fingerprint of the one device `share.port` names, and says what happened. A
/// failure of the service is said here and on the standard error, and the detection goes on.
fn share_learned(share: &Share, learned: &[(crate::detect::Unit, crate::detect::Learned)]) -> Result<String, String> {
    let wanted = share.port.to_lowercase();
    let found: Vec<_> = learned.iter().filter(|(u, _)| u.inputs.iter().chain(&u.outputs).any(|p| p.to_lowercase().contains(&wanted))).collect();
    let [(unit, one)] = found.as_slice() else {
        return Err(format!("share_fingerprint {:?} names {} device(s): name exactly one port", share.port, found.len()));
    };
    let name = format!("{} {}", share.vendor, share.model);
    let id = device_id(share.vendor, share.model);
    if crate::online::Settings::covers(share.private, &id, &id) {
        return Ok(format!("not sent: {name} is private on this machine"));
    }
    let (Some(client), Some(file)) = (share.client, crate::online::settings_file()) else {
        return Ok("not sent: this copy is not linked to the SynthHub server".into());
    };
    Ok(match client.share_fingerprint(&file, share.vendor, share.model, share.firmware, &one.fingerprint) {
        crate::online::Sharing::Sent => format!("the fingerprint of {name} (firmware {}, {}) was sent to SynthHub ({}). {}",
            share.firmware, unit.label(), SENT_FIELDS, shared_next_step(share.vendor, share.model)),
        crate::online::Sharing::Withheld(why) => format!("not sent: {why}"),
        crate::online::Sharing::Failed(why) => {
            let why = refusal_in_words(&why, share.command);
            eprintln!("osc-bridge: the fingerprint of {name} was not sent, it stays on this machine: {why}");
            format!("not sent, it stays on this machine: {why}")
        }
    })
}

/// The driver of the device a preset tool is told to talk to; a device the
/// catalogue does not know is refused, with the invitation to describe it.
fn preset_device(index: &DeviceIndex, wanted: &str) -> Result<Device, String> {
    let entry = index.find(wanted).ok_or_else(|| format!(
        "no driver '{wanted}' in the catalogue (see list_devices): describe the device first, so osc-bridge knows its banks and sounds"))?;
    crate::device::load(&entry.path).map_err(|e| format!("load {}: {e}", entry.path.display()))
}

/// `install_export`: the export of `source` put on `target` by the way its
/// driver declares (see `crate::install`).
fn install_export(source: &Device, target: &Device, device: &str, on: &str, confirm: bool, open: crate::take::Opener) -> Result<Value, String> {
    let mut midi = open().map_err(|e| format!("{} cannot be reached (MIDI system): {e}", crate::export::device_title(target)))?;
    let done = crate::install::install(source, target, confirm, midi.as_mut(),&mut crate::programs::SystemClock)?;
    let lost: Vec<Value> = done.lost.iter().map(|l| json!({ "place": l.place, "reason": l.reason })).collect();
    let text = if done.written {
        format!("Wrote {} ({} bytes, {}) on {on}: {} message(s) sent.", done.path, done.size, done.format, done.messages)
    } else {
        format!("Nothing sent. With confirm_overwrite: true it would write {} ({} bytes, {}) on {on}, by {}; it may overwrite a file there.",
            done.path, done.size, done.format, done.way)
    };
    let structured = json!({ "device": device, "on": on, "format": done.format, "way": done.way, "path": done.path,
        "size": done.size, "lost": lost, "written": done.written, "messages": done.messages });
    Ok(content_structured(&text, structured))
}

fn backup_presets(dev: &Device, in_port: &str, file: &Path, out_port: Option<&str>, max_secs: u64) -> Result<Value, String> {
    use crate::librarian::{backup_from_device, write_syx};
    let cap = backup_from_device(dev, in_port, out_port, Duration::from_secs(3), Duration::from_secs(max_secs)).map_err(|e| e.to_string())?;
    if cap.messages.is_empty() {
        return Err(format!("no sound of {} received within {max_secs} s — start the dump from the device's menu, or pass out_port to send the driver's request", dev.device.name));
    }
    write_syx(file, &cap.messages).map_err(|e| e.to_string())?;
    let structured = json!({ "file": file.display().to_string(), "messages": cap.messages.len(), "bytes": cap.bytes });
    Ok(content_structured(&format!("Saved {} SysEx message(s), {} bytes, to {}", cap.messages.len(), cap.bytes, file.display()), structured))
}

fn restore_presets(dev: &Device, out_port: &str, file: &Path, target: Option<(&str, i64)>) -> Result<Value, String> {
    let sent = crate::librarian::restore_to_device(dev, out_port, file, target).map_err(|e| format!("{e:#}"))?;
    let bytes = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    let structured = json!({ "file": file.display().to_string(), "messages": sent, "bytes": bytes });
    Ok(content_structured(&format!("Sent {sent} SysEx message(s), {bytes} bytes, from {}", file.display()), structured))
}

/// A shared preset's files on this machine, read: `file` (a .syx) sent as
/// `hex`, `midi_file` as `midi_base64`; `audio_file` read for the upload
/// that follows the call.
fn read_local_files(args: &Value) -> Result<(Value, Option<Vec<u8>>), String> {
    use base64::Engine;
    let mut sent = args.clone();
    let Some(o) = sent.as_object_mut() else { return Ok((sent, None)) };
    if let Some(file) = o.remove("file") {
        if o.contains_key("hex") {
            return Err("give hex or file, not both".into());
        }
        let msgs = crate::librarian::read_syx(Path::new(file.as_str().unwrap_or_default())).map_err(|e| e.to_string())?;
        o.insert("hex".into(), json!(msgs.iter().flatten().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")));
    }
    if let Some(file) = o.remove("midi_file") {
        let path = file.as_str().unwrap_or_default();
        let bytes = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
        o.insert("midi_base64".into(), json!(base64::engine::general_purpose::STANDARD.encode(bytes)));
    }
    let audio = match o.remove("audio_file") {
        Some(file) => {
            let path = file.as_str().unwrap_or_default();
            Some(std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?)
        }
        None => None,
    };
    Ok((sent, audio))
}

/// Share or update a preset with its files on this machine, then upload its
/// audio preview, if any.
fn share_with_files(link: &Link, name: &str, args: &Value) -> Result<Value, String> {
    let (sent, audio) = read_local_files(args)?;
    let mut result = link.forward(name, &sent)?;
    if let Some(bytes) = audio {
        let id = result["structuredContent"]["id"].as_str().or(args["id"].as_str()).unwrap_or_default().to_string();
        let user = link.user(false).map_err(|e| e.to_string())?;
        let note = match link.client.upload(&user, &id, "audio", &bytes) {
            Ok(()) => "Audio preview attached.".to_string(),
            Err(e) => format!("The audio preview was not attached: {e}"),
        };
        if let Some(text) = result.pointer_mut("/content/0/text") {
            *text = json!(format!("{} {note}", text.as_str().unwrap_or_default()));
        }
    }
    Ok(result)
}

/// The synths the server's librarian knows, and one librarian step, for
/// `crate::take`.
fn librarian_ask(link: &Link) -> impl Fn(&str, &Value) -> Result<Value, String> + '_ {
    move |op, body| link.client.librarian(op, body).map_err(|e| format!("SynthHub's librarian: {e}"))
}

/// Take the sound a synth plays (or a program) through the server's
/// librarian, saved as a .syx file for share_preset.
fn take_sound(args: &Value, link: &Link, open: crate::take::Opener) -> Result<Value, String> {
    use crate::take::{find, take};
    let lib = link.client.librarian_synths().map_err(|e| format!("SynthHub's librarian could not be reached: {e}"))?;
    let ask = librarian_ask(link);
    let mut midi = open().map_err(|e| e.to_string())?;
    let found = find(midi.as_mut(), &lib, &ask, args["port"].as_str())?;
    let f = match found.as_slice() {
        [] => return Err("no synth the librarian knows answered: check that it is on, plugged in, and that SysEx is \
            allowed in its MIDI settings. For another synth, back its sounds up with backup_presets.".into()),
        [f] => f,
        several => return Err(format!("several synths answered — call again with port: {}",
            several.iter().map(|f| format!("{} on {:?}", f.name, f.port)).collect::<Vec<_>>().join(", "))),
    };
    let program = args["program"].as_u64().map(|p| p.saturating_sub(1));
    let taken = take(midi.as_mut(), &ask, f, program, Duration::from_secs(10))?;
    drop(midi);
    let file = match args["file"].as_str() {
        Some(p) => PathBuf::from(p),
        // In the temporary folder, under a name of its own: kept, since it is the user's sound.
        None => tempfile::Builder::new()
            .prefix(&format!("{}-{}-", slugify(&f.name, '-'), slugify(if taken.name.is_empty() { "sound" } else { &taken.name }, '-')))
            .suffix(".syx")
            .tempfile()
            .and_then(|t| t.keep().map_err(|e| e.error))
            .map_err(|e| e.to_string())?
            .1,
    };
    crate::librarian::write_syx(&file, &taken.messages).map_err(|e| e.to_string())?;
    let synth = lib["synths"].as_array().into_iter().flatten().find(|s| s["synth"] == f.synth.as_str());
    let note = synth.map(|s| s["note"].clone()).filter(|n| !n.is_null());
    let bytes: usize = taken.messages.iter().map(Vec::len).sum();
    let structured = json!({ "synth": f.name, "librarian": f.synth, "device": f.devices.first(), "name": taken.name,
        "firmware": f.firmware, "port": f.port, "file": file.display().to_string(), "messages": taken.messages.len(),
        "bytes": bytes, "note": note });
    let share = match f.devices.first() {
        Some(_) => "Ask the user whether they made it; if so, share_preset with device, name, file, librarian, firmware and own_sound: true.",
        None => "The catalogue has no driver for this synth yet, so it cannot be shared.",
    };
    Ok(content_structured(&format!("Took {:?} from {} on {} ({} message(s), {bytes} bytes), saved to {}. {share}{}",
        taken.name, f.name, f.port, taken.messages.len(), file.display(),
        note.as_ref().and_then(Value::as_str).map(|n| format!(" Note: {n}")).unwrap_or_default()), structured))
}

/// Fetch a shared preset's messages from the server and play them into a
/// local device (patch data only once the user agreed: it replaces what the
/// device holds). A sound the librarian took is fitted to the receiving
/// synth's MIDI channel, read from its answer to the detect messages.
fn load_preset(args: &Value, link: &Link, open: crate::take::Opener) -> Result<Value, String> {
    let id = args.get("id").and_then(Value::as_str).ok_or("missing id")?;
    let out_port = args.get("out_port").and_then(Value::as_str).ok_or("missing out_port")?;
    let gap = Duration::from_millis(args.get("gap_ms").and_then(Value::as_u64).unwrap_or(50).min(2000));
    let preset = link.forward("get_preset", &json!({ "id": id }))?["structuredContent"].clone();
    let mut note = None;
    let channel = match preset["librarian"].as_str().and_then(|l| l.split('@').next()) {
        Some(synth) => {
            let mut lib = link.client.librarian_synths().map_err(|e| format!("SynthHub's librarian could not be reached: {e}"))?;
            if let Some(all) = lib["synths"].as_array_mut() { all.retain(|s| s["synth"] == synth); }
            note = lib["synths"][0]["note"].as_str().map(str::to_string);
            let mut midi = open().map_err(|e| e.to_string())?;
            let found = crate::take::find(midi.as_mut(), &lib, &librarian_ask(link), Some(out_port))?;
            Some(found.iter().find(|f| f.synth == synth).map(|f| f.channel)
                .ok_or_else(|| format!("no {synth} answered on {out_port}: is it the synth this sound was taken from, on and plugged in?"))?)
        }
        None => None,
    };
    let r = link.client.preset_messages(id, channel).map_err(|e| e.to_string())?;
    let messages: Vec<Vec<u8>> = r["messages"].as_array().into_iter().flatten().filter_map(Value::as_str)
        .map(|h| crate::librarian::parse_hex(h).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
    if messages.is_empty() {
        return Err(format!("{id} is a MIDI clip: play its MIDI file from a sequencer (get_preset gives it)"));
    }
    if r["device_data"] == json!(true) && args.get("confirm_overwrite").and_then(Value::as_bool) != Some(true) {
        return Err(format!("{id} carries patch data, which replaces the sound the device holds: ask the user, \
            then pass confirm_overwrite: true"));
    }
    crate::librarian::send_messages(out_port, &messages, gap).map_err(|e| e.to_string())?;
    let structured = json!({ "id": id, "name": preset["name"], "device": preset["device"], "messages": messages.len(),
        "attribution": preset["attribution"], "note": note });
    Ok(content_structured(&format!("Loaded {} ({} message(s)) on {out_port}.{} Tell SynthHub how it went with report_preset.",
        preset["name"].as_str().unwrap_or(id), messages.len(), note.map(|n| format!(" Note: {n}")).unwrap_or_default()), structured))
}

/// `osc-bridge detect --learn`: the notice first (the person moves the controls while it
/// listens), then the fingerprint of each port and its measures.
/// With `share` (the port, then the vendor, model and firmware the person gives), that
/// fingerprint is sent to SynthHub when linked and not private; naming the port is the person's
/// agreement. A missing vendor, model or firmware is refused before anything is listened to.
pub fn print_learning(listen_ms: Option<u64>, share: Option<(&str, [Option<&str>; 3])>) -> Result<()> {
    let share = share.map(|(port, [vendor, model, firmware])| {
        share_fields(vendor, model, firmware, true).map(|(vendor, model, firmware)| (port, vendor, model, firmware))
    }).transpose().map_err(|e| anyhow::anyhow!(e))?;
    let listen = crate::detect::learn_listen(listen_ms).map_err(|e| anyhow::anyhow!(e))?;
    println!("{}", crate::detect::learn_notice(listen));
    let client = crate::online::linked().then(|| crate::online::Online::new(crate::online::SERVER));
    let private = crate::online::settings_file().and_then(|f| crate::online::Settings::load(Some(&f)).ok()).map(|s| s.private).unwrap_or_default();
    let share = share.map(|(port, vendor, model, firmware)| Share { port, vendor, model, firmware, command: true, client: client.as_ref(), private: &private });
    let catalogue = DeviceIndex::build(&crate::online::cache_dir().unwrap_or_default());
    let report = learn_hardware(&catalogue, client.as_ref(), listen, share).map_err(|e| anyhow::anyhow!(e))?;
    if let Some(said) = report["structuredContent"]["sharing"].as_str() { println!("Sharing: {said}"); }
    let devices = report["structuredContent"]["devices"].as_array().cloned().unwrap_or_default();
    if devices.is_empty() {
        println!("No MIDI port found.");
    }
    for d in devices {
        let names = |k: &str| d[k].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
            .filter(|s| !s.is_empty()).unwrap_or("-".into());
        println!("{}  (out {}; in {})", d["port"].as_str().unwrap_or("?"), names("outputs"), names("inputs"));
        println!("  fingerprint: {}", serde_json::to_string_pretty(&d["fingerprint"]).unwrap_or_default().replace('\n', "\n  "));
        println!("  measured: {}", d["measured"]);
        if let Some(step) = d["next_step"].as_str() { println!("  next step: {step}"); }
    }
    Ok(())
}

/// `osc-bridge detect`: the same report as the `detect_hardware` tool, for a
/// terminal: drivers of `devices_dir`, else of the SynthHub catalogue when
/// linked, else those cached on this machine.
///
/// With `write`, the observation is also written to that file as a capture. With `read`, the
/// capture of that file is replayed instead of scanning: no device, nothing sent; it refuses
/// `probe` off and `write`, a replay sending and writing nothing.
pub fn print_detection(devices_dir: Option<&Path>, probe: bool, write: Option<&Path>, read: Option<&Path>, consent: &[String]) -> Result<()> {
    let root = devices_dir.map(Path::to_path_buf).or_else(crate::online::cache_dir).unwrap_or_default();
    let devices = if let Some(read) = read {
        if !probe { anyhow::bail!("--read cannot be used with --no-probe: a replay sends nothing"); }
        if write.is_some() { anyhow::bail!("--read cannot be used with --write: a replay writes no capture"); }
        if !consent.is_empty() { anyhow::bail!("--read cannot be used with --consent: a replay sends nothing"); }
        let capture = crate::capture::Capture::read(read)?;
        detect_capture(&root, &capture).map_err(|e| anyhow::anyhow!(e))?
    } else {
        let index = DeviceIndex::build(&root);
        let server = (devices_dir.is_none() && crate::online::linked()).then(|| crate::online::Online::new(crate::online::SERVER));
        let report = detect_hardware(&index, server.as_ref(), devices_dir.is_some(), probe, std::time::Duration::from_millis(800), write, consent)
            .map_err(|e| anyhow::anyhow!(e))?;
        report["structuredContent"]["devices"].as_array().cloned().unwrap_or_default()
    };
    print!("{}", format_detection(&devices, probe));
    Ok(())
}

/// The text of a detection report: one block per port, `usb: vvvv:pppp`
/// (as `lsusb` writes it) right under the port when its ids are known.
fn format_detection(devices: &[Value], probe: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if devices.is_empty() {
        out.push_str("No MIDI port found.\n");
    }
    for d in devices {
        let _ = writeln!(out, "{}  (out {}, in {})", d["port"].as_str().unwrap_or("?"),
            d["out_port"].as_u64().map(|n| n.to_string()).unwrap_or("-".into()),
            d["in_port"].as_u64().map(|n| n.to_string()).unwrap_or("-".into()));
        // A device of several ports says which of them this block names.
        if let Some(ports) = d["ports"].as_array().filter(|p| p.len() > 1) {
            let name = d["port"].as_str().unwrap_or("?");
            if let Some(n) = ports.iter().position(|p| p["port"] == d["port"]) {
                let _ = writeln!(out, "  port {} of {}", n + 1, name.split(':').next().unwrap_or(name));
            }
        }
        let id16 = |k: &str| d["usb"][k].as_u64().and_then(|n| u16::try_from(n).ok());
        if let (Some(v), Some(p)) = (id16("vendor_id"), id16("product_id")) {
            let _ = writeln!(out, "  usb: {v:04x}:{p:04x}");
        }
        if let Some(held) = d["held_back"].as_array().filter(|h| !h.is_empty()) {
            let _ = writeln!(out, "  held back (no consent, see --consent): {} maker request(s)", held.len());
        }
        match d["identity"].as_object() {
            Some(id) => {
                let _ = writeln!(out, "  identity: {} [{}], family {}, model {}, firmware {}",
                    id["manufacturer"].as_str().unwrap_or("unknown manufacturer"),
                    id["manufacturer_id"].as_str().unwrap_or(""), id["family"], id["model"],
                    id["firmware"].as_str().unwrap_or(""));
            }
            None if probe => out.push_str("  identity: no reply\n"),
            None => {}
        }
        for c in d["candidates"].as_array().into_iter().flatten() {
            let fw = c["firmware"].as_str().map(|f| format!(", firmware {f}")).unwrap_or_default();
            let state = c["state_label"].as_str().map(|l| format!(" — {l}")).unwrap_or_default();
            let id = c["device"].as_str().or(c["id"].as_str()).unwrap_or("");
            let name = c["name"].as_str().map(|n| format!("{n} — ")).unwrap_or_default();
            let _ = writeln!(out, "  match: {name}{id} — {} ({}, score {}{fw}){state}",
                c["file"].as_str().unwrap_or(id), c["source_tier"].as_str().unwrap_or(""), c["score"]);
            let because: Vec<String> = c["proofs"].as_array().into_iter().flatten()
                .map(|p| format!("{} ({})", p["kind"].as_str().unwrap_or("?"), p["nature"].as_str().unwrap_or("?"))).collect();
            if !because.is_empty() {
                let _ = writeln!(out, "    because: {}", because.join(", "));
            }
        }
        if d["candidates"].as_array().is_none_or(|c| c.is_empty()) && d["unlisted"].as_array().is_none_or(|u| u.is_empty()) {
            out.push_str("  unknown: nothing proves which device it is; `osc-bridge detect --learn` learns it\n");
        }
        for u in d["unlisted"].as_array().into_iter().flatten() {
            let said = u["state_label"].as_str().map(str::to_string)
                .unwrap_or_else(|| if u["disputed"] == true { "disputed".into() } else { "named by the service".into() });
            if !(u["vendor"].is_string() || u["model"].is_string()) { continue; }
            let _ = writeln!(out, "  unlisted: {} — {said}", unlisted_text(u));
        }
        if let Some(cmd) = d["run_command"].as_str() {
            let _ = writeln!(out, "  run:   {cmd}");
        }
    }
    out
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
        "device": e.id,
        "usage_status": status(&e.id),
        "name": e.name,
        "vendor": e.vendor,
        "osc_prefix": e.slug,
        "kind": e.kind,
        "revision": e.revision,
        "source_tier": e.tier,
        "firmware": e.firmware,
        "loads_sounds_of": e.loads_sounds_of,
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
    dev.commands.iter().flatten().map(|c| crate::device::address_text(&c.osc)).chain(cc).chain(params).chain(triggers)
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
    let path = index.find(slug).map(|e| e.path.as_path()).or_else(|| index.find_path(slug))
        .ok_or_else(|| format!("no device '{slug}' (see list_devices)"))?;
    crate::device::load_with_docs(path).map_err(|e| format!("load {}: {e}", path.display()))
}

fn export_device(index: &DeviceIndex, args: &Value) -> Result<Value, String> {
    let wanted = device_arg(args)?;
    let format: crate::export::Format = args.get("format").and_then(Value::as_str).ok_or("missing format")?
        .parse().map_err(|e: anyhow::Error| e.to_string())?;
    let entry = index.find(wanted).ok_or_else(|| format!("no driver '{wanted}' (see list_devices)"))?;
    let dev = crate::device::load(&entry.path).map_err(|e| format!("load {}: {e}", entry.path.display()))?;
    let out = crate::export::export(&dev, format).map_err(|e| e.to_string())?;
    let document = out.value;
    let file_name = format!("{}.{}", entry.id.replace('/', "_"), format.extension());
    let lost: Vec<Value> = out.lost.iter().map(|l| json!({ "place": l.place, "reason": l.reason })).collect();
    let structured = json!({ "device": entry.id, "format": args["format"], "media_type": format.media_type(),
        "file_name": file_name, "bytes": document.len(), "lost": lost });
    Ok(content_structured(&document, structured))
}

fn get_device_docs(index: &DeviceIndex, slug: &str) -> Result<Value, String> {
    let dev = load_device_by_slug(index, slug)?;
    let (has_docs, markdown) = match crate::device::attached_docs(&dev) {
        Some(md) => (true, md),
        None => (false, format!("No companion .md found for device '{slug}'.")),
    };
    let structured = json!({ "device": slug, "has_docs": has_docs, "markdown": markdown });
    Ok(content_structured(&markdown, structured))
}

/// A bank as `list_routes` states it: its numbering always, the rest when the driver gives it.
fn bank_json(b: &osd::Bank) -> Value {
    let mut o = json!({
        "name": b.name,
        "first_pc": b.first_pc.unwrap_or(0),
        "size": b.size,
        "first_number": b.first_number.unwrap_or(1),
    });
    for (k, v) in [("msb", b.msb), ("lsb", b.lsb), ("channel", b.channel)] {
        if let Some(v) = v { o[k] = json!(v); }
    }
    for (k, v) in [("display", &b.display), ("selects", &b.selects)] {
        if let Some(v) = v { o[k] = json!(v); }
    }
    o
}

fn list_routes(index: &DeviceIndex, slug: &str) -> Result<Value, String> {
    let dev = load_device_by_slug(index, slug)?;
    let prefix = dev.device.osc_prefix.clone();
    let mut cmds: Vec<Value> = Vec::new();
    for c in dev.commands.iter().flatten() {
        let mode = if c.forward.is_some() { "forward (OSC)" } else { "frame (MIDI/SysEx)" };
        cmds.push(json!({
            "address": format!("{prefix}{}", crate::device::address_text(&c.osc)),
            "args": c.args.iter().flatten().map(|a| json!({"name": a.name, "type": crate::device::arg_type_name(a.ty)})).collect::<Vec<_>>(),
            "mode": mode,
        }));
    }
    let expects = |e: &Option<String>| e.clone().unwrap_or_else(|| "continuous".into());
    let range = |r: Option<[i64; 2]>| r.unwrap_or([0, 127]);
    let cc_params: Vec<Value> = dev.cc_params.as_ref().map(|t| t.entries.iter().map(|e| json!({
        "address": format!("{prefix}{}", e.osc),
        "cc": e.cc,
        "cc_lsb": e.cc_lsb,
        "nrpn_msb": e.nrpn_msb,
        "nrpn_lsb": e.nrpn_lsb,
        "nrpn_lsb_range": e.nrpn_lsb_range,
        "rpn_msb": e.rpn_msb,
        "rpn_lsb": e.rpn_lsb,
        "range": range(e.range),
        "expects": expects(&e.expects),
        "usage": e.remarks.get("_usage").and_then(Value::as_str),
    })).collect()).unwrap_or_default();
    let params: Vec<Value> = dev.params.as_ref().map(|t| t.entries.iter().map(|e| json!({
        "address": format!("{prefix}{}", e.osc),
        "range": range(e.range),
        "expects": expects(&e.expects),
    })).collect()).unwrap_or_default();
    let triggers: Vec<Value> = dev.triggers.as_ref().map(|t| t.entries.iter().map(|e| json!({
        "address": format!("{prefix}{}", e.osc),
        "sound": e.sound,
        "section": e.section,
        "channel": e.channel,
        "channel_mode": crate::device::channel_mode_name(e.channel_mode),
        "note_range": e.note_range,
        "note": e.note,
        "velocity_usage": e.velocity_usage,
    })).collect()).unwrap_or_default();
    let replies: Vec<Value> = dev.replies.iter().flatten().map(|r| json!({
        "match_osc": r.match_osc.as_ref().map(crate::device::address_text),
        "match_frame_len": r.match_frame.as_ref().map_or(0, Vec::len),
        "emit_osc": crate::device::emit_text(r),
    })).collect();
    let identity = (dev.device.transport.as_ref().is_none_or(|t| t.kind != osd::TransportKind::Osc)).then(|| json!({
        "address": format!("{prefix}{}", crate::programs::IDENTITY),
        "reply_address": format!("{prefix}{} <firmware> <hex>", crate::programs::IDENTITY_REPLY),
        "profile": dev.identity.as_ref().and_then(|i| i.profile.clone()),
    }));
    let programs = dev.programs.as_ref().map(|p| json!({
        "address": (!crate::device::kept_list(&p.banks).is_empty()).then(|| format!("{prefix}{} <bank> <number> [channel]", crate::programs::RECALL)),
        "dump_address": crate::device::kept(&p.dump).filter(|d| d.request.is_some() || d.script.is_some())
            .map(|_| format!("{prefix}{} [<bank> <number>] [channel]", crate::programs::DUMP)),
        "selects": crate::device::kept(&p.selects),
        "banks": crate::device::kept_list(&p.banks).iter().map(bank_json).collect::<Vec<_>>(),
        "requires": crate::device::kept_list(&p.requires),
        "dump": crate::device::kept(&p.dump).map(|d| crate::device::dump_kind_name(d.kind)),
    }));
    let payload = json!({
        "device": dev.device.name,
        "vendor": dev.device.vendor.clone().unwrap_or_default(),
        "osc_prefix": prefix,
        "kind": dev.device.kind.map(|k| crate::device::kind_name(Some(k))),
        "identity": identity,
        "programs": programs,
        "commands": cmds,
        "cc_params": cc_params,
        "params": params,
        "triggers": triggers,
        "replies": replies,
    });
    let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
    Ok(content_structured(&text, payload))
}

/// How long `send` waits for osc-bridge to answer before saying it does not: a guard for a
/// bridge that is not there, never the time of a message.
const SEND_ANSWER_GUARD: Duration = Duration::from_secs(3);

/// Send one message in a packet with a `/bridge/sync <id>` and hand back at the first answer:
/// `/bridge/fault <address> <reason>` is an error result, `/bridge/done <id>` is `sent: true`.
fn send_osc_message(addr: &str, args: &Value, target: SocketAddr) -> Result<Value, String> {
    use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};
    let osc_args: Vec<OscType> = args.as_array().map(|a| a.iter().map(json_to_osc).collect()).unwrap_or_default();
    let id = format!("send-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    let pkt = OscPacket::Bundle(OscBundle {
        timetag: OscTime { seconds: 0, fractional: 1 },
        content: vec![
            OscPacket::Message(OscMessage { addr: addr.to_string(), args: osc_args }),
            OscPacket::Message(OscMessage { addr: "/bridge/sync".into(), args: vec![OscType::String(id.clone())] }),
        ],
    });
    let bytes = encoder::encode(&pkt).map_err(|e| format!("osc encode: {e:?}"))?;
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.send_to(&bytes, target).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + SEND_ANSWER_GUARD;
    let mut buf = [0u8; 65536];
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let silent = || format!(
            "osc-bridge does not answer at {target}: {addr} may not have been handled. Is osc-bridge running on that address?");
        if left.is_zero() { return Err(silent()); }
        sock.set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
        let Ok((n, _)) = sock.recv_from(&mut buf) else { return Err(silent()); };
        let Ok((_, pkt)) = rosc::decoder::decode_udp(&buf[..n]) else { continue };
        let mut answers = Vec::new();
        flatten_messages(pkt, &mut answers);
        for m in answers {
            let strings: Vec<String> = m.args.iter().filter_map(|a| match a { OscType::String(s) => Some(s.clone()), _ => None }).collect();
            if m.addr == "/bridge/fault" && strings.len() == 2 {
                let structured = json!({
                    "sent": false,
                    "address": strings[0],
                    "reason": strings[1],
                    "bytes": bytes.len(),
                    "target": target.to_string(),
                });
                let mut res = content_structured(&format!("Not sent: {} — {}", strings[0], strings[1]), structured);
                res["isError"] = json!(true);
                return Ok(res);
            }
            if m.addr == "/bridge/done" && m.args.first() == Some(&OscType::String(id.clone())) {
                let structured = json!({
                    "sent": true,
                    "address": addr,
                    "bytes": bytes.len(),
                    "target": target.to_string(),
                });
                return Ok(content_structured(&format!("Sent {addr} ({} bytes) to {target}.", bytes.len()), structured));
            }
        }
    }
}

fn flatten_messages(pkt: rosc::OscPacket, out: &mut Vec<rosc::OscMessage>) {
    match pkt {
        rosc::OscPacket::Message(m) => out.push(m),
        rosc::OscPacket::Bundle(b) => for p in b.content { flatten_messages(p, out); },
    }
}

fn get_status(target: SocketAddr) -> Result<Value, String> {
    ask_orchestrator("/bridge/status", vec![], target)
}

/// `add_link`, `remove_link`, `list_links`: one control address of the running
/// orchestrator, asked like `/bridge/status`.
fn link_tool(name: &str, args: &Value, target: SocketAddr) -> Result<Value, String> {
    use rosc::OscType;
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_string)
        .ok_or_else(|| format!("missing {key}"));
    match name {
        "list_links" => ask_orchestrator("/bridge/link/list", vec![], target),
        "remove_link" => {
            let osc_args = vec![OscType::String(text("from")?), OscType::String(text("to")?)];
            ask_orchestrator("/bridge/link/remove", osc_args, target)
        }
        _ => {
            let mut osc_args = vec![OscType::String(text("from")?), OscType::String(text("to")?)];
            if let Some(map) = args.get("map") {
                let range = |key: &str| -> Result<[f32; 2], String> {
                    let a = map.get(key).and_then(Value::as_array).filter(|a| a.len() == 2)
                        .ok_or_else(|| format!("map.{key} must be [lo, hi]"))?;
                    let n = |i: usize| a[i].as_f64().map(|x| x as f32).ok_or_else(|| format!("map.{key} must be numbers"));
                    Ok([n(0)?, n(1)?])
                };
                let (from, to) = (range("from")?, range("to")?);
                osc_args.extend(from.iter().chain(to.iter()).map(|x| OscType::Float(*x)));
            }
            ask_orchestrator("/bridge/link/add", osc_args, target)
        }
    }
}

/// Send one control message to a running orchestrator and give every reply
/// that arrives within 500 ms, or say that none did.
fn ask_orchestrator(addr: &str, osc_args: Vec<rosc::OscType>, target: SocketAddr) -> Result<Value, String> {
    use rosc::{OscMessage, OscPacket, encoder};
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(Duration::from_millis(500))).map_err(|e| e.to_string())?;
    let pkt = OscPacket::Message(OscMessage { addr: addr.into(), args: osc_args });
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

/// Every `.json` file under `dir` (or `dir` itself when it is one).
fn walk_json<F: FnMut(&Path)>(dir: &Path, f: &mut F) -> std::io::Result<()> {
    if dir.is_file() {
        if dir.extension().and_then(|s| s.to_str()) == Some("json") {
            f(dir);
        }
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_json(&path, f)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
            f(&path);
        }
    }
    Ok(())
}

mod roles;

#[cfg(test)]
mod tests;
