//! A stand-in for the SynthHub server, on a free local port, keeping the
//! service's contract for kept drivers: each file under `/api/v1/devices/`
//! comes with its `ETag` (the SHA-256 of the file served, in hexadecimal,
//! between quotes), a request whose `If-None-Match` names it gets
//! `304 Not Modified` without a body, and a client the service marks as out
//! of date gets `Deprecation` with `Link: <url>; rel="deprecation"`
//! (RFC 9745) on every reply. It also names a device's best variant by its
//! slug (`/v1/devices/<slug>`) and gives an empty trust list. Every request
//! it receives is recorded.

#![allow(dead_code)]

pub mod orchestrator_rig;

use serde_json::Value;
use std::path::Path;

/// A reply route that drops every reply: for a test that does not read what the sender is told.
pub fn no_reply() -> osc_bridge::runtime::ReplyTo {
    std::sync::Arc::new(|_, _| {})
}
use std::sync::{Arc, Mutex};

/// One request the fake server received.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub url: String,
    pub if_none_match: Option<String>,
    /// The status it was answered with.
    pub status: u16,
}

pub struct FakeSynthHub {
    pub base: String,
    pub requests: Arc<Mutex<Vec<Seen>>>,
}

impl FakeSynthHub {
    /// Serve the drivers under `root`; `deprecation`, the address of a newer
    /// osc-bridge, marks every reply as for a deprecated client.
    pub fn start(root: &Path, deprecation: Option<&str>) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (root, seen, deprecation) = (root.to_path_buf(), requests.clone(), deprecation.map(str::to_string));
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let url = req.url().to_string();
                let if_none_match = req.headers().iter().find(|h| h.field.equiv("If-None-Match")).map(|h| h.value.to_string());
                let (status, body, etag) = if url == "/v1/access/challenge" {
                    // The service's delivery (accepts any proof: this fake does not check accounts).
                    (200, br#"{"challenge":"fake","algorithm":"sha256","bits":4,"deliver":"/v1/access/deliver"}"#.to_vec(), None)
                } else if url == "/v1/access/deliver" {
                    let id = "0123456789abcdef0123456789abcdef";
                    (200, serde_json::json!({ "id": id, "user_id": id, "mcp_path": format!("/u/{id}/mcp"), "tier": "free" })
                        .to_string().into_bytes(), None)
                } else if url == "/v1/librarian" {
                    // The synths the librarian knows: one, whose messages ask nothing of a port.
                    (200, serde_json::json!({ "synths": [{ "synth": "acme-box", "name": "Acme Box", "detect": [], "detect_wait_ms": 100 }] })
                        .to_string().into_bytes(), None)
                } else if url == "/v1/librarian/recognize" {
                    (200, serde_json::json!({ "found": [{ "synth": "acme-box", "name": "Acme Box", "channel": 0, "devices": ["acme/box"] }] })
                        .to_string().into_bytes(), None)
                } else { match url.strip_prefix("/api/v1/devices/") {
                    Some(rel) => match std::fs::read(root.join(rel)) {
                        Ok(bytes) => {
                            let etag = format!("\"{}\"", sha256_hex(&bytes));
                            if if_none_match.as_deref() == Some(etag.as_str()) {
                                (304, Vec::new(), Some(etag))
                            } else {
                                (200, bytes, Some(etag))
                            }
                        }
                        Err(_) => (404, b"not found\n".to_vec(), None),
                    },
                    None if url == "/v1/trust" => (200, br#"{"devices":{}}"#.to_vec(), None),
                    // A device's variants by its slug: the Acme one only.
                    None => match url.strip_prefix("/v1/devices/").filter(|s| root.join(format!("acme/{s}.json")).is_file()) {
                        Some(slug) => (200, serde_json::json!({ "best": format!("acme/{slug}"),
                            "variants": [{ "id": format!("acme/{slug}"), "slug": slug }] }).to_string().into_bytes(), None),
                        None => (404, b"{}".to_vec(), None),
                    },
                } };
                seen.lock().unwrap().push(Seen { method: req.method().to_string(), url, if_none_match, status });
                let mut resp = tiny_http::Response::from_data(body).with_status_code(status);
                if let Some(etag) = etag {
                    resp.add_header(header("ETag", &etag));
                }
                if let Some(link) = &deprecation {
                    resp.add_header(header("Deprecation", "@1767225600"));
                    resp.add_header(header("Link", &format!("<{link}>; rel=\"deprecation\"")));
                }
                let _ = req.respond(resp);
            }
        });
        Self { base, requests }
    }

    /// The requests received so far for driver files (`/api/v1/devices/…`).
    pub fn driver_requests(&self) -> Vec<Seen> {
        self.requests.lock().unwrap().iter().filter(|s| s.url.starts_with("/api/v1/devices/")).cloned().collect()
    }

    pub fn forget_requests(&self) {
        self.requests.lock().unwrap().clear();
    }
}

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh folder for one test, under the system's temporary folder,
/// removed when dropped, a failed test included.
pub fn temp(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(&format!("osc-bridge-{tag}-")).tempdir().unwrap()
}

