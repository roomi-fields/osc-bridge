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
//!
//! These are the 16 local tools, answered with no service (`call_local`, which takes what a
//! service knows as values: `Known`). The tools of the SynthHub service — the reports, the
//! contributions, the shared presets, `take_sound`, `load_preset` — belong to the link, which
//! wraps this server: it answers its own tools first and hands every other request to the
//! local answer.
//!
//! The server reads one JSON-RPC request per line from stdin and writes one
//! response per line to stdout; logging goes to stderr.
//!
//! The hosted server is a separate program built on this module: it serves
//! the catalogue tools (`catalogue_tool`), the schemas of `local_tool_schemas` and
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
    /// Which of two cards of one device wins: the caller's order (`recognition::TierOrder`).
    pub tier_order: crate::recognition::TierOrder,
    /// The folder of cards the server serves, used as it is. None: an empty index.
    pub devices_dir: Option<PathBuf>,
    pub default_target: SocketAddr,
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
    /// lookup (the caller's `order`, lower first), ties broken by path so the choice is stable.
    pub fn build(dir: &Path, order: crate::recognition::TierOrder) -> Self {
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
                        (order(&tier), path) < (order(&c.tier), c.path.as_path())
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

/// Build the catalogue index once — every device tool reads from it
/// instead of re-walking the tree.
pub(crate) fn build_index(devices_dir: &Path, opts: &McpOptions) -> DeviceIndex {
    eprintln!("MCP server starting (devices_dir={}, default_target={})",
        devices_dir.display(), opts.default_target);
    let index = DeviceIndex::build(devices_dir, opts.tier_order);
    eprintln!("indexed {} device file(s)", index.entries.len());
    index
}

/// The local MCP server over stdio, its cards those of `opts.devices_dir` (an empty index
/// without it). A linked copy runs the server of the link, which wraps this one.
pub fn run(opts: McpOptions) -> Result<()> {
    serve(opts, std::io::stdin().lock(), std::io::stdout().lock())
}

/// `serve_with_midi` on the machine's MIDI system (the guarded opening).
pub fn serve(opts: McpOptions, input: impl BufRead, out: impl Write) -> Result<()> {
    serve_with_midi(opts, input, out, &crate::midi_port::open_hardware)
}

/// One local MCP session: a JSON-RPC message per line of `input`, each reply a line of `out`,
/// answered by the 16 local tools over the drivers of `opts.devices_dir`. The tools that reach
/// the MIDI system take it from `open`.
pub fn serve_with_midi(opts: McpOptions, input: impl BufRead, out: impl Write, open: crate::midi_port::Opener) -> Result<()> {
    let root = opts.devices_dir.clone().unwrap_or_default();
    serve_in(opts, &root, input, out, open)
}

fn serve_in(opts: McpOptions, root: &Path, input: impl BufRead, out: impl Write, open: crate::midi_port::Opener) -> Result<()> {
    let index = build_index(root, &opts);
    run_session(input, out, |req| Ok(handle_request_with(req, &opts, &index, open)))
}

/// A session over JSON-RPC lines: each line of `input` parsed and handed to `handle`, each reply
/// written as a line of `out`. A line that does not parse is told on the standard error and skipped.
pub fn run_session(input: impl BufRead, mut out: impl Write, mut handle: impl FnMut(&Value) -> Result<Option<Value>>) -> Result<()> {
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
        if let Some(resp) = handle(&req)? {
            let s = serde_json::to_string(&resp)?;
            writeln!(out, "{s}")?;
            out.flush()?;
        }
    }
    Ok(())
}

