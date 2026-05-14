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
//! The server reads one JSON-RPC request per line from stdin and writes one
//! response per line to stdout. Logging goes to stderr.

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
}

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
    /// per-variant convention), the first one walked wins the slug lookup.
    fn build(dir: &Path) -> Self {
        let mut entries: Vec<DeviceIndexEntry> = Vec::new();
        let mut by_slug: HashMap<String, usize> = HashMap::new();
        let _ = walk_json(dir, &mut |path, doc| {
            let device = doc.get("device");
            let get = |k: &str| device.and_then(|d| d.get(k)).and_then(Value::as_str);
            let slug = get("osc_prefix").unwrap_or("").trim_start_matches('/').to_string();
            let tier = doc.get("_sources").and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|s| s.get("type").and_then(Value::as_str))
                .unwrap_or("unknown").to_string();
            let idx = entries.len();
            if !slug.is_empty() {
                by_slug.entry(slug.clone()).or_insert(idx);
            }
            entries.push(DeviceIndexEntry {
                path: path.to_path_buf(),
                slug,
                name: get("name").unwrap_or("?").to_string(),
                vendor: get("vendor").unwrap_or("?").to_string(),
                kind: get("kind").unwrap_or("hardware").to_string(),
                revision: get("revision").unwrap_or("").to_string(),
                tier,
            });
        });
        Self { entries, by_slug }
    }

    fn find_path(&self, slug: &str) -> Option<&Path> {
        self.by_slug.get(slug).map(|&i| self.entries[i].path.as_path())
    }
}

pub fn run(opts: McpOptions) -> Result<()> {
    eprintln!("MCP server starting (devices_dir={}, default_target={})",
        opts.devices_dir.display(), opts.default_target);
    // Build the catalogue index once — every device tool reads from it
    // instead of re-walking the tree.
    let index = DeviceIndex::build(&opts.devices_dir);
    eprintln!("indexed {} device file(s)", index.entries.len());

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

fn handle_request(req: &Value, opts: &McpOptions, index: &DeviceIndex) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");

    // Notifications carry no id and expect no response.
    if id.is_none() && method.starts_with("notifications/") {
        return None;
    }

    let result: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(init_result()),
        "tools/list" => Ok(tools_list()),
        "tools/call" => match req.get("params").and_then(|p| Some((p.get("name")?.as_str()?, p.get("arguments").cloned().unwrap_or(json!({})))))
        {
            Some((name, args)) => call_tool(name, &args, opts, index).map_err(|e| (-32602, e)),
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

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "list_devices",
                "description": "Enumerate every device driver JSON. Returns name, vendor, OSC prefix, kind (hardware/software), and source tier for each.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "count": { "type": "integer", "description": "Number of device drivers found." },
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
                "description": "Return the OSC routes a device exposes: declarative commands (OSC paths it accepts), cc/nrpn params, sysex params, and reply patterns. Use this to discover what a device can do.",
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
                            "description": "CC / NRPN parameters, each mapped to an OSC address.",
                            "items": { "type": "object" }
                        },
                        "params": {
                            "type": "array",
                            "description": "SysEx parameters, each mapped to an OSC address.",
                            "items": { "type": "object" }
                        },
                        "replies": {
                            "type": "array",
                            "description": "Reply patterns the device emits back onto the OSC surface.",
                            "items": { "type": "object" }
                        }
                    },
                    "required": ["device", "osc_prefix", "commands", "cc_params", "params", "replies"]
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
            }
        ]
    })
}

fn call_tool(name: &str, args: &Value, opts: &McpOptions, index: &DeviceIndex) -> Result<Value, String> {
    match name {
        "list_devices" => list_devices(index),
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
        other => Err(format!("unknown tool: {other}")),
    }
}

fn list_devices(index: &DeviceIndex) -> Result<Value, String> {
    let devices: Vec<Value> = index.entries.iter().map(|e| json!({
        "name": e.name,
        "vendor": e.vendor,
        "slug": e.slug,
        "kind": e.kind,
        "revision": e.revision,
        "source_tier": e.tier,
        "file": e.path.display().to_string(),
    })).collect();
    let count = devices.len();
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
    })).collect()).unwrap_or_default();
    let params: Vec<Value> = dev.params.as_ref().map(|t| t.entries.iter().map(|e| json!({
        "address": format!("{prefix}{}", e.osc),
        "range": e.range,
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
    fn tools_list_has_five_tools() {
        let t = tools_list();
        let arr = t.get("tools").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 5);
        let names: Vec<&str> = arr.iter().map(|t| t.get("name").unwrap().as_str().unwrap()).collect();
        for expected in ["list_devices", "get_device_docs", "list_routes", "send", "get_status"] {
            assert!(names.contains(&expected), "tool {expected} missing");
        }
    }

    #[test]
    fn every_tool_declares_schema_and_annotations() {
        let t = tools_list();
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
        let res = list_devices(&idx).unwrap();
        let sc = res.get("structuredContent").expect("structuredContent present");
        assert!(sc.get("count").and_then(Value::as_u64).unwrap() > 100);
        assert!(sc.get("devices").and_then(Value::as_array).unwrap().len() > 100);
        // The text fallback block is still there for non-structured clients.
        assert!(res.get("content").and_then(Value::as_array).is_some());
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
        let opts = McpOptions {
            devices_dir: PathBuf::from("devices"),
            default_target: "127.0.0.1:7777".parse().unwrap(),
        };
        let resp = handle_request(&req, &opts, &empty_index()).unwrap();
        assert_eq!(resp.get("id"), Some(&json!(1)));
        assert!(resp.get("result").is_some());
    }

    #[test]
    fn handle_initialized_notification_returns_nothing() {
        let req = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let opts = McpOptions {
            devices_dir: PathBuf::from("devices"),
            default_target: "127.0.0.1:7777".parse().unwrap(),
        };
        assert!(handle_request(&req, &opts, &empty_index()).is_none());
    }

    #[test]
    fn handle_unknown_method_returns_error() {
        let req = json!({"jsonrpc":"2.0","id":99,"method":"foo/bar","params":{}});
        let opts = McpOptions {
            devices_dir: PathBuf::from("devices"),
            default_target: "127.0.0.1:7777".parse().unwrap(),
        };
        let resp = handle_request(&req, &opts, &empty_index()).unwrap();
        assert!(resp.get("error").is_some());
    }
}