/// A catalogue of two Acme devices: `acme/box` (with its notes, naming the
/// Acme profile in its identity block) and `acme/synth` (with its notes).
pub fn catalogue(root: &Path) {
    std::fs::create_dir_all(root.join("acme")).unwrap();
    std::fs::write(root.join("acme/box.json"), format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box"}},
        "identity":{{"profile":"acme","family":[1,0],"member":[1,0]}}}}"#)).unwrap();
    std::fs::write(root.join("acme/box.md"), "# Box\n\nThe box's notes.\n").unwrap();
    std::fs::write(root.join("acme/_profile.json"), format!(r#"{{"$schema":"{P05}","identity":{{"device_id":{{"default":127}},
        "reply":[240,126,{{"device_id":true}},6,2,0,33,9,{{"identity":"family"}},{{"identity":"member"}}]}}}}"#)).unwrap();
    std::fs::write(root.join("acme/synth.json"), format!(r#"{{"$schema":"{D05}","device":{{"name":"Synth","vendor":"Acme","osc_prefix":"/synth"}}}}"#)).unwrap();
    std::fs::write(root.join("acme/synth.md"), "# Synth\n\nThe synth's notes.\n").unwrap();
}

/// The `$schema` of an OSD 0.5 device and of a 0.5 maker profile.
pub const D05: &str = "https://w3id.org/osd/0.5/device.schema.json";
pub const P05: &str = "https://w3id.org/osd/0.5/profile.schema.json";

/// Write each `(path under root, text)` file, folders made.
pub fn files(root: &Path, files: &[(&str, &str)]) {
    for (rel, text) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

/// The OSD 0.7 text of a driver of an earlier version, as `osd::convert_to` writes it. A
/// checksum is written `{checksum:twos7@5}` in the tests, as the catalogue writes it, and the
/// converter reads it as is.
pub fn to07(text: &str) -> String {
    osd::convert_to(text, osd::Kind::Device, osd::Version::V0_7).unwrap_or_else(|e| panic!("convert: {e}\n{text}"))
}

/// The device a text holds, read by `osd` (no file, no resolution).
pub fn read_device(text: &str) -> osd::Device {
    match osd::read(text, osd::Kind::Device).unwrap_or_else(|e| panic!("read: {e}\n{text}")) {
        osd::Descriptor::Device(d) => *d,
        other => panic!("not a device: {:?}", other.kind()),
    }
}

/// A device from the text of a driver of an earlier version, in its 0.7 form.
pub fn device_of(text: &str) -> osd::Device {
    read_device(&to07(text))
}

/// The frame a JSON list of 0.4 elements stands for, in its 0.7 form.
pub fn frame(json: &str) -> Vec<osd::FrameToken> {
    let d = device_of(&format!(r#"{{"device":{{"name":"F","osc_prefix":"/f"}},"commands":[{{"osc":"/x","frame":{json}}}]}}"#));
    d.commands.unwrap().remove(0).frame.unwrap()
}

/// The text of an address without capture.
pub fn address_text(a: &osd::Address) -> String {
    match a {
        osd::Address::Text(t) => t.clone(),
        osd::Address::Segments(s) => s.iter().map(|p| match p { osd::Segment::Text(t) => t.clone(), osd::Segment::Capture(n) => format!("{{{n}}}") }).collect(),
    }
}

/// An OSD 0.5 device text: `body` is the keys after `device` (a leading comma included).
pub fn device05(name: &str, prefix: &str, body: &str) -> String {
    format!(r#"{{"$schema":"{D05}","device":{{"name":"{name}","vendor":"Acme","osc_prefix":"{prefix}"}}{body}}}"#)
}

/// A client of the server at `base` keeping its account in `dir/user`, a
/// folder of the test's own, never the machine's.
pub fn online_in(base: &str, dir: &Path) -> osc_bridge::synthub::online::Online {
    osc_bridge::synthub::online::Online::keeping_account_in(base, Some(dir.join("user")))
}

static CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// A configuration folder of its own for the client of one session: while it
/// lives, `XDG_CONFIG_HOME` and `APPDATA` name it (so the account, `user`, and
/// the settings, `settings.toml`, go under `osc-bridge/` there, never to the
/// machine's real ones), and a process-wide lock keeps any other guard out.
/// Dropped, it gives both variables back and removes the folder, a failed
/// test included.
pub struct ConfigHome {
    dir: tempfile::TempDir,
    before: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ConfigHome {
    pub fn new() -> Self {
        let lock = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp("config");
        let before = ["XDG_CONFIG_HOME", "APPDATA"].into_iter().map(|k| (k, std::env::var_os(k))).collect();
        for k in ["XDG_CONFIG_HOME", "APPDATA"] {
            std::env::set_var(k, dir.path());
        }
        Self { dir, before, _lock: lock }
    }

    /// The configuration folder.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Where the client keeps its account.
    pub fn account(&self) -> std::path::PathBuf {
        self.dir.path().join("osc-bridge").join("user")
    }

    /// Where the client keeps its settings.
    pub fn settings(&self) -> std::path::PathBuf {
        self.dir.path().join("osc-bridge").join("settings.toml")
    }
}

impl ConfigHome {
    fn give_back(&self) {
        for (k, v) in &self.before {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }

    /// Ends the guard with the lock still held: the variables as the guard
    /// recorded them at creation, and as they stand once given back.
    #[allow(clippy::type_complexity)]
    pub fn end(self) -> (Vec<(&'static str, Option<std::ffi::OsString>)>, Vec<(&'static str, Option<std::ffi::OsString>)>) {
        self.give_back();
        let after = self.before.iter().map(|(k, _)| (*k, std::env::var_os(k))).collect();
        let before = self.before.clone();
        (before, after)
    }
}

impl Drop for ConfigHome {
    fn drop(&mut self) {
        self.give_back();
        let path = self.dir.path().to_path_buf();
        let mut last = None;
        for _ in 0..10 {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => return,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                Err(e) => last = Some(e),
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let message = format!("the configuration folder {} could not be removed: {}", path.display(), last.unwrap());
        if std::thread::panicking() {
            eprintln!("{message}");
        } else {
            panic!("{message}");
        }
    }
}

/// Run one MCP session over stdio-like buffers, linked to the server at
/// `base`, its drivers kept in `cache`: one reply per request with an id.
/// The client's account and settings live in a configuration folder of the
/// session's own (`ConfigHome`), held for the whole session.
pub fn mcp_session(base: &str, cache: &Path, requests: &[Value]) -> Vec<Value> {
    mcp_session_in(&ConfigHome::new(), base, cache, requests)
}

/// `mcp_session` under a configuration folder the caller holds, to read what
/// the session left there.
pub fn mcp_session_in(_home: &ConfigHome, base: &str, cache: &Path, requests: &[Value]) -> Vec<Value> {
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: None,
        default_target: "127.0.0.1:9000".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: None,
    };
    let input: String = requests.iter().map(|r| format!("{r}\n")).collect();
    let mut out = Vec::new();
    osc_bridge::synthub::mcp::serve_with_midi(opts, base, Some(cache.to_path_buf()), &[], input.as_bytes(), &mut out, &refusing("mcp_session")).unwrap();
    String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// `mcp_session_in` with the MIDI the session takes from `open`.
pub fn mcp_session_midi(_home: &ConfigHome, open: osc_bridge::midi_port::Opener, base: &str, cache: &Path, requests: &[Value]) -> Vec<Value> {
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: None,
        default_target: "127.0.0.1:9000".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: Some(osc_bridge::synthub::learn::detect_offline),
    };
    let input: String = requests.iter().map(|r| format!("{r}\n")).collect();
    let mut out = Vec::new();
    osc_bridge::synthub::mcp::serve_with_midi(opts, base, Some(cache.to_path_buf()), &[], input.as_bytes(), &mut out, open).unwrap();
    String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

pub fn tool_call(id: u64, name: &str, args: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } })
}

pub fn initialize() -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } } })
}

