//! Hardware detection: which MIDI devices are plugged in, and which drivers
//! of the catalogue match them.
//!
//! Two signals, combined:
//! - the port name the OS reports (`"Minilab3 MIDI"`, `"TR-8S"`);
//! - the answer to the MIDI Universal Identity Request (`F0 7E 7F 06 01 F7`),
//!   which most devices built after the mid-90s send back with their
//!   manufacturer ID, model family/number and firmware version.
//!
//! The identity request changes nothing on the device; it is the standard
//! "who are you?" every editor sends on connection.

use anyhow::{anyhow, Result};
use osd::{FirmwareRule, FrameToken};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const IDENTITY_REQUEST: [u8; 6] = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// 1 byte, or 3 bytes starting with 0x00.
    pub manufacturer: Vec<u8>,
    pub family: u16,
    pub model: u16,
    pub version: [u8; 4],
}

impl Identity {
    pub fn manufacturer_hex(&self) -> String {
        self.manufacturer.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    /// The Identity Reply this identity reads from (`parse_identity_reply`).
    pub fn reply(&self) -> Vec<u8> {
        let le14 = |v: u16| [(v & 0x7F) as u8, ((v >> 7) & 0x7F) as u8];
        let mut out = vec![0xF0, 0x7E, 0x7F, 0x06, 0x02];
        out.extend(&self.manufacturer);
        out.extend(le14(self.family));
        out.extend(le14(self.model));
        out.extend(self.version);
        out.push(0xF7);
        out
    }
}

/// Parse an Identity Reply: `F0 7E <dev> 06 02 <mfr 1|3> <fam 2> <model 2> <ver 4> F7`.
pub fn parse_identity_reply(msg: &[u8]) -> Option<Identity> {
    if msg.len() < 5 || msg[0] != 0xF0 || msg[1] != 0x7E || msg[3] != 0x06 || msg[4] != 0x02 {
        return None;
    }
    let rest = &msg[5..];
    let mfr_len = if *rest.first()? == 0x00 { 3 } else { 1 };
    if rest.len() < mfr_len + 8 {
        return None;
    }
    let (mfr, rest) = rest.split_at(mfr_len);
    let le14 = |lo: u8, hi: u8| (lo as u16 & 0x7F) | ((hi as u16 & 0x7F) << 7);
    Some(Identity {
        manufacturer: mfr.to_vec(),
        family: le14(rest[0], rest[1]),
        model: le14(rest[2], rest[3]),
        version: [rest[4], rest[5], rest[6], rest[7]],
    })
}

/// Vendor name for the manufacturer IDs most often met on synths. Unknown IDs
/// still match drivers that declare the same `manufacturer_id`.
pub fn manufacturer_name(id: &[u8]) -> Option<&'static str> {
    Some(match id {
        [0x01] => "Sequential",
        [0x7D] => "non-commercial (test device)",
        [0x04] => "Moog",
        [0x06] => "Lexicon",
        [0x07] => "Kurzweil",
        [0x0F] => "Ensoniq",
        [0x10] => "Oberheim",
        [0x18] => "E-mu",
        [0x1C] => "Eventide",
        [0x3E] => "Waldorf",
        [0x40] => "Kawai",
        [0x41] => "Roland",
        [0x42] => "Korg",
        [0x43] => "Yamaha",
        [0x44] => "Casio",
        [0x47] => "Akai",
        [0x52] => "Zoom",
        [0x00, 0x00, 0x0E] => "Alesis",
        [0x00, 0x01, 0x0C] => "Line 6",
        [0x00, 0x20, 0x29] => "Novation",
        [0x00, 0x20, 0x32] => "Behringer",
        [0x00, 0x20, 0x33] => "Access",
        [0x00, 0x20, 0x3C] => "Elektron",
        [0x00, 0x20, 0x6B] => "Arturia",
        [0x00, 0x20, 0x76] => "Teenage Engineering",
        [0x00, 0x21, 0x09] => "Native Instruments",
        [0x00, 0x21, 0x1D] => "Ableton",
        [0x00, 0x21, 0x45] => "Electra One",
        _ => return None,
    })
}

