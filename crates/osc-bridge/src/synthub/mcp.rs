//! The MCP server of a copy linked to the SyntHub service: the envelope of the local server.
//!
//! The link answers its own 20 tools first — the 18 the service serves (`online::FORWARDED_TOOLS`),
//! `take_sound` and `load_preset`, and the branches of `list_devices`, `detect_hardware` and
//! `get_trust` that the service answers — and hands every other request to the local answer
//! (`crate::mcp::call_local`), giving it what the service knows as values. The dependency goes
//! from the link to `crate::mcp`, never back.

use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::online::{Online, Settings, FORWARDED_TOOLS};
use super::learn::{learn_service_matches, learn_tool, share_learned, Learned, Share, Unit};
use crate::mcp::{content_structured, device_arg, DeviceIndex, Known, McpOptions};

/// Tools a private device keeps on the machine: refused before anything is
/// sent. `get_trust` on a private device is answered from the trust fetched
/// at startup instead.
const PRIVATE_REFUSED: [&str; 5] = ["report_result", "report_mapping", "submit_fix", "submit_driver", "share_preset"];

/// A local copy's link to the hosted server: the client, the account kept on
/// this machine, and the trust of every reported driver, as last fetched.
struct Link {
    client: Online,
    user: std::sync::Mutex<Option<String>>,
    trust: std::sync::RwLock<Value>,
    /// Devices the user keeps private on this machine (ids, slugs or `vendor/*`, see
    /// `Settings`): nothing about them is sent.
    private: Vec<String>,
}

impl Link {
    fn new(base: &str) -> Self {
        Self { client: Online::new(base), user: std::sync::Mutex::new(None), trust: std::sync::RwLock::new(Value::Null),
               private: Vec::new() }
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
        let unreachable = |e: anyhow::Error| format!("{name} goes to the SyntHub server, which could not be reached: {e}");
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

/// Sent with `initialize` by a linked local copy.
const LINKED_INSTRUCTIONS: &str = "osc-bridge with the SyntHub catalogue of MIDI implementations (CC, NRPN, \
    SysEx, drum maps) of synths, drum machines and DAWs: each driver is fetched from the SyntHub server when a \
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
        Err(e) => eprintln!("SyntHub server unreachable ({e}): only the drivers cached on this machine"),
    })
}

/// The driver a tool call needs on this machine, by id or OSC prefix: the one it
/// reads, or the one it reports on (to tell whether it is private).
fn driver_wanted(req: &Value) -> Option<&str> {
    if req["method"] != "tools/call" { return None; }
    let (name, args) = (req["params"]["name"].as_str()?, &req["params"]["arguments"]);
    match name {
        "get_device_docs" | "list_routes" | "export_device" | "list_roles" | "set_by_role" | "get_by_role" => args["device"].as_str(),
        n if FORWARDED_TOOLS.contains(&n) => args["device"].as_str(),
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
/// in `online::cache_dir`; `private` are the devices the user keeps private.
pub fn run(opts: McpOptions, private: &[String]) -> Result<()> {
    serve(opts, super::online::SERVER, super::online::cache_dir(), private, std::io::stdin().lock(), std::io::stdout().lock())
}

/// `serve_with_midi` on the machine's MIDI system (the guarded opening).
pub fn serve(opts: McpOptions, base: &str, cache: Option<PathBuf>, private: &[String], input: impl BufRead, out: impl Write) -> Result<()> {
    serve_with_midi(opts, base, cache, private, input, out, &crate::midi_port::open_hardware)
}

/// One MCP session linked to the server at `base`: a JSON-RPC message per line of `input`, each
/// reply a line of `out`. It asks the server for the drivers it needs and keeps them in `cache`,
/// unless the user gave `opts.devices_dir`. Nothing about the devices of `private` is sent. The
/// tools that reach the MIDI system take it from `open`.
pub fn serve_with_midi(opts: McpOptions, base: &str, cache: Option<PathBuf>, private: &[String], input: impl BufRead,
                       out: impl Write, open: crate::midi_port::Opener) -> Result<()> {
    use std::sync::{Arc, RwLock};
    let link = Arc::new(Link { private: private.to_vec(), ..Link::new(base) });
    // Drivers from the server are kept in the cache; a folder the user gave is theirs.
    let cache = if opts.devices_dir.is_some() { None } else { cache };
    let root = opts.devices_dir.clone().or_else(|| cache.clone()).unwrap_or_default();
    let index = RwLock::new(Arc::new(crate::mcp::build_index(&root, &opts)));
    let trust = trust_in_background(link.clone());
    let mut fetched = std::collections::HashSet::new();

    let session = crate::mcp::run_session(input, out, |req| {
        // Once a session, the driver a call needs comes from the server; a
        // device the user keeps private by that very name is not asked for.
        if let Some(cache) = &cache {
            for name in drivers_wanted(req) {
                if Settings::covers(&link.private, name, name) || !fetched.insert(name.to_string()) { continue; }
                match super::catalogue::fetch(&link.client, name, cache) {
                    Ok(_) => if let Ok(mut g) = index.write() { *g = Arc::new(DeviceIndex::build(cache, opts.tier_order)) },
                    Err(e) => eprintln!("{name}: {e:#}"),
                }
            }
        }
        let current = index.read().map(|g| g.clone()).map_err(|_| anyhow::anyhow!("catalogue index poisoned"))?;
        let Some(mut resp) = handle_request_with(req, &opts, &current, &link, open) else { return Ok(None) };
        if let (true, Some(result)) = (req["method"] == "initialize", resp.get_mut("result")) {
            result["instructions"] = json!(LINKED_INSTRUCTIONS);
        }
        // A newer osc-bridge the server announced is told once, in the next tool result.
        if let (true, Some(content)) = (req["method"] == "tools/call", resp.pointer_mut("/result/content").and_then(Value::as_array_mut)) {
            if let Some(notice) = super::online::newer_notice_once() {
                content.push(json!({ "type": "text", "text": notice }));
            }
        }
        Ok(Some(resp))
    });
    let _ = trust.join();
    session
}

/// The reply of the link to one JSON-RPC message: its tools are offered beside the local ones.
fn handle_request_with(req: &Value, opts: &McpOptions, index: &DeviceIndex, link: &Link, open: crate::midi_port::Opener) -> Option<Value> {
    crate::mcp::rpc_reply(req, &tool_names(opts), tools_list, |name, args| call_tool_with(name, args, opts, index, link, open))
}

/// The local tools and the link's.
fn tool_names(opts: &McpOptions) -> Vec<&'static str> {
    crate::mcp::tool_names(opts).into_iter().chain(linked_tool_names()).collect()
}

/// The tools the link answers: those the service serves, then the two that need its librarian.
fn linked_tool_names() -> impl Iterator<Item = &'static str> {
    FORWARDED_TOOLS.iter().copied().chain(["take_sound", "load_preset"])
}