fn handle_request_with(req: &Value, opts: &McpOptions, index: &DeviceIndex, open: crate::midi_port::Opener) -> Option<Value> {
    let nothing = Known { trust: Value::Null, candidates: Vec::new(), status: &|_| None, private: &|_| false };
    rpc_reply(req, &tool_names(opts), || tools_list(opts), |name, args| call_local(name, args, opts, index, open, &nothing))
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

fn tools_list(_opts: &McpOptions) -> Value {
    local_tool_schemas()
}

/// The schema of the 16 local tools. The tools of the service are the link's
/// (`linked_tool_schemas` of the link).
pub fn local_tool_schemas() -> Value {
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
                "description": "Find the MIDI devices plugged into this computer and match them to catalogue drivers. Lists MIDI ports and sends each port the standard MIDI Identity Request (universal, read-only by the norm), and nothing else without consent: whatever designates the port (its USB id, its name), no maker request goes out. A maker request may be a load for another maker's device, so every one is held back, port by port, and each device lists held_back, the requests not sent. Consent adds them: pass consent_ports, the ports that may receive every maker request; the person must have been asked and agreed for that port (never pass it on your own). Reads each device's manufacturer, model and firmware. Reads the USB ids (vendor_id, product_id) of each USB device's card (Linux; absent for a virtual or DIN port). A driver whose identity block recognises the reply, the USB ids (usb) or the port name comes first, the variant for the firmware read from it above the others; of drivers matching equally, the one the others share their MIDI with comes first, and each lists the others in equally_matching. Returns, per device, the best matching drivers, each with its proofs (each proof has a nature, strong, medium or weak, and a kind: identity reply, usb id, maker, name) and its firmware when recognised. A candidate whose only proof is the name is 'possible, to confirm' and gets no command; a port with no other proof is 'unknown' and the learn mode takes over. A ready-to-run 'osc-bridge run' command with the right port numbers is given only for a candidate with a strong proof and no rival of equal weight.",
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

/// The 16 tools the local server answers.
pub(crate) fn tool_names(_opts: &McpOptions) -> Vec<&'static str> {
    vec!["list_devices", "get_device_docs", "list_routes", "export_device", "send", "get_status",
         "add_link", "remove_link", "list_links", "detect_hardware", "backup_presets", "restore_presets", "install_export",
         "list_roles", "set_by_role", "get_by_role"]
}

/// What the service knows, given to the local answer as values: the trust of every reported
/// driver, the service's answers about the ports of a detection, the status of a card, and
/// whether a device is private on this machine. Without a service, nothing is known.
// The local tools read the card status and the candidates; the trust and the private decision
// are the link's own (its private-device branches) and ride along for the tools that learn them.
#[allow(dead_code)]
pub(crate) struct Known<'a> {
    pub(crate) trust: Value,
    pub(crate) candidates: Vec<Value>,
    pub(crate) status: &'a dyn Fn(&str) -> Option<String>,
    pub(crate) private: &'a dyn Fn(&str) -> bool,
}