/// A driver of the catalogue, as much as matching needs.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Catalogue id (`korg/minilogue`).
    pub id: String,
    pub slug: String,
    pub name: String,
    pub vendor: String,
    pub file: String,
    pub tier: String,
    pub manufacturer_id: Vec<u8>,
    /// How the driver's device is recognised, and the firmware the driver
    /// is for (its first source's).
    pub identity: Option<osd::Identity>,
    pub firmware: Option<String>,
    /// The driver this one shares its MIDI with (`midi_of`), by id.
    pub midi_of: Option<String>,
}

/// One message sent while detecting, and the ports it goes to (all of them
/// when `ports` is empty).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Probe {
    pub request: Vec<u8>,
    pub ports: Vec<String>,
}

impl Probe {
    pub fn goes_to(&self, port: &str) -> bool {
        self.ports.is_empty() || port_matches(&self.ports, port)
    }
}

/// The request `id` names, its `{dev}` set to the device-id rule's default;
/// None for the Universal Identity Request, which every scan sends.
pub fn probe_of(id: &osd::Identity) -> Result<Option<Probe>> {
    let Some(tokens) = &id.request else { return Ok(None) };
    let dev = id.device_id.as_ref().and_then(|d| d.default).unwrap_or(0x7F) as u8;
    let mut bind = crate::frame::Bindings::new();
    bind.set_u7("dev", dev);
    let bytes = crate::frame::build_message(&crate::frame::expand_identity(id, tokens), &bind)
        .map_err(|e| anyhow!("identity.request: {e}"))?;
    if bytes == IDENTITY_REQUEST {
        return Ok(None);
    }
    let ports = if id.to == Some(osd::IdentityTo::NamedPorts) { id.ports.clone().unwrap_or_default() } else { vec![] };
    Ok(Some(Probe { request: bytes, ports }))
}

/// Every distinct request the identities name, in a stable order.
/// with the faults of the identities set aside because their request cannot be built,
/// each named by its device.
pub fn probes<'a>(identities: impl IntoIterator<Item = (&'a str, &'a osd::Identity)>) -> Probes {
    let mut out = Probes::default();
    for (name, id) in identities {
        match probe_of(id) {
            Ok(Some(p)) => out.probes.push(p),
            Ok(None) => {}
            Err(e) => out.faults.push(crate::device::DeviceFault {
                device: name.to_string(), field: "identity.request".into(), cause: e.to_string() }),
        }
    }
    out.probes.sort_by(|a, b| (&a.request, &a.ports).cmp(&(&b.request, &b.ports)));
    out.probes.dedup();
    out
}

/// The requests of `probes` and the faults of the identities it set aside.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probes {
    pub probes: Vec<Probe>,
    pub faults: Vec<crate::device::DeviceFault>,
}

/// A port name holds one of `patterns`, any case.
pub fn port_matches(patterns: &[String], port: &str) -> bool {
    let port = port.to_lowercase();
    patterns.iter().any(|p| !p.is_empty() && port.contains(&p.to_lowercase()))
}

/// How a device was recognised by its identity block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recognised {
    /// One of the replies matched (`reply`, `reply_text`).
    pub by_reply: bool,
    /// The port name matched `ports`.
    pub by_port: bool,
    /// The reply answered a maker's own request, not the universal one.
    pub own_request: bool,
    /// The firmware version read from the reply.
    pub firmware: Option<String>,
}

/// Whether `id` recognises the device on `port` that sent `replies`.
pub fn recognise(id: &osd::Identity, port: &str, replies: &[Vec<u8>]) -> Option<Recognised> {
    let read: Vec<Vec<u8>> = match id.unpack {
        Some(_) => replies.iter().map(|r| unpack_lsb_8to7(r)).collect(),
        None => replies.to_vec(),
    };
    let reply = id.reply.as_deref().unwrap_or_default();
    let ports = id.ports.as_deref().unwrap_or_default();
    let matching = (!reply.is_empty() || id.script.is_some()).then(|| read.iter().find(|r| reply_matches(id, r))).flatten();
    let by_port = !ports.is_empty() && port_matches(ports, port);
    if matching.is_none() && !by_port {
        return None;
    }
    Some(Recognised {
        by_reply: matching.is_some(),
        by_port,
        own_request: matching.is_some() && probe_of(id).ok().flatten().is_some(),
        firmware: matching.and_then(|r| id.firmware.as_ref().and_then(|f| f.as_ref()).and_then(|f| decode_firmware(f, r))),
    })
}

