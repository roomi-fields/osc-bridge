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

use crate::device::{DeviceIdentity, FirmwareRule, FrameToken};
use anyhow::{anyhow, Result};
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
    pub identity: Option<DeviceIdentity>,
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
pub fn probe_of(id: &DeviceIdentity) -> Result<Option<Probe>> {
    let Some(tokens) = &id.request else { return Ok(None) };
    let dev = id.device_id.as_ref().map(|d| d.default).unwrap_or(0x7F);
    let mut bind = crate::frame::Bindings::new();
    bind.set_u7("dev", dev);
    let bytes = crate::frame::build_message(&id.expand(tokens), &bind)
        .map_err(|e| anyhow!("identity.request: {e}"))?;
    if bytes == IDENTITY_REQUEST {
        return Ok(None);
    }
    let ports = if id.to.as_deref() == Some("named_ports") { id.ports.clone() } else { vec![] };
    Ok(Some(Probe { request: bytes, ports }))
}

/// Every distinct request the identities name, in a stable order.
pub fn probes<'a>(identities: impl IntoIterator<Item = &'a DeviceIdentity>) -> Vec<Probe> {
    let mut out: Vec<Probe> = identities.into_iter().filter_map(|i| probe_of(i).ok().flatten()).collect();
    out.sort_by(|a, b| (&a.request, &a.ports).cmp(&(&b.request, &b.ports)));
    out.dedup();
    out
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
pub fn recognise(id: &DeviceIdentity, port: &str, replies: &[Vec<u8>]) -> Option<Recognised> {
    let read: Vec<Vec<u8>> = match id.unpack.as_deref() {
        Some(_) => replies.iter().map(|r| unpack_lsb_8to7(r)).collect(),
        None => replies.to_vec(),
    };
    let matching = (!id.reply.is_empty() || id.script.is_some()).then(|| read.iter().find(|r| reply_matches(id, r))).flatten();
    let by_port = !id.ports.is_empty() && port_matches(&id.ports, port);
    if matching.is_none() && !by_port {
        return None;
    }
    Some(Recognised {
        by_reply: matching.is_some(),
        by_port,
        own_request: matching.is_some() && probe_of(id).ok().flatten().is_some(),
        firmware: matching.and_then(|r| id.firmware.as_ref().and_then(|f| decode_firmware(f, r))),
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

fn reply_matches(id: &DeviceIdentity, reply: &[u8]) -> bool {
    let pattern = id.expand(&id.reply);
    reply.len() >= pattern.len()
        && pattern.iter().zip(reply).all(|(t, b)| match t {
            FrameToken::Literal(l) => l == b,
            FrameToken::Placeholder(_) => true,
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
    let bytes: Vec<u8> = rule.at.iter().map(|&i| reply.get(i).copied()).collect::<Option<_>>()?;
    match (rule.decode.as_str(), bytes.as_slice()) {
        ("raw_bytes", b) if !b.is_empty() => Some(b.iter().map(u8::to_string).collect::<Vec<_>>().join(".")),
        ("korg_minor_major", [min_lo, min_hi, maj_lo, maj_hi]) => {
            let v = |lo: &u8, hi: &u8| (*lo as u16) | ((*hi as u16) << 7);
            Some(format!("{}.{}", v(maj_lo, maj_hi), v(min_lo, min_hi)))
        }
        ("yamaha_tenths", [n]) => Some(format!("{}.{}", 1 + n / 10, n % 10)),
        ("ascii", [_]) => {
            let text: String = reply[rule.at[0]..].iter().take_while(|b| (0x20..0x7F).contains(*b)).map(|b| *b as char).collect();
            let text = text.trim().to_string();
            (!text.is_empty()).then_some(text)
        }
        ("json", [_]) => {
            let end = reply.iter().rposition(|b| *b == 0xF7).unwrap_or(reply.len());
            let doc: serde_json::Value = serde_json::from_slice(reply.get(rule.at[0]..end)?).ok()?;
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
mod tests {
    use super::*;

    fn cand(slug: &str, name: &str, vendor: &str, mfr: &[u8]) -> Candidate {
        Candidate { id: format!("acme/{slug}"), slug: slug.into(), name: name.into(), vendor: vendor.into(),
                    file: format!("devices/{slug}.json"), tier: "pencilresearch".into(),
                    manufacturer_id: mfr.to_vec(), identity: None, firmware: None, midi_of: None }
    }

    fn ident(json: &str) -> DeviceIdentity { serde_json::from_str(json).unwrap() }

    fn with_id(slug: &str, firmware: Option<&str>, id: &str) -> Candidate {
        Candidate { identity: Some(ident(id)), firmware: firmware.map(str::to_string), ..cand(slug, "Box", "Acme", &[]) }
    }

    #[test]
    fn parses_three_byte_manufacturer_reply() {
        // Arturia, family 0x0002, model 0x0004, firmware 1.0.4.0
        let msg = [0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x00, 0x20, 0x6B, 0x02, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0xF7];
        let id = parse_identity_reply(&msg).unwrap();
        assert_eq!(id.manufacturer, vec![0x00, 0x20, 0x6B]);
        assert_eq!((id.family, id.model), (2, 4));
        assert_eq!(id.version, [1, 0, 4, 0]);
        assert_eq!(parse_identity_reply(&id.reply()), Some(id.clone()), "written back as it was read");
        assert_eq!(manufacturer_name(&id.manufacturer), Some("Arturia"));
    }

    #[test]
    fn parses_one_byte_manufacturer_reply() {
        let msg = [0xF0, 0x7E, 0x10, 0x06, 0x02, 0x41, 0x31, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0xF7];
        let id = parse_identity_reply(&msg).unwrap();
        assert_eq!(id.manufacturer, vec![0x41]);
        assert_eq!(id.family, 0x31 | (0x02 << 7));
    }

    #[test]
    fn rejects_non_identity_and_truncated_messages() {
        assert!(parse_identity_reply(&IDENTITY_REQUEST).is_none());
        assert!(parse_identity_reply(&[0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x41, 0x01, 0xF7]).is_none());
        assert!(parse_identity_reply(&[0x90, 60, 100]).is_none());
    }

    #[test]
    fn port_name_alone_finds_the_device() {
        let cat = vec![
            cand("minilab3", "MiniLab 3", "Arturia", &[]),
            cand("keystep", "KeyStep", "Arturia", &[]),
            cand("tr_8s", "TR-8S", "Roland", &[]),
        ];
        let best = rank("Minilab3 MIDI", &[], &cat, 3);
        assert_eq!(cat[best[0].index].slug, "minilab3");
        let best = rank("TR-8S", &[], &cat, 3);
        assert_eq!(cat[best[0].index].slug, "tr_8s");
    }

    #[test]
    fn identity_breaks_ties_between_same_named_models() {
        let cat = vec![
            cand("ms_20_korg", "MS-20", "Korg", &[]),
            cand("ms_20_arturia", "MS-20", "Arturia", &[]),
        ];
        let korg = Identity { manufacturer: vec![0x42], family: 0, model: 0, version: [0; 4] };
        let best = rank("MS-20 mini", &[korg.reply()], &cat, 2);
        assert_eq!(cat[best[0].index].slug, "ms_20_korg");
    }

    #[test]
    fn an_input_is_paired_with_its_device_output() {
        let outs = vec!["Midi Through Port-0".to_string(), "minilogue xd MIDI 2".to_string()];
        assert_eq!(output_for("minilogue xd MIDI 1", &outs).as_deref(), Some("minilogue xd MIDI 2"));
        assert_eq!(output_for("Midi Through Port-0", &outs).as_deref(), Some("Midi Through Port-0"));
        assert_eq!(output_for("TR-8S", &outs), None);
        assert_eq!(output_for("TR-8S", &outs[1..]).as_deref(), Some("minilogue xd MIDI 2"), "the only output");
    }

    #[test]
    fn unrelated_port_matches_nothing() {
        let cat = vec![
            cand("minilab3", "MiniLab 3", "Arturia", &[]),
            cand("pcm_80_v1_14", "PCM 80 v1.14", "Lexicon", &[]),
        ];
        assert!(rank("Midi Through:Midi Through Port-0 14:0", &[], &cat, 3).is_empty());
    }

    // A Roland reply: device id 10h, family 59 01, member 00 00, revision 00 03 00 00.
    const ROLAND: [u8; 15] = [0xF0, 0x7E, 0x10, 0x06, 0x02, 0x41, 0x59, 0x01, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0xF7];

    #[test]
    fn an_identity_reply_finds_the_device_behind_a_nameless_port() {
        let cat = vec![
            with_id("mc909", None, r#"{"reply":[240,126,"{dev}",6,2,65,89,1,0,0],"firmware":{"decode":"raw_bytes","at":[10,11,12,13]}}"#),
            with_id("mc808", None, r#"{"reply":[240,126,"{dev}",6,2,65,20,2,0,0]}"#),
            cand("tr_8s", "TR-8S", "Roland", &[0x41]),
        ];
        let best = rank("USB MIDI Interface", &[ROLAND.to_vec()], &cat, 3);
        assert_eq!(best.len(), 1, "only the recognised driver: {best:?}");
        assert_eq!(cat[best[0].index].slug, "mc909");
        assert_eq!(best[0].recognised.as_ref().unwrap().firmware.as_deref(), Some("0.3.0.0"));
        assert!(rank("USB MIDI Interface", &[], &cat, 3).is_empty(), "no reply, nothing to go by");
    }

    #[test]
    fn the_firmware_in_the_reply_picks_the_variant() {
        let id = r#"{"reply":[240,126,"{dev}",6,2,65,89,1,0,0],"firmware":{"decode":"raw_bytes","at":[10,11,12,13]}}"#;
        let cat = vec![with_id("old", Some("0.2"), id), with_id("new", Some("0.3"), id), with_id("any", None, id)];
        let order: Vec<&str> = rank("x", &[ROLAND.to_vec()], &cat, 3).iter().map(|r| cat[r.index].slug.as_str()).collect();
        assert_eq!(order, ["new", "any", "old"], "the same version first, another version last");
    }

    #[test]
    fn of_equal_matches_the_driver_the_others_build_on_comes_first() {
        let id = r#"{"reply":[240,126,"{dev}",6,2,65,89,1,0,0]}"#;
        let fork = Candidate { midi_of: Some("acme/zbase".into()), ..with_id("afork", None, id) };
        let cat = vec![fork, with_id("zbase", None, id), cand("other", "Other", "Acme", &[])];
        let best = rank("USB MIDI Interface", &[ROLAND.to_vec()], &cat, 3);
        let order: Vec<&str> = best.iter().map(|r| cat[r.index].slug.as_str()).collect();
        assert_eq!(order, ["zbase", "afork"], "the shared MIDI first, though its id sorts last");
        assert_eq!((best[0].equally.as_slice(), best[1].equally.as_slice()), (&[0][..], &[1][..]), "each says the other matches too");
        let alone = rank("USB MIDI Interface", &[ROLAND.to_vec()], &cat[1..], 3);
        assert!(alone[0].equally.is_empty(), "no other match, nothing to say");
    }

    #[test]
    fn a_maker_request_and_its_text_tell_a_firmware_apart() {
        let stock = with_id("stock", None, r#"{"reply":[240,126,"{dev}",6,2,0,32,60,2,0]}"#);
        let fork = with_id("mdx", None, r#"{"request":[240,0,32,60,2,0,112,48,247],"reply":[240,0,32,60,2,0,114,48]}"#);
        let named = with_id("felucca", None, r#"{"request":[240,125,70,76,1,247],"to":"named_ports","ports":["FM-1"],
            "reply":[240,125,70,76],"reply_text":"Dinghy","firmware":{"decode":"ascii","at":[5]}}"#);
        let cat = vec![stock, fork, named];
        let uir = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x00, 0x20, 0x3C, 0x02, 0x00, 0x00, 0x00, 1, 2, 3, 4, 0xF7].to_vec();
        let mdx = [0xF0, 0x00, 0x20, 0x3C, 0x02, 0x00, 0x72, 0x30, 1, 0, 0, 0, 0xF7].to_vec();
        let best = rank("x", &[uir.clone(), mdx], &cat, 3);
        assert_eq!(cat[best[0].index].slug, "mdx", "{best:?}");
        assert_eq!(cat[rank("x", &[uir], &cat, 3)[0].index].slug, "stock");

        let info = [&[0xF0, 0x7D, 0x46, 0x4C, 0x02][..], b"Dinghy 0.4", &[0xF7]].concat();
        let r = recognise(cat[2].identity.as_ref().unwrap(), "FM-1 Midi", std::slice::from_ref(&info)).unwrap();
        assert!(r.by_reply && r.by_port && r.own_request);
        assert_eq!(r.firmware.as_deref(), Some("Dinghy 0.4"));
        let other = [&[0xF0, 0x7D, 0x46, 0x4C, 0x02][..], b"Grimm", &[0xF7]].concat();
        assert!(recognise(cat[2].identity.as_ref().unwrap(), "x", &[other]).is_none(), "its text is not there");

        let all = probes(cat.iter().filter_map(|c| c.identity.as_ref()));
        assert_eq!(all.len(), 2, "the universal request is not a probe: {all:?}");
        let felucca = all.iter().find(|p| p.request[1] == 0x7D).unwrap();
        assert!(felucca.goes_to("FM-1 Midi") && !felucca.goes_to("Digitakt"), "sent only where it is safe");
        assert!(all.iter().find(|p| p.request[1] == 0x00).unwrap().goes_to("anything"));
    }

    #[test]
    fn a_packed_reply_is_read_once_unpacked() {
        // The M-VAVE identify query, as sent and as its 8-bit data.
        let query = [0xF0, 0x00, 0x32, 0x45, 0x00, 0x00, 0x00, 0x40, 0x7F, 0xF7];
        assert_eq!(unpack_lsb_8to7(&query), [0x00, 0x59, 0x11, 0x00, 0x00, 0x00, 0xFF]);
        let pack = |data: &[u8]| -> Vec<u8> {
            let (mut out, mut acc, mut bits) = (vec![0xF0], 0u32, 0u32);
            for b in data {
                acc |= (*b as u32) << bits;
                bits += 8;
                while bits >= 7 { out.push((acc & 0x7F) as u8); acc >>= 7; bits -= 7; }
            }
            if bits > 0 { out.push((acc & 0x7F) as u8); }
            out.push(0xF7);
            out
        };
        assert_eq!(pack(&[0x00, 0x59, 0x11, 0x00, 0x00, 0x00, 0xFF]), query, "the test packs as the device does");
        let reply = pack(&[&[0x00, 0x59, 0x11, 0x1B, 0x00, 0x00][..], b"FM-1_016", &[0; 19], &[0x42]].concat());
        let v16 = ident(r#"{"request":[240,0,50,69,0,0,0,64,127,247],"unpack":"lsb_8to7","reply":[0,89,17],"reply_text":"FM-1_016",
            "firmware":{"decode":"ascii","at":[6]}}"#);
        let r = recognise(&v16, "FM-1 Midi", std::slice::from_ref(&reply)).unwrap();
        assert_eq!(r.firmware.as_deref(), Some("FM-1_016"));
        let v15 = DeviceIdentity { reply_text: Some("FM-1_015".into()), ..v16.clone() };
        assert!(recognise(&v15, "FM-1 Midi", &[reply]).is_none());
    }

    #[test]
    fn a_maker_pattern_takes_the_models_bytes_and_lua_reads_what_no_rule_covers() {
        let id = ident(r#"{"family":[89,1],"member":[0,0],"reply":[240,126,"{dev}",6,2,65,"{family}","{member}"]}"#);
        assert!(recognise(&id, "x", &[ROLAND.to_vec()]).is_some());
        let other = DeviceIdentity { family: vec![20, 2], ..id.clone() };
        assert!(recognise(&other, "x", &[ROLAND.to_vec()]).is_none(), "another model's bytes");
        let req = ident(r#"{"request":[240,0,32,50,0,1,"{family}",8,0,247],"family":[36,0],"reply":[240]}"#);
        assert_eq!(probe_of(&req).unwrap().unwrap().request, [0xF0, 0, 0x20, 0x32, 0, 1, 0x24, 0, 8, 0, 0xF7]);
        // A version written as BCD in one byte: no built-in rule, a few lines of Lua.
        let lua = ident(r#"{"reply":[240,126,"{any}",6,2,65],
            "script":"return reply[7] == 0x59",
            "firmware":{"decode":"lua","script":"local b = reply[12]\nreturn string.format('%d.%02d', b >> 4, b & 15)"}}"#);
        let r = recognise(&lua, "x", &[ROLAND.to_vec()]).unwrap();
        assert_eq!(r.firmware.as_deref(), Some("0.03"));
        let not_it = [0xF0, 0x7E, 0x10, 0x06, 0x02, 0x41, 0x14, 0x02, 0, 0, 0, 0x12, 0, 0, 0xF7];
        assert!(recognise(&lua, "x", &[not_it.to_vec()]).is_none(), "its script says no");
    }

    #[test]
    fn a_usb_port_name_recognises_a_device_that_answers_nothing() {
        let cat = vec![with_id("plinky", None, r#"{"ports":["PlinkySynth"]}"#), cand("other", "Other", "Acme", &[])];
        let best = rank("plinkysynth MIDI 1", &[], &cat, 3);
        assert_eq!(cat[best[0].index].slug, "plinky");
        assert!(best[0].recognised.as_ref().unwrap().by_port);
    }

    #[test]
    fn firmware_decodes_as_each_maker_writes_it() {
        let rule = |decode: &str, at: &[usize]| FirmwareRule { decode: decode.into(), at: at.to_vec(), field: None, script: None };
        // Korg: minor 10 (LSB, MSB), major 2.
        let korg = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x42, 0x51, 0x01, 0x00, 0x00, 10, 0, 2, 0, 0xF7];
        assert_eq!(decode_firmware(&rule("korg_minor_major", &[10, 11, 12, 13]), &korg).as_deref(), Some("2.10"));
        assert_eq!(decode_firmware(&rule("raw_bytes", &[10, 12]), &korg).as_deref(), Some("10.2"));
        let seqtrak = [0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x43, 0x00, 0x41, 0x64, 0x06, 5, 0, 0, 0x7F, 0xF7];
        assert_eq!(decode_firmware(&rule("yamaha_tenths", &[10]), &seqtrak).as_deref(), Some("1.5"));
        assert_eq!(decode_firmware(&rule("raw_bytes", &[40]), &seqtrak), None, "past the end");
        // Electra One: its info reply is JSON after six header bytes.
        let electra = [&[0xF0, 0x00, 0x21, 0x45, 0x01, 0x7F][..], br#"{"versionText":"v4.1.4","model":"mk2"}"#, &[0xF7]].concat();
        let json = FirmwareRule { field: Some("versionText".into()), ..rule("json", &[6]) };
        assert_eq!(decode_firmware(&json, &electra).as_deref(), Some("4.1.4"));
        assert_eq!(decode_firmware(&FirmwareRule { field: Some("serial".into()), ..json }, &electra), None, "no such key");
        assert!(same_version("2.0", "2.0.0") && same_version("v1.5", "1.5") && !same_version("2.1", "2.10"));
        assert!(same_version("hacktribe-94c8e60", "hacktribe-94c8e60") && !same_version("1.2", "x"));
        assert!(same_version("1.2.1", "community-1.2.1") && same_version("FM-1_015", "15") && !same_version("1.1", "1.2.1"));
        assert!(same_version("1.1", "clipper-1.1-experimental") && !same_version("1.2", "1.2-on-2.0"), "the last number counts");
    }
}