/// The local answer to one call of the 16 local tools, with what the service knows given as values;
/// any other tool is unknown to it.
pub(crate) fn call_local(name: &str, args: &Value, opts: &McpOptions, index: &DeviceIndex,
                         open: crate::midi_port::Opener, known: &Known) -> Result<Value, String> {
    if args.get("slug").is_some() { device_arg(args)?; }
    if let Some(done) = catalogue_tool(name, args, index, known.status) {
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
        "detect_hardware" if args.get("learn").and_then(Value::as_bool) == Some(true) =>
            learn_tool(args, index, opts, &not_linked_share, &|_| ServiceMatches::NotLinked),
        "detect_hardware" => detect_tool(args, index, opts, None, &|_| Ok(known.candidates.clone())),
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
pub(crate) fn device_arg(args: &Value) -> Result<&str, String> {
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

/// The hardware drivers of the catalogue, in the shape `recognition::rank` scores.
pub fn hardware_candidates(index: &DeviceIndex) -> Vec<crate::recognition::Candidate> {
    index.entries.iter().filter(|e| e.kind != "software").map(|e| crate::recognition::Candidate {
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
pub fn detect_probes(index: &DeviceIndex) -> Vec<crate::recognition::Probe> {
    detect_plan(index).probes
}

/// The requests of `detect_probes` with the faults of the identities set aside.
fn detect_plan(index: &DeviceIndex) -> crate::recognition::Probes {
    crate::recognition::probes(index.entries.iter().filter(|e| e.kind != "software")
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
pub fn probes_json(probes: &[crate::recognition::Probe]) -> Value {
    let one = |p: &crate::recognition::Probe| {
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
fn proofs_json(proofs: &[crate::recognition::Proof]) -> Value {
    json!(proofs.iter().map(|p| json!({ "nature": p.nature.as_str(), "kind": p.kind })).collect::<Vec<_>>())
}

/// Whether a published candidate holds a strong proof.
fn has_strong(c: &Value) -> bool {
    c["proofs"].as_array().is_some_and(|ps| ps.iter().any(|p| p["nature"] == "strong"))
}

/// The drivers of `catalogue` (`hardware_candidates`) that best match a MIDI
/// port, by its name and the SysEx the device sent back while detected
/// (`recognition::rank`), best first: id, slug, name, vendor, source tier,
/// score, and, when its identity block recognised the device, how, the
/// firmware read from the reply and the drivers matching it equally. The hosted server answers
/// `POST /v1/detect` with them.
pub fn match_port(catalogue: &[crate::recognition::Candidate], port: &str, usb: Option<(u16, u16)>,
                  replies: &[Vec<u8>], order: crate::recognition::TierOrder) -> Vec<Value> {
    crate::recognition::rank(port, usb, replies, catalogue, 3, order).iter().map(|r| {
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

/// What a caller gives to know the requests that recognise devices: asked only when probing.
type ProbesOf<'a> = &'a dyn Fn() -> Result<Vec<crate::recognition::Probe>, String>;

/// What a caller gives to know what the service says of the ports found, one answer per port.
type AnswersOf<'a> = &'a dyn Fn(&[crate::detect::Probed]) -> Result<Vec<Value>, String>;

/// `detect_hardware` as the tool is called: the arguments read, then `detect_hardware`.
pub(crate) fn detect_tool(args: &Value, index: &DeviceIndex, opts: &McpOptions, probes_of: Option<ProbesOf>, answers: AnswersOf) -> Result<Value, String> {
    let probe = args.get("probe").and_then(Value::as_bool).unwrap_or(true);
    let wait = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(800).clamp(100, 5000);
    let consent = consent_arg(args)?;
    detect_hardware(index, probes_of, answers, opts.devices_dir.is_some(), probe, std::time::Duration::from_millis(wait), None, &consent, opts.tier_order)
}

/// What a caller gives to share a fingerprint: the port, the vendor, model and firmware the
/// person gave, and what was learned; it says what happened.
pub(crate) type SharesFingerprint<'a> =
    &'a dyn Fn(&str, &str, &str, &str, &[(crate::detect::Unit, crate::detect::Learned)]) -> Result<String, String>;

/// The sharing of the fingerprint of the port the person named, as a learn mode runs it.
pub(crate) type SharesLearned<'a> = &'a dyn Fn(&[(crate::detect::Unit, crate::detect::Learned)]) -> Result<String, String>;

/// What a caller gives to know what the service says of the devices a learn mode found.
pub(crate) type MatchesOf<'a> = &'a dyn Fn(&[(crate::detect::Unit, crate::detect::Learned)]) -> ServiceMatches;

/// The one device whose port `port` names, or the refusal that asks to name exactly one port.
pub(crate) fn sharing_target<'a>(port: &str, learned: &'a [(crate::detect::Unit, crate::detect::Learned)])
    -> Result<&'a (crate::detect::Unit, crate::detect::Learned), String> {
    let wanted = port.to_lowercase();
    let found: Vec<_> = learned.iter().filter(|(u, _)| u.inputs.iter().chain(&u.outputs).any(|p| p.to_lowercase().contains(&wanted))).collect();
    match found.as_slice() {
        [one] => Ok(one),
        _ => Err(format!("share_fingerprint {port:?} names {} device(s): name exactly one port", found.len())),
    }
}

/// The sharing of a copy with no link: nothing goes out.
fn not_linked_share(port: &str, _vendor: &str, _model: &str, _firmware: &str,
                    learned: &[(crate::detect::Unit, crate::detect::Learned)]) -> Result<String, String> {
    sharing_target(port, learned)?;
    Ok("not sent: this copy is not linked to the SynthHub server".into())
}

/// `detect_hardware` in learn mode as the tool is called: the arguments read, the fingerprint
/// shared through `shares` when a name is given, the ports matched through `matches`.
pub(crate) fn learn_tool(args: &Value, index: &DeviceIndex, opts: &McpOptions, shares: SharesFingerprint,
                         matches: MatchesOf) -> Result<Value, String> {
    let text = |k: &str| args.get(k).and_then(Value::as_str);
    let named = match text("share_fingerprint") {
        None if ["vendor", "model", "firmware"].iter().any(|k| text(k).is_some()) =>
            return Err("vendor, model and firmware go with share_fingerprint: the port whose fingerprint is shared".into()),
        None => None,
        Some(port) => {
            let (vendor, model, firmware) = share_fields(text("vendor"), text("model"), text("firmware"), false)?;
            Some((port, vendor, model, firmware))
        }
    };
    let share = named.map(|(port, vendor, model, firmware)| move |learned: &[(crate::detect::Unit, crate::detect::Learned)]|
        shares(port, vendor, model, firmware, learned));
    learn_hardware(index, matches, crate::detect::learn_listen(args.get("listen_ms").and_then(Value::as_u64))?,
        share.as_ref().map(|s| s as SharesLearned), opts.tier_order)
}

/// Find the MIDI devices plugged into this machine and their drivers: those
/// of `index`, or, given `answers`, those of the SynthHub catalogue the service
/// matched the ports against. `probes_of`, when given, says which requests recognise devices
/// (else the drivers of `index` do). The command to run names the driver by its
/// file when `by_file` (a folder the user keeps), else by its id.
#[allow(clippy::too_many_arguments)] // the caller's order of the cards is the ninth value
pub(crate) fn detect_hardware(index: &DeviceIndex, probes_of: Option<ProbesOf>, answers: AnswersOf, by_file: bool, probe: bool,
                   wait: std::time::Duration, write: Option<&Path>, consent: &[String], order: crate::recognition::TierOrder) -> Result<Value, String> {
    let mut faults = Vec::new();
    let probes = match probes_of {
        Some(probes_of) if probe => probes_of()?,
        _ => { let plan = detect_plan(index); faults = plan.faults; plan.probes }
    };
    let capture = crate::detect::observe(probe, wait, &probes, consent).map_err(|e| format!("MIDI scan: {e}"))?;
    if let Some(path) = write {
        capture.write(path).map_err(|e| e.to_string())?;
    }
    let plan = if probe { held_plan(&capture, &probes, consent)? } else { Vec::new() };
    let found = crate::detect::replay(&capture);
    let said = answers(&found)?;
    detection_report(index, &said, by_file, probe, found, faults, &plan, order)
}

/// What each output port of `capture` received of `probes`, and what was held back.
fn held_plan(capture: &crate::capture::Capture, probes: &[crate::recognition::Probe], consent: &[String]) -> Result<Vec<crate::recognition::PortSends>, String> {
    let consented = crate::detect::consent_indexes(&capture.outputs, consent)?;
    Ok(crate::recognition::send_plan(&crate::detect::output_usbs(capture), probes, &consented))
}

/// The devices a capture shows, matched against the drivers of `devices_dir`: the report of
/// `detect_hardware`, with no device plugged in and nothing sent. The caller's `order` ranks two
/// cards of one device.
pub fn detect_capture(devices_dir: &Path, capture: &crate::capture::Capture, order: crate::recognition::TierOrder) -> Result<Vec<Value>, String> {
    let index = DeviceIndex::build(devices_dir, order);
    let report = detection_report(&index, &[], true, false,crate::detect::replay(capture), Vec::new(), &[], order)?;
    Ok(report["structuredContent"]["devices"].as_array().cloned().unwrap_or_default())
}

/// `detect_capture` with the requests `probes` would go out as, given the `consent` of the person:
/// each device lists the maker requests held back for its output port.
pub fn detect_capture_held(devices_dir: &Path, capture: &crate::capture::Capture, probes: &[crate::recognition::Probe],
                           consent: &[String], order: crate::recognition::TierOrder) -> Result<Vec<Value>, String> {
    let index = DeviceIndex::build(devices_dir, order);
    let plan = held_plan(capture, probes, consent)?;
    let report = detection_report(&index, &[], true, true,crate::detect::replay(capture), Vec::new(), &plan, order)?;
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

/// The report of `detect_hardware` over the devices `found`. `answers` is what the service said of
/// each port, one answer per port in order; without any, the ports are matched against `index`.
#[allow(clippy::too_many_arguments)] // the caller's order of the cards is the eighth value
fn detection_report(index: &DeviceIndex, answers: &[Value], by_file: bool, probe: bool,
                    found: Vec<crate::detect::Probed>, faults: Vec<crate::device::DeviceFault>,
                    plan: &[crate::recognition::PortSends], order: crate::recognition::TierOrder) -> Result<Value, String> {
    let mut matched: Vec<Vec<Value>> = if answers.is_empty() {
        let catalogue = hardware_candidates(index);
        found.iter().map(|d| match_port(&catalogue, d.label(), d.usb, &d.replies, order).into_iter().map(|mut c| { c["kind"] = json!("candidate"); c }).collect()).collect()
    } else {
        (0..found.len()).map(|i| answers.get(i).map(port_answer).unwrap_or_default()).collect()
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
                "manufacturer": crate::recognition::manufacturer_name(&id.manufacturer),
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
pub(crate) fn learn_hardware(index: &DeviceIndex, matches: MatchesOf, listen: std::time::Duration,
                             share: Option<SharesLearned>,
                             order: crate::recognition::TierOrder) -> Result<Value, String> {
    let learned = crate::detect::learn_all(listen).map_err(|e| format!("MIDI scan: {e}"))?;
    let sharing = share.map(|s| s(&learned)).transpose()?;
    let service = matches(&learned);
    let mut report = learn_report(&learned, index, listen, &service, order);
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
pub(crate) fn learn_devices(learned: &[(crate::detect::Unit, crate::detect::Learned)]) -> Vec<&(crate::detect::Unit, crate::detect::Learned)> {
    learned.iter().filter(|(u, _)| !crate::detect::is_system_unit(u)).collect()
}

/// One port of the detection request: its name, its identity replies and, when read, its USB ids
/// (`usb: {vendor_id, product_id}`, numbers); a port without ids has no `usb` field.
pub(crate) fn detect_port(port: &str, replies_hex: Vec<String>, usb: Option<(u16, u16)>) -> Value {
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
pub(crate) fn port_answer(answer: &Value) -> Vec<Value> {
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
    let ask = format!("{NEXT_STEP_ASK}{} Then map its controls one by one with report_mapping, device being its vendor and model as shared (e.g. Behringer BCR-32); a first driver built from the mappings goes to submit_driver.",
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
/// device named as the service accepts it, « maker model » as the person declared it, the firmware
/// not being part of it.
pub fn shared_next_step(vendor: &str, model: &str) -> String {
    format!("Map its controls one by one with report_mapping (device: {}); a first driver built from the mappings goes to submit_driver.",
        declared_name(vendor, model))
}

/// The name of a device without a card as the service accepts it: the model alone when it already
/// starts with the maker (whole word, case ignored), else the maker and the model; spaces reduced.
fn declared_name(vendor: &str, model: &str) -> String {
    let (v, m): (Vec<&str>, Vec<&str>) = (vendor.split_whitespace().collect(), model.split_whitespace().collect());
    let starts = !v.is_empty() && m.len() >= v.len() && v.iter().zip(&m).all(|(a, b)| a.eq_ignore_ascii_case(b));
    if starts { m.join(" ") } else { v.into_iter().chain(m).collect::<Vec<_>>().join(" ") }
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
pub(crate) fn share_fields<'a>(vendor: Option<&'a str>, model: Option<&'a str>, firmware: Option<&'a str>, command: bool)
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
                    service: &ServiceMatches, order: crate::recognition::TierOrder) -> Value {
    let candidates = hardware_candidates(catalogue);
    let devices: Vec<Value> = learn_devices(learned).into_iter().enumerate().map(|(at, (d, l))| {
        let replies: Vec<Vec<u8>> = l.fingerprint["identity_reply"].as_str().map(|h| {
            h.split_whitespace().filter_map(|b| u8::from_str_radix(b, 16).ok()).collect()
        }).into_iter().collect();
        // Only a strong proof names a driver; a name alone leaves the device unknown.
        let program = d.client_kind == crate::detect::ClientKind::User;
        let local = d.inputs.iter().chain(&d.outputs).filter(|_| !program)
            .find_map(|p| crate::recognition::rank(p, d.usb, &replies, &candidates, 1, order).into_iter().find(|r| r.is_strong()))
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

/// The driver of the device a preset tool is told to talk to; a device the
/// catalogue does not know is refused, with the invitation to describe it.
fn preset_device(index: &DeviceIndex, wanted: &str) -> Result<Device, String> {
    let entry = index.find(wanted).ok_or_else(|| format!(
        "no driver '{wanted}' in the catalogue (see list_devices): describe the device first, so osc-bridge knows its banks and sounds"))?;
    crate::device::load(&entry.path).map_err(|e| format!("load {}: {e}", entry.path.display()))
}

/// `install_export`: the export of `source` put on `target` by the way its
/// driver declares (see `crate::install`).
fn install_export(source: &Device, target: &Device, device: &str, on: &str, confirm: bool, open: crate::midi_port::Opener) -> Result<Value, String> {
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

/// The text of a detection report: one block per port, `usb: vvvv:pppp`
/// (as `lsusb` writes it) right under the port when its ids are known.
pub(crate) fn format_detection(devices: &[Value], probe: bool) -> String {
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