/// The text of every content item of a tool call's result.
pub fn texts(reply: &Value) -> Vec<String> {
    reply["result"]["content"].as_array().into_iter().flatten()
        .filter_map(|c| c["text"].as_str().map(str::to_string)).collect()
}

/// The shared fake MIDI hardware: the named output ports (and inputs) are plugged, every send is
/// recorded in `sent`, and only what a device was told to answer ever comes back. A test never
/// opens the machine's MIDI system: it gives this hardware as the opener, or `refusing`.
///
/// The fake also stands for the machine's listings: its own ALSA cards, USB ids and clients
/// (empty by default), so a real device plugged into the machine changes nothing in a test.
pub type SentLog = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// What a fake device sends back on one of its inputs when `request` goes out on one of its outputs.
#[derive(Clone)]
pub struct Answer {
    pub output: String,
    pub input: String,
    pub request: Vec<u8>,
    pub reply: Vec<u8>,
}

#[derive(Clone, Default)]
pub struct FakeHardware {
    /// The output ports, by name.
    pub ports: Vec<String>,
    /// The input ports, by name.
    pub inputs: Vec<String>,
    pub answers: Vec<Answer>,
    /// What an input sends by itself while it is listened to (a control moved).
    pub chatter: Vec<(String, Vec<u8>)>,
    /// The machine's listings this fake stands for: empty unless a test gives them.
    pub asound: osc_bridge::capture::Asound,
    pub sent: SentLog,
    delivered: usize,
}