/// The 8-bit data of a SysEx message whose bytes between F0 and F7 are one
/// stream of 7-bit groups, least significant bit first (M-VAVE).
pub fn unpack_lsb_8to7(msg: &[u8]) -> Vec<u8> {
    let body = msg.strip_prefix(&[0xF0]).unwrap_or(msg);
    let body = body.strip_suffix(&[0xF7]).unwrap_or(body);
    let (mut out, mut acc, mut bits) = (Vec::with_capacity(body.len()), 0u32, 0u32);
    for b in body {
        acc |= ((*b & 0x7F) as u32) << bits;
        bits += 7;
        while bits >= 8 {
            out.push((acc & 0xFF) as u8);
            acc >>= 8;
            bits -= 8;
        }
    }
    out
}

fn reply_matches(id: &osd::Identity, reply: &[u8]) -> bool {
    let pattern = crate::frame::expand_identity(id, id.reply.as_deref().unwrap_or_default());
    reply.len() >= pattern.len()
        && pattern.iter().zip(reply).all(|(t, b)| match t {
            FrameToken::Byte(l) => l == b,
            // `any`, `device_id`, a 0.4 `{placeholder}`: one byte, whatever it is.
            _ => true,
        })
        && id.reply_text.as_deref().is_none_or(|text| {
            !text.is_empty() && reply.windows(text.len()).any(|w| w == text.as_bytes())
        })
        && id.script.as_deref().is_none_or(|code| {
            matches!(run_lua(code, reply), Some(serde_json::Value::Bool(true)))
        })
}

/// What a driver's Lua returns for `reply` (given as `reply`, bytes from
/// 1); None when it fails.
fn run_lua(code: &str, reply: &[u8]) -> Option<serde_json::Value> {
    let engine = crate::scripting::ScriptEngine::new().ok()?;
    engine.call_json(code, &serde_json::json!({ "reply": reply })).ok()
}

/// The firmware version `rule` reads from `reply` (see `FIRMWARE_DECODES`).
pub fn decode_firmware(rule: &FirmwareRule, reply: &[u8]) -> Option<String> {
    if rule.decode == "lua" {
        return match run_lua(rule.script.as_deref()?, reply)? {
            serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        };
    }
    let at: Vec<usize> = rule.at.iter().map(|&i| usize::try_from(i).ok()).collect::<Option<_>>()?;
    let bytes: Vec<u8> = at.iter().map(|&i| reply.get(i).copied()).collect::<Option<_>>()?;
    match (rule.decode.as_str(), bytes.as_slice()) {
        ("raw_bytes", b) if !b.is_empty() => Some(b.iter().map(u8::to_string).collect::<Vec<_>>().join(".")),
        ("korg_minor_major", [min_lo, min_hi, maj_lo, maj_hi]) => {
            let v = |lo: &u8, hi: &u8| (*lo as u16) | ((*hi as u16) << 7);
            Some(format!("{}.{}", v(maj_lo, maj_hi), v(min_lo, min_hi)))
        }
        ("yamaha_tenths", [n]) => Some(format!("{}.{}", 1 + n / 10, n % 10)),
        ("ascii", [_]) => {
            let text: String = reply[at[0]..].iter().take_while(|b| (0x20..0x7F).contains(*b)).map(|b| *b as char).collect();
            let text = text.trim().to_string();
            (!text.is_empty()).then_some(text)
        }
        ("json", [_]) => {
            let end = reply.iter().rposition(|b| *b == 0xF7).unwrap_or(reply.len());
            let doc: serde_json::Value = serde_json::from_slice(reply.get(at[0]..end)?).ok()?;
            let v = doc.get(rule.field.as_deref()?)?.as_str()?.trim();
            let v = v.strip_prefix(['v', 'V']).unwrap_or(v);
            (!v.is_empty()).then(|| v.to_string())
        }
        _ => None,
    }
}