fn tools_list() -> Value {
    let mut all = crate::mcp::local_tool_schemas();
    if let (Some(tools), Some(linked)) = (all["tools"].as_array_mut(), linked_tool_schemas()["tools"].as_array()) {
        tools.extend(linked.iter().cloned());
    }
    all
}

#[cfg(test)]
fn call_tool(name: &str, args: &Value, opts: &McpOptions, index: &DeviceIndex, link: &Link) -> Result<Value, String> {
    call_tool_with(name, args, opts, index, link, &crate::midi_port::open_hardware)
}

/// The answer of the link to one call: its own tools first, every other request to the local answer.
fn call_tool_with(name: &str, args: &Value, opts: &McpOptions, index: &DeviceIndex, link: &Link,
                  open: crate::midi_port::Opener) -> Result<Value, String> {
    if args.get("slug").is_some() { device_arg(args)?; }
    if FORWARDED_TOOLS.contains(&name) {
        if let Some(id) = private_device(index, &link.private, args) {
            if name == "get_trust" {
                let trust = link.trust.read().map(|t| t["devices"][&id].clone()).unwrap_or(Value::Null);
                let structured = json!({ "device": id, "private": true,
                    "overall": if trust.is_null() { json!({ "status": "unreported" }) } else { trust } });
                return Ok(content_structured(&format!("{id} is private on this machine: trust as fetched when osc-bridge \
                    started, nothing asked of the server.\n{}", serde_json::to_string_pretty(&structured).unwrap_or_default()), structured));
            }
            if PRIVATE_REFUSED.contains(&name) {
                return Err(format!("{id} is private on this machine: {name} is not sent to the SyntHub server. \
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
    // Drivers from the server: it finds them, and matches the ports found here.
    let server = opts.devices_dir.is_none().then_some(link);
    if let (Some(link), "list_devices") = (server, name) {
        return link.client.catalogue_call(name, args).map_err(|e| format!("list_devices asks the SyntHub server, \
            which could not be reached: {e}. Offline, it lists the drivers cached on this machine."));
    }
    if name == "detect_hardware" {
        if args.get("learn").and_then(Value::as_bool) == Some(true) {
            let shares = |port: &str, vendor: &str, model: &str, firmware: &str, learned: &[(Unit, Learned)]|
                share_learned(&Share { port, vendor, model, firmware, command: false, client: Some(&link.client), private: &link.private }, learned);
            let matches = |learned: &[(Unit, Learned)]| learn_service_matches(server.map(|l| &l.client), learned);
            return learn_tool(args, index, opts, &shares, &matches, open);
        }
        if let Some(server) = server {
            let probes = || service_probes(&server.client);
            let answers = |found: &[crate::detect::Probed]| detect_answers(&server.client, found);
            return crate::mcp::detect_tool(args, index, opts, Some(&probes), &answers, open);
        }
    }
    let status = |id: &str| link.status(id);
    let private = |id: &str| Settings::covers(&link.private, id, id);
    let trust = link.trust.read().map(|t| t.clone()).unwrap_or(Value::Null);
    crate::mcp::call_local(name, args, opts, index, open, &Known { trust, candidates: Vec::new(), status: &status, private: &private })
}

/// What the service says of each port of `found`, one answer per port, in order.
pub fn detect_answers(client: &Online, found: &[crate::detect::Probed]) -> Result<Vec<Value>, String> {
    service_answers(client, found)
}

/// The requests the service says recognise devices.
fn service_probes(server: &Online) -> Result<Vec<crate::recognition::Probe>, String> {
    server.probes().map_err(|e| format!("detect_hardware asks the SyntHub server which \
        requests recognise devices, and it could not be reached: {e}. Offline, the ports are matched against the drivers cached here."))
}

/// What the service says of each port of `found`, one answer per port in order.
fn service_answers(server: &Online, found: &[crate::detect::Probed]) -> Result<Vec<Value>, String> {
    let ports: Vec<Value> = found.iter().map(|d| crate::mcp::detect_port(d.label(),
        d.replies.iter().map(|r| crate::mcp::hex_bytes(r)).collect(), d.usb)).collect();
    server.detect(&ports).map_err(|e| format!("detect_hardware asks the SyntHub server to match \
        the ports, and it could not be reached: {e}. Offline, the ports are matched against the drivers cached here."))
}

/// Whether `private` covers a device named « maker model » without a card: its slug is compared
/// to the list under each cut of the words into maker and model.
fn covers_declared(private: &[String], name: &str) -> bool {
    let words: Vec<&str> = name.split_whitespace().collect();
    (1..=words.len()).any(|k| {
        let id = format!("{}/{}", crate::mcp::slugify(&words[..k].join(" "), '-'), crate::mcp::slugify(&words[k..].join(" "), '-'));
        Settings::covers(private, &id, &id)
    })
}

/// `osc-bridge privacy`: change the settings at `path` — `offline`, devices
/// made private or public again (ids or slugs, found in `index`, else asked
/// of `server`) — then show them. Returns what to print.
pub fn privacy(path: &Path, index: &DeviceIndex, server: Option<&Online>, offline: Option<bool>,
               private: &[String], public: &[String]) -> Result<String> {
    let mut settings = Settings::load(Some(path))?;
    let id_of = |name: &str| match name.strip_suffix("/*") {
        Some(vendor) => super::catalogue::is_slug(vendor).then(|| name.to_string())
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
        "Offline: this copy does not contact the SyntHub server and uses only the drivers cached on this machine \
         (osc-bridge privacy --offline off to link it again).\n".to_string()
    } else {
        "Linked to the SyntHub server: drivers come from it one at a time under the anonymous account of this machine, and reports \
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

/// `osc-bridge detect`: the same report as the `detect_hardware` tool, for a
/// terminal: drivers of `devices_dir`, else of the SyntHub catalogue when
/// linked, else those cached on this machine.
///
/// With `write`, the observation is also written to that file as a capture. With `read`, the
/// capture of that file is replayed instead of scanning: no device, nothing sent; it refuses
/// `probe` off and `write`, a replay sending and writing nothing.
pub fn print_detection(open: crate::midi_port::Opener, devices_dir: Option<&Path>, probe: bool, write: Option<&Path>, read: Option<&Path>, consent: &[String],
                       order: crate::recognition::TierOrder) -> Result<()> {
    let root = devices_dir.map(Path::to_path_buf).or_else(super::online::cache_dir).unwrap_or_default();
    let devices = if let Some(read) = read {
        if !probe { anyhow::bail!("--read cannot be used with --no-probe: a replay sends nothing"); }
        if write.is_some() { anyhow::bail!("--read cannot be used with --write: a replay writes no capture"); }
        if !consent.is_empty() { anyhow::bail!("--read cannot be used with --consent: a replay sends nothing"); }
        let capture = crate::capture::Capture::read(read)?;
        crate::mcp::detect_capture(&root, &capture, order).map_err(|e| anyhow::anyhow!(e))?
    } else {
        let index = DeviceIndex::build(&root, order);
        let server = (devices_dir.is_none() && super::online::linked()).then(|| Online::new(super::online::SERVER));
        let probes = || service_probes(server.as_ref().expect("asked only when linked"));
        let answers = |found: &[crate::detect::Probed]| match &server {
            Some(server) => service_answers(server, found),
            None => Ok(Vec::new()),
        };
        let report = crate::mcp::detect_hardware(&index, server.as_ref().map(|_| &probes as &dyn Fn() -> Result<Vec<crate::recognition::Probe>, String>),
            &answers, devices_dir.is_some(), probe, std::time::Duration::from_millis(800), write, consent, order, open)
            .map_err(|e| anyhow::anyhow!(e))?;
        report["structuredContent"]["devices"].as_array().cloned().unwrap_or_default()
    };
    print!("{}", crate::mcp::format_detection(&devices, probe));
    Ok(())
}

/// The driver a tool call is about, when the user keeps it private: the
/// `device` argument, or the device of a submitted driver. A private id also
/// covers the other variants of the same device (same slug).
fn private_device(index: &DeviceIndex, private: &[String], args: &Value) -> Option<String> {
    let covers = |id: &str, slug: &str| Settings::covers(private, id, slug)
        || private.iter().any(|p| index.find(p).is_some_and(|e| e.slug == slug));
    if let Some(wanted) = args.get("device").and_then(Value::as_str) {
        // Named as the private list names it, even when no driver of it is here.
        if covers(wanted, wanted) {
            return Some(index.find(wanted).map(|e| e.id.clone()).unwrap_or_else(|| wanted.to_string()));
        }
        if index.find(wanted).is_none() && covers_declared(private, wanted) {
            return Some(wanted.to_string());
        }
        let entry = index.find(wanted)?;
        return covers(&entry.id, &entry.slug).then(|| entry.id.clone());
    }
    let dev = args.get("driver")?.get("device")?;
    let (vendor, name) = (dev.get("vendor")?.as_str()?, dev.get("name")?.as_str()?);
    let slug = dev.get("osc_prefix")?.as_str()?.trim_start_matches('/');
    let id = format!("{}/{}", crate::mcp::vendor_dir(index, vendor), crate::mcp::slugify(name, '_'));
    covers(&id, slug).then_some(id)
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
/// `super::take`.
fn librarian_ask(link: &Link) -> impl Fn(&str, &Value) -> Result<Value, String> + '_ {
    move |op, body| link.client.librarian(op, body).map_err(|e| format!("SyntHub's librarian: {e}"))
}

/// Take the sound a synth plays (or a program) through the server's
/// librarian, saved as a .syx file for share_preset.
fn take_sound(args: &Value, link: &Link, open: crate::midi_port::Opener) -> Result<Value, String> {
    use super::take::{find, take};
    let lib = link.client.librarian_synths().map_err(|e| format!("SyntHub's librarian could not be reached: {e}"))?;
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
            .prefix(&format!("{}-{}-", crate::mcp::slugify(&f.name, '-'),
                crate::mcp::slugify(if taken.name.is_empty() { "sound" } else { &taken.name }, '-')))
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
fn load_preset(args: &Value, link: &Link, open: crate::midi_port::Opener) -> Result<Value, String> {
    let id = args.get("id").and_then(Value::as_str).ok_or("missing id")?;
    let out_port = args.get("out_port").and_then(Value::as_str).ok_or("missing out_port")?;
    let gap = Duration::from_millis(args.get("gap_ms").and_then(Value::as_u64).unwrap_or(50).min(2000));
    let preset = link.forward("get_preset", &json!({ "id": id }))?["structuredContent"].clone();
    let mut note = None;
    let channel = match preset["librarian"].as_str().and_then(|l| l.split('@').next()) {
        Some(synth) => {
            let mut lib = link.client.librarian_synths().map_err(|e| format!("SyntHub's librarian could not be reached: {e}"))?;
            if let Some(all) = lib["synths"].as_array_mut() { all.retain(|s| s["synth"] == synth); }
            note = lib["synths"][0]["note"].as_str().map(str::to_string);
            let mut midi = open().map_err(|e| e.to_string())?;
            let found = super::take::find(midi.as_mut(), &lib, &librarian_ask(link), Some(out_port))?;
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
    Ok(content_structured(&format!("Loaded {} ({} message(s)) on {out_port}.{} Tell SyntHub how it went with report_preset.",
        preset["name"].as_str().unwrap_or(id), messages.len(), note.map(|n| format!(" Note: {n}")).unwrap_or_default()), structured))
}

/// The schema of the 20 tools the link answers: the 18 the service serves too
/// (`online::FORWARDED_TOOLS`), `take_sound` and `load_preset`.
pub fn linked_tool_schemas() -> Value {
    json!({
        "tools": [
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
                "description": "Teach the catalogue which MIDI message a control really sends: after the user moved one control on the device (see relay_listen), give the driver address it is (from list_routes) — or, for a control the driver lacks, its panel label — and the CC, NRPN or note it sent, with the firmware and the device's Identity Reply when you have them. When several accounts observe the same thing on one firmware the mapping is learned; get_trust shows learned mappings and where they differ from the driver. For a device with no driver, the first step of its first driver: after detect_hardware in learn mode (its next_step), the person gives its vendor, model and firmware (share_fingerprint), and you map its controls one by one with device set to its vendor and model as its fingerprint was shared (e.g. Behringer BCR-32), a device with a driver keeping its driver id; the mappings then make the first driver you send with submit_driver.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "device": { "type": "string", "description": "Driver id or OSC prefix; for a device with no driver yet, its vendor and model as its fingerprint was shared (e.g. Behringer BCR-32)." },
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
                "description": "Contribute a driver for a device the catalogue lacks (or a better one): the full driver file as a JSON object, in the Open Synth Descriptors format (device.name, device.vendor, device.osc_prefix, and cc_params / params / commands / triggers — see https://synthub.roomi-fields.com/osd/). It is checked, then merged into the SyntHub catalogue by the merge job if it parses, lints and passes the tests, as its own file of source 'contributed'. Say where the data comes from (manual page, tested on the device). A first driver for a device with no driver: build it from the mappings sent with report_mapping, naming the device by the vendor and model its fingerprint was shared with, and give its source and its licence (e.g. 'mapped on the device by report_mapping; license CC0-1.0').",
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
                "description": "Share something the user made on a device with every SyntHub user, under CC BY-SA 4.0: a sound, a sequence or pattern, a kit, a project, a bank of their own sounds, or a MIDI clip any device can play. Its content: the device's own data (SysEx, e.g. from backup_presets), settings by driver address (from list_routes, e.g. {'/filter/cutoff': [90]}, which also works for devices without dumps), and/or a standard MIDI file. Only what the user made: never factory presets, nor sounds or sequences bought or downloaded from others — set own_sound only after asking the user. Made from someone else's shared preset? Pass derived_from: it credits them, as the licence requires. Describe it well (category, genres, tags, tempo, key, tips): that is how people find it. The same content shared twice returns the existing id; the author publishes new versions with update_preset and can withdraw it (withdraw_preset).",
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
                "description": "Search what SyntHub users shared (CC BY-SA 4.0): sounds, sequences, kits, projects, banks, MIDI clips. Filter by device, kind, category, genre, tag, author (pseudonym) or words; sort by newest, most stars, most loaded into a device or most trusted (confirmed on real devices). Returns, per preset, its id, name, kind, device, description, author, version, whether it has device data, settings, a MIDI file or an audio preview (URL), and its counts. Get one with get_preset.",
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
                "description": "Tell SyntHub how a shared preset behaved once loaded on a real device: 'works' (it loaded and sounds as described), 'fails' (refused, wrong sound), or 'factory' (it is a factory preset or someone else's sound: presets that several users flag are hidden until reviewed). Give the firmware.",
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
                "description": "Link this machine's SyntHub account to the user's account on the site: returns a one-time address (valid 15 minutes) for the user to open and sign in (passkey, e-mail, Google or Discord). Afterwards what this copy shared appears under their profile, and they manage it — and their stars, collections, comments — from the site.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
                "annotations": { "title": "Link to the user's account", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true }
            },
            {
                "name": "take_sound",
                "description": "Take the sound a hardware synth is playing — or one of its stored programs — straight from the synth, with its name, and save it as a .syx file. The synth is found among those SyntHub's librarian can talk to (the KnobKraft Orm adaptations, about 90 synths): every MIDI port gets the identity and detect messages, which change nothing on the synth, then the found synth is asked for its sound. To share it: ask the user whether they made this sound themselves (never a factory preset, nor one bought or downloaded), then call share_preset with the device, name, file, librarian and firmware returned here, and own_sound: true. When several synths answer, call again with port.",
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
                "description": "Load a shared preset (find_presets, get_preset) into a device plugged into this computer: its settings are sent by driver address, its patch data as SysEx. Patch data replaces the sound in the device's edit buffer or memory: ask the user, then pass confirm_overwrite: true. Afterwards, tell SyntHub how it went with report_preset.",
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

#[cfg(test)]
mod tests;