impl FakeHardware {
    pub fn with_ports(ports: &[&str]) -> Self {
        Self { ports: ports.iter().map(|p| p.to_string()).collect(), ..Self::default() }
    }
    pub fn none() -> Self {
        Self::default()
    }
    /// One device on the port `label` (an output and an input of that name) that answers the
    /// Universal Identity Request with `identity_reply`.
    pub fn device(label: &str, identity_reply: &[u8]) -> Self {
        Self::none().with_device(label, identity_reply)
    }
    /// Another such device.
    pub fn with_device(mut self, label: &str, identity_reply: &[u8]) -> Self {
        self.ports.push(label.to_string());
        self.inputs.push(label.to_string());
        self.with_answer(label, label, &osc_bridge::recognition::IDENTITY_REQUEST, identity_reply)
    }
    /// `reply` comes back on `input` when `request` is sent on `output` (a maker request, say).
    pub fn with_answer(mut self, output: &str, input: &str, request: &[u8], reply: &[u8]) -> Self {
        self.answers.push(Answer { output: output.into(), input: input.into(), request: request.to_vec(), reply: reply.to_vec() });
        self
    }
    /// `msg` arrives on `input` by itself while the input is listened to.
    pub fn with_chatter(mut self, input: &str, msg: &[u8]) -> Self {
        self.chatter.push((input.into(), msg.to_vec()));
        self
    }
    /// The machine's listings this hardware carries: `/proc/asound/cards`, the `usbid` of each card
    /// by number, and `/proc/asound/seq/clients`.
    pub fn with_asound(mut self, cards: &str, usbids: &[(u32, &str)], clients: &str) -> Self {
        self.asound = osc_bridge::capture::Asound {
            cards: cards.into(),
            usbids: usbids.iter().map(|(card, text)| osc_bridge::capture::UsbId { card: *card, text: text.to_string() }).collect(),
            clients: clients.into(),
        };
        self
    }
    /// The sends recorded so far, port and bytes.
    pub fn sent_messages(&self) -> Vec<(String, Vec<u8>)> {
        self.sent.lock().unwrap().clone()
    }
    /// An opener (what the commands take their MIDI from) that hands this hardware over.
    pub fn opener(&self) -> impl Fn() -> anyhow::Result<Box<dyn osc_bridge::midi_port::Midi>> + '_ {
        move || Ok(Box::new(self.clone()) as Box<dyn osc_bridge::midi_port::Midi>)
    }
}