/// Two firmware versions name the same release: equal, or the last
/// version number in each (`2.0`, `community-1.2.1`, `FM-1_015`) equal once
/// trailing zeros are dropped (`2.0` and `2.0.0`, `015` and `15`).
pub fn same_version(a: &str, b: &str) -> bool {
    let nums = |s: &str| -> Option<Vec<u32>> {
        let mut runs: Vec<Vec<u32>> = Vec::new();
        let mut cur = String::new();
        for c in s.chars().chain(std::iter::once(' ')) {
            if c.is_ascii_digit() || (c == '.' && !cur.is_empty() && !cur.ends_with('.')) {
                cur.push(c);
            } else if !cur.is_empty() {
                runs.push(cur.trim_end_matches('.').split('.').filter_map(|p| p.parse().ok()).collect());
                cur.clear();
            }
        }
        let mut v = runs.pop()?;
        while v.len() > 1 && v.last() == Some(&0) { v.pop(); }
        Some(v)
    };
    a.trim().eq_ignore_ascii_case(b.trim()) || matches!((nums(a), nums(b)), (Some(x), Some(y)) if x == y)
}

/// Words that say nothing about which device a port belongs to.
const NOISE: &[&str] = &[
    "midi", "port", "in", "out", "input", "output", "usb", "device", "audio",
    "interface", "ctrl", "control", "da", "the", "and",
];

fn tokens(s: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for raw in s.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()) {
        // Bare numbers ("14:0" client IDs, "v1.14") identify nothing.
        if raw.is_empty() || NOISE.contains(&raw) || raw.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        out.insert(raw.to_string());
    }
    out
}

/// The output of the device whose input is `input`: the same name, else the
/// one sharing most words with it, else the only one.
pub fn output_for(input: &str, outputs: &[String]) -> Option<String> {
    if let Some(o) = outputs.iter().find(|o| *o == input) {
        return Some(o.clone());
    }
    let it = tokens(input);
    outputs.iter().map(|o| (tokens(o).intersection(&it).count(), o)).filter(|(n, _)| *n > 0).max_by_key(|(n, _)| *n)
        .map(|(_, o)| o.clone())
        .or_else(|| (outputs.len() == 1).then(|| outputs[0].clone()))
}