impl osc_bridge::midi_port::Midi for FakeHardware {
    fn outputs(&self) -> Vec<String> {
        self.ports.clone()
    }
    fn send(&mut self, port: &str, messages: &[Vec<u8>], _why: &str) -> Result<(), String> {
        for m in messages {
            self.sent.lock().unwrap().push((port.to_string(), m.clone()));
        }
        Ok(())
    }
    fn received(&mut self, _wait: std::time::Duration) -> Vec<(String, Vec<u8>)> {
        let sent = self.sent.lock().unwrap().clone();
        let mut got = Vec::new();
        for (port, bytes) in sent.iter().skip(self.delivered) {
            for a in self.answers.iter().filter(|a| a.output == *port && a.request == *bytes) {
                got.push((a.output.clone(), a.reply.clone()));
            }
        }
        self.delivered = sent.len();
        got
    }
    fn capture(&mut self, probe: bool, _wait: std::time::Duration, probes: &[osc_bridge::recognition::Probe],
               consent: &[String]) -> anyhow::Result<osc_bridge::capture::Capture> {
        let mut capture = osc_bridge::capture::Capture::new(self.ports.clone(), self.inputs.clone(), self.asound.clone());
        if !probe {
            return Ok(capture);
        }
        let consented = osc_bridge::detect::consent_indexes(&self.ports, consent).map_err(|e| anyhow::anyhow!(e))?;
        let plan = osc_bridge::recognition::send_plan(&osc_bridge::detect::output_usbs(&capture), probes, &consented);
        let rounds = plan.iter().map(|p| p.sent.len()).max().unwrap_or(0);
        for round in 0..rounds {
            for (oi, port) in self.ports.iter().enumerate() {
                let Some(bytes) = plan[oi].sent.get(round) else { continue };
                self.sent.lock().unwrap().push((port.clone(), bytes.clone()));
                capture.requests.push(osc_bridge::capture::Request { port: port.clone(), bytes: bytes.clone() });
                for a in self.answers.iter().filter(|a| a.output == *port && a.request == *bytes) {
                    capture.received.push(osc_bridge::capture::Received { port: a.input.clone(), bytes: a.reply.clone(), ms: 5 * (round as u64 + 1) });
                }
            }
        }
        Ok(capture)
    }
    fn open_wire(&mut self, device: &osc_bridge::synthub::learn::Unit) -> anyhow::Result<Box<dyn osc_bridge::synthub::learn::Wire + Send>> {
        Ok(Box::new(FakeWire {
            inputs: device.inputs.clone(),
            answers: self.answers.clone(),
            chatter: self.chatter.iter().filter(|(p, _)| device.inputs.contains(p)).cloned().collect(),
            sent: self.sent.clone(),
            queue: Vec::new(),
        }))
    }
}

/// The wire of one fake device: what it is sent is recorded, and its answers and chatter are heard.
struct FakeWire {
    inputs: Vec<String>,
    answers: Vec<Answer>,
    chatter: Vec<(String, Vec<u8>)>,
    sent: SentLog,
    queue: Vec<(String, Vec<u8>)>,
}

impl osc_bridge::synthub::learn::Wire for FakeWire {
    fn send(&mut self, port: &str, msg: &[u8]) {
        self.sent.lock().unwrap().push((port.to_string(), msg.to_vec()));
        for a in self.answers.iter().filter(|a| a.output == port && a.request == msg && self.inputs.contains(&a.input)) {
            self.queue.push((a.input.clone(), a.reply.clone()));
        }
    }
    fn heard(&mut self, _listen: std::time::Duration) -> Vec<osc_bridge::synthub::learn::Heard> {
        let mut all = std::mem::take(&mut self.queue);
        all.extend(self.chatter.iter().cloned());
        all.into_iter().enumerate().map(|(n, (port, msg))| osc_bridge::synthub::learn::Heard {
            port, at: std::time::Duration::from_millis(5 * (n as u64 + 1)), msg,
        }).collect()
    }
}

/// An opener whose MIDI system cannot be opened, for the cause `why`.
pub fn failing_opener(why: &'static str) -> impl Fn() -> anyhow::Result<Box<dyn osc_bridge::midi_port::Midi>> {
    move || Err(anyhow::anyhow!(why))
}

/// The guard of the MIDI system in a day test: an opener that refuses, naming `test`, the test that
/// would have opened the machine's MIDI. Every day test that calls the library and does not need
/// MIDI passes it; one that needs MIDI passes `FakeHardware::opener()`.
pub fn refusing(test: &'static str) -> impl Fn() -> anyhow::Result<Box<dyn osc_bridge::midi_port::Midi>> {
    move || Err(anyhow::anyhow!("the test {test} opened the machine's MIDI system: give it a fake opener (FakeHardware::opener())"))
}

/// The environment variable that makes the binary take an opener that fails by construction.
pub const MIDI_ENV: &str = "OSC_BRIDGE_TEST_MIDI";