/// Collapse "Minilab3", "mini lab 3", "MiniLab-3" to one key.
fn squash(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// A candidate driver for a port: its score, its place in the catalogue,
/// and what recognised it.
#[derive(Debug, Clone)]
pub struct Ranked {
    pub score: u32,
    pub index: usize,
    pub recognised: Option<Recognised>,
    /// The other recognised candidates with the same score (indexes in the
    /// catalogue), which the device's replies match as well.
    pub equally: Vec<usize>,
}

/// Score every candidate against a port name and the SysEx the device sent
/// back while detected (its Identity Reply, its answers to maker requests);
/// return the best ones, highest score first (then the most trustworthy
/// source). A driver whose identity block
/// recognises the device ranks first, the one for the firmware it reported
/// above the others; drivers matching neither the name nor the identity are
/// dropped.
pub fn rank(port_name: &str, replies: &[Vec<u8>], catalogue: &[Candidate], limit: usize) -> Vec<Ranked> {
    let port_tokens = tokens(port_name);
    let port_squashed = squash(port_name);
    let identity = replies.iter().find_map(|r| parse_identity_reply(r));
    let id_vendor = identity.as_ref().and_then(|i| manufacturer_name(&i.manufacturer)).map(squash);
    let mut scored: Vec<Ranked> = catalogue.iter().enumerate().filter_map(|(i, c)| {
        let mut score = 0u32;
        let name_tokens = tokens(&c.name);
        score += 10 * name_tokens.intersection(&port_tokens).count() as u32;
        // The model name, with or without the vendor in front ("Arturia
        // MiniLab 3" vs port "Minilab3 MIDI"), or the driver slug.
        let vendor_sq = squash(&c.vendor);
        let full = squash(&c.name);
        let model = full.strip_prefix(vendor_sq.as_str()).filter(|m| !m.is_empty()).unwrap_or(&full).to_string();
        let slug = squash(&c.slug);
        // A longer match is a more specific one: "minilab3" beats "mini".
        if let Some(len) = [full, model, slug].iter()
            .filter(|k| k.len() >= 3 && port_squashed.contains(k.as_str()))
            .map(String::len).max()
        {
            score += 10 + 3 * len as u32;
        }
        let recognised = c.identity.as_ref().and_then(|id| recognise(id, port_name, replies));
        if score == 0 && recognised.is_none() {
            return None;
        }
        let vendor = vendor_sq;
        if !vendor.is_empty() && port_squashed.contains(&vendor) {
            score += 5;
        }
        if let Some(id) = &identity {
            if !c.manufacturer_id.is_empty() && c.manufacturer_id.starts_with(&id.manufacturer) {
                score += 40;
            } else if id_vendor.as_deref().is_some_and(|v| vendor.contains(v) || v.contains(&vendor)) {
                score += 20;
            }
        }
        if let Some(r) = &recognised {
            if r.by_reply { score += 100; }
            if r.own_request { score += 20; }
            if r.by_port { score += 60; }
            match (&r.firmware, &c.firmware) {
                (Some(got), Some(fw)) if same_version(got, fw) => score += 50,
                (Some(_), Some(_)) => score = score.saturating_sub(30),
                _ => {}
            }
        }
        Some(Ranked { score, index: i, recognised, equally: vec![] })
    }).collect();
    // Equal scores: the driver the others share their MIDI with first (they
    // are built on it), then the most trustworthy source, as a slug resolves.
    let shared: HashSet<(u32, &str)> = scored.iter()
        .filter_map(|r| catalogue[r.index].midi_of.as_deref().map(|m| (r.score, m))).collect();
    let builds_others = |r: &Ranked| !shared.contains(&(r.score, catalogue[r.index].id.as_str()));
    scored.sort_by(|a, b| b.score.cmp(&a.score)
        .then_with(|| builds_others(a).cmp(&builds_others(b)))
        .then_with(|| crate::mcp::tier_rank(&catalogue[a.index].tier).cmp(&crate::mcp::tier_rank(&catalogue[b.index].tier)))
        .then_with(|| catalogue[a.index].id.cmp(&catalogue[b.index].id)));
    let equally: Vec<Vec<usize>> = scored.iter().map(|r| match r.recognised {
        Some(_) => scored.iter().filter(|o| o.index != r.index && o.score == r.score && o.recognised.is_some())
            .map(|o| o.index).collect(),
        None => vec![],
    }).collect();
    for (r, e) in scored.iter_mut().zip(equally) { r.equally = e; }
    scored.truncate(limit);
    scored
}

/// One physical device as seen through its ports.
#[derive(Debug, Clone)]
pub struct Probed {
    pub out_index: Option<usize>,
    pub out_name: Option<String>,
    pub in_index: Option<usize>,
    pub in_name: Option<String>,
    pub identity: Option<Identity>,
    /// Every SysEx message its input sent while detecting, in order.
    pub replies: Vec<Vec<u8>>,
}

impl Probed {
    pub fn label(&self) -> &str {
        self.out_name.as_deref().or(self.in_name.as_deref()).unwrap_or("?")
    }
}

/// SysEx messages kept per input while detecting.
const REPLIES_KEPT: usize = 32;
/// The SysEx messages heard while detecting, each with the index of its input.
type Replies = Arc<Mutex<Vec<(usize, Vec<u8>)>>>;
/// Gap between two requests on one port.
const PROBE_GAP: Duration = Duration::from_millis(10);

/// List MIDI ports, pair inputs with outputs by name, and — when `probe` is
/// true — send the Identity Request, then each of `probes` (the maker
/// requests the catalogue's identity blocks name, each to its ports), on
/// every output and collect the SysEx that comes back for `wait`.
pub fn scan(probe: bool, wait: Duration, probes: &[Probe]) -> Result<Vec<Probed>> {
    let output = MidiOutput::new("osc-bridge-detect")?;
    let out_ports = output.ports();
    let out_names: Vec<String> = out_ports.iter()
        .map(|p| output.port_name(p).unwrap_or_else(|_| "?".into())).collect();
    let input = MidiInput::new("osc-bridge-detect")?;
    let in_ports = input.ports();
    let in_names: Vec<String> = in_ports.iter()
        .map(|p| input.port_name(p).unwrap_or_else(|_| "?".into())).collect();

    // Pair each output with the input of the same (or most similar) name.
    let mut devices: Vec<Probed> = Vec::new();
    let mut used_in: HashSet<usize> = HashSet::new();
    for (oi, oname) in out_names.iter().enumerate() {
        let ot = tokens(oname);
        let best = in_names.iter().enumerate()
            .filter(|(ii, _)| !used_in.contains(ii))
            .map(|(ii, iname)| (ot.intersection(&tokens(iname)).count(), ii))
            .filter(|(n, _)| *n > 0)
            .max();
        let in_index = best.map(|(_, ii)| ii);
        if let Some(ii) = in_index { used_in.insert(ii); }
        devices.push(Probed {
            out_index: Some(oi),
            out_name: Some(oname.clone()),
            in_index,
            in_name: in_index.map(|ii| in_names[ii].clone()),
            identity: None,
            replies: Vec::new(),
        });
    }
    for (ii, iname) in in_names.iter().enumerate() {
        if !used_in.contains(&ii) {
            devices.push(Probed { out_index: None, out_name: None, in_index: Some(ii),
                                  in_name: Some(iname.clone()), identity: None, replies: Vec::new() });
        }
    }
    if !probe {
        return Ok(devices);
    }

    // Listen on every input, then ask every output who it is.
    let replies: Replies = Arc::new(Mutex::new(Vec::new()));
    let mut listeners: Vec<MidiInputConnection<()>> = Vec::new();
    for (ii, port) in in_ports.iter().enumerate() {
        let mut inp = MidiInput::new("osc-bridge-detect-in")?;
        inp.ignore(Ignore::None);
        let sink = replies.clone();
        if let Ok(conn) = inp.connect(port, "osc-bridge-detect-in", move |_, msg, _| {
            if msg.first() == Some(&0xF0) {
                if let Ok(mut g) = sink.lock() {
                    if g.iter().filter(|(i, _)| *i == ii).count() < REPLIES_KEPT { g.push((ii, msg.to_vec())); }
                }
            }
        }, ()) {
            listeners.push(conn);
        }
    }
    let mut conns = Vec::new();
    for (oi, port) in out_ports.iter().enumerate() {
        let out = MidiOutput::new("osc-bridge-detect-out")?;
        if let Ok(conn) = out.connect(port, "osc-bridge-detect-out") {
            conns.push((oi, conn));
        }
    }
    let universal = Probe { request: IDENTITY_REQUEST.to_vec(), ports: vec![] };
    for p in std::iter::once(&universal).chain(probes) {
        for (oi, conn) in conns.iter_mut() {
            if p.goes_to(&out_names[*oi]) {
                let _ = conn.send(&p.request);
            }
        }
        std::thread::sleep(PROBE_GAP);
    }
    std::thread::sleep(wait);
    drop(listeners);

    let got = replies.lock().map_err(|_| anyhow!("detect replies lock poisoned"))?.clone();
    for (ii, msg) in got {
        if let Some(d) = devices.iter_mut().find(|d| d.in_index == Some(ii)) {
            if d.identity.is_none() { d.identity = parse_identity_reply(&msg); }
            if !d.replies.contains(&msg) { d.replies.push(msg); }
        }
    }
    Ok(devices)
}

#[cfg(test)]
mod tests;
