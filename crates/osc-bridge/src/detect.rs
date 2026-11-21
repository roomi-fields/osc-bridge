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
use crate::capture::{Asound, Capture, Received, Request};
use crate::device::{kept, kept_list};
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

/// One maker request sent while detecting: the ports its identity names (`ports`, empty for
/// all), and what says a port is its maker's: the USB ids and the port names of the identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Probe {
    pub request: Vec<u8>,
    pub ports: Vec<String>,
    /// The (vendor, product) USB ids of the identity.
    pub usb: Vec<(u16, u16)>,
    /// The port names of the identity, compared to a port's name whatever its `to`.
    pub match_ports: Vec<String>,
}

impl Probe {
    /// The request goes to `port` without the person's consent: a port whose USB ids were read
    /// is matched by them alone; a port with none, by its name. A probe with neither is never safe.
    pub fn safe_for(&self, port: &str, usb: Option<(u16, u16)>) -> bool {
        match usb {
            Some(ids) => self.usb.contains(&ids),
            None => port_matches(&self.match_ports, port),
        }
    }
}

/// What one output port receives while detecting: the requests sent (the universal one first)
/// and the maker requests held back for want of the person's consent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSends {
    pub port: String,
    pub sent: Vec<Vec<u8>>,
    pub held: Vec<Vec<u8>>,
}

/// The requests each of `ports` (name, USB ids read) receives: the universal request, the maker
/// requests safe for it, and every other one when its index is in `consent`. Computed once,
/// before any send.
pub fn send_plan(ports: &[(String, Option<(u16, u16)>)], probes: &[Probe], consent: &[usize]) -> Vec<PortSends> {
    ports.iter().enumerate().map(|(i, (port, usb))| {
        let mut sent = vec![IDENTITY_REQUEST.to_vec()];
        let mut held: Vec<Vec<u8>> = Vec::new();
        for p in probes {
            if p.safe_for(port, *usb) || consent.contains(&i) {
                if !sent.contains(&p.request) { sent.push(p.request.clone()); }
            } else if !held.contains(&p.request) {
                held.push(p.request.clone());
            }
        }
        held.retain(|h| !sent.contains(h));
        PortSends { port: port.clone(), sent, held }
    }).collect()
}

/// The ports a consent names, by index: each fragment is an exact index, else a part of one
/// port's name, any case. A fragment naming several ports or none is refused, naming them.
pub fn consent_indexes(names: &[String], fragments: &[String]) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for f in fragments {
        let at = match f.trim().parse::<usize>() {
            Ok(n) if n < names.len() => vec![n],
            _ => {
                let low = f.to_lowercase();
                names.iter().enumerate().filter(|(_, n)| !low.is_empty() && n.to_lowercase().contains(&low)).map(|(i, _)| i).collect()
            }
        };
        match at.as_slice() {
            [one] => if !out.contains(one) { out.push(*one) },
            [] => return Err(format!("consent {f:?} names no port; the output ports are: {}",
                names.iter().enumerate().map(|(i, n)| format!("{i}: {n}")).collect::<Vec<_>>().join(", "))),
            many => return Err(format!("consent {f:?} names several ports ({}); name one, or give its index",
                many.iter().map(|&i| format!("{i}: {}", names[i])).collect::<Vec<_>>().join(", "))),
        }
    }
    Ok(out)
}

/// The USB ids read for each output port of `capture`, once.
pub fn output_usbs(capture: &Capture) -> Vec<(String, Option<(u16, u16)>)> {
    let ids: Vec<(u32, &str)> = capture.usbids.iter().map(|u| (u.card, u.text.as_str())).collect();
    let cards = usb_cards(&capture.cards, &ids);
    capture.outputs.iter().map(|n| (n.clone(), usb_of_port(n, &cards))).collect()
}

/// The request `id` names, its `{dev}` set to the device-id rule's default;
/// None for the Universal Identity Request, which every scan sends.
pub fn probe_of(id: &osd::Identity) -> Result<Option<Probe>> {
    let Some(tokens) = kept(&id.request) else { return Ok(None) };
    let dev = kept(&id.device_id).and_then(|d| d.default).unwrap_or(0x7F) as u8;
    let mut bind = crate::frame::Bindings::new();
    bind.set_u7("dev", dev);
    let bytes = crate::frame::build_message(&crate::frame::expand_identity(id, tokens), &bind)
        .map_err(|e| anyhow!("identity.request: {e}"))?;
    if bytes == IDENTITY_REQUEST {
        return Ok(None);
    }
    let ports = if kept(&id.to) == Some(&osd::IdentityTo::NamedPorts) { kept_list(&id.ports).to_vec() } else { vec![] };
    let usb = kept_list(&id.usb).iter().map(|u| (u.vendor_id, u.product_id)).collect();
    Ok(Some(Probe { request: bytes, ports, usb, match_ports: kept_list(&id.ports).to_vec() }))
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
    out.probes.sort_by(|a, b| (&a.request, &a.ports, &a.usb, &a.match_ports).cmp(&(&b.request, &b.ports, &b.usb, &b.match_ports)));
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
    /// The port's USB ids are among the block's `usb`.
    pub by_usb: bool,
    /// The reply answered a maker's own request, not the universal one.
    pub own_request: bool,
    /// The firmware version read from the reply.
    pub firmware: Option<String>,
}

/// Whether `id` recognises the device on `port` (whose USB ids are `usb`,
/// when known) that sent `replies`.
pub fn recognise(id: &osd::Identity, port: &str, usb: Option<(u16, u16)>, replies: &[Vec<u8>]) -> Option<Recognised> {
    let read: Vec<Vec<u8>> = match kept(&id.unpack) {
        Some(_) => replies.iter().map(|r| unpack_lsb_8to7(r)).collect(),
        None => replies.to_vec(),
    };
    let reply = kept_list(&id.reply);
    let ports = kept_list(&id.ports);
    let matching = (!reply.is_empty() || kept(&id.script).is_some()).then(|| read.iter().find(|r| reply_matches(id, r))).flatten();
    let by_port = !ports.is_empty() && port_matches(ports, port);
    let by_usb = usb.is_some_and(|(vendor, product)| kept_list(&id.usb).iter()
        .any(|u| u.vendor_id == vendor && u.product_id == product));
    if matching.is_none() && !by_port && !by_usb {
        return None;
    }
    Some(Recognised {
        by_reply: matching.is_some(),
        by_port,
        by_usb,
        own_request: matching.is_some() && probe_of(id).ok().flatten().is_some(),
        firmware: matching.and_then(|r| kept(&id.firmware).and_then(|f| decode_firmware(f, r))),
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
    let pattern = crate::frame::expand_identity(id, kept_list(&id.reply));
    reply.len() >= pattern.len()
        && pattern.iter().zip(reply).all(|(t, b)| match t {
            FrameToken::Byte(l) => l == b,
            // `any`, `device_id`, a 0.4 `{placeholder}`: one byte, whatever it is.
            _ => true,
        })
        && kept(&id.reply_text).is_none_or(|text| {
            !text.is_empty() && reply.windows(text.len()).any(|w| w == text.as_bytes())
        })
        && kept(&id.script).is_none_or(|code| {
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

/// The USB cards of an ALSA listing: from the text of `/proc/asound/cards`
/// and the `usbid` text (`vid:pid`, hex) of each card by its number, the
/// name of each card (the text after the first " - " following "]:", what ALSA
/// names the card's sequencer client; not its id between brackets), with its ids. A card
/// without `usbid` (virtual, DIN) and an unreadable line are left out.
pub fn usb_cards(cards: &str, usbids: &[(u32, &str)]) -> Vec<(String, u16, u16)> {
    cards.lines().filter_map(|line| {
        let line = line.trim_start();
        let digits = line.find(|c: char| !c.is_ascii_digit())?;
        let number: u32 = line[..digits].parse().ok()?;
        let rest = line[digits..].trim_start().strip_prefix('[')?;
        let after = rest[rest.find(']')? + 1..].trim_start().strip_prefix(':')?;
        let name = after.split_once(" - ")?.1.trim();
        let (_, text) = usbids.iter().find(|(n, _)| *n == number)?;
        let (vendor, product) = text.trim().split_once(':')?;
        let vendor = u16::from_str_radix(vendor.trim(), 16).ok()?;
        let product = u16::from_str_radix(product.trim(), 16).ok()?;
        (!name.is_empty()).then(|| (name.to_string(), vendor, product))
    }).collect()
}

/// The USB ids of the cards the ALSA client of `port` (the text before the
/// first ':') names, once squashed. Several cards of one name give their ids
/// when all share them; no card, or different ids, give none.
pub fn usb_of_port(port: &str, cards: &[(String, u16, u16)]) -> Option<(u16, u16)> {
    let client = squash(port.split(':').next().unwrap_or(port));
    if client.is_empty() {
        return None;
    }
    let mut ids = cards.iter().filter(|(name, ..)| squash(name) == client).map(|(_, v, p)| (*v, *p));
    let first = ids.next()?;
    ids.all(|i| i == first).then_some(first)
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
pub fn rank(port_name: &str, usb: Option<(u16, u16)>, replies: &[Vec<u8>], catalogue: &[Candidate], limit: usize) -> Vec<Ranked> {
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
        let recognised = c.identity.as_ref().and_then(|id| recognise(id, port_name, usb, replies));
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
            if r.by_usb { score += 80; }
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
    /// The USB ids (vendor, product) of the device's card, when it has one.
    pub usb: Option<(u16, u16)>,
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
/// Gap between two requests on one port.
const PROBE_GAP: Duration = Duration::from_millis(10);

/// List MIDI ports, pair inputs with outputs by name, and — when `probe` is
/// true — send the Identity Request, then each of `probes` (the maker
/// requests the catalogue's identity blocks name, each to its ports), on
/// every output and collect the SysEx that comes back for `wait`.
pub fn scan(probe: bool, wait: Duration, probes: &[Probe]) -> Result<Vec<Probed>> {
    Ok(replay(&observe(probe, wait, probes, &[])?))
}

/// Replay a capture: the derivation `scan` runs over what it observed, with no device and no
/// read of the system.
pub fn replay(capture: &Capture) -> Vec<Probed> {
    derive(capture)
}

/// The derivation, pure: pair each output with the input of the same (or most similar) name,
/// read the USB ids of the cards, and give each port the SysEx it received, with the identity
/// it carries.
fn derive(capture: &Capture) -> Vec<Probed> {
    let (out_names, in_names) = (&capture.outputs, &capture.inputs);
    let mut devices: Vec<Probed> = Vec::new();
    let mut used_in: HashSet<usize> = HashSet::new();
    // First pass: every output takes the unused input of its full name (lowest index first).
    let mut paired: Vec<Option<usize>> = out_names.iter().map(|oname| {
        let ii = in_names.iter().enumerate()
            .find(|(ii, iname)| *iname == oname && !used_in.contains(ii)).map(|(ii, _)| ii)?;
        used_in.insert(ii);
        Some(ii)
    }).collect();
    // Second pass: the outputs left take the nearest name; ties go to the lowest input.
    for (oi, oname) in out_names.iter().enumerate() {
        if paired[oi].is_some() { continue; }
        let ot = tokens(oname);
        let best = in_names.iter().enumerate()
            .filter(|(ii, _)| !used_in.contains(ii))
            .map(|(ii, iname)| (ot.intersection(&tokens(iname)).count(), std::cmp::Reverse(ii)))
            .filter(|(n, _)| *n > 0)
            .max();
        if let Some((_, std::cmp::Reverse(ii))) = best {
            used_in.insert(ii);
            paired[oi] = Some(ii);
        }
    }
    for (oi, oname) in out_names.iter().enumerate() {
        let in_index = paired[oi];
        devices.push(Probed {
            out_index: Some(oi),
            out_name: Some(oname.clone()),
            in_index,
            in_name: in_index.map(|ii| in_names[ii].clone()),
            identity: None,
            usb: None,
            replies: Vec::new(),
        });
    }
    for (ii, iname) in in_names.iter().enumerate() {
        if !used_in.contains(&ii) {
            devices.push(Probed { out_index: None, out_name: None, in_index: Some(ii),
                                  in_name: Some(iname.clone()), identity: None, usb: None, replies: Vec::new() });
        }
    }
    let ids: Vec<(u32, &str)> = capture.usbids.iter().map(|u| (u.card, u.text.as_str())).collect();
    let cards = usb_cards(&capture.cards, &ids);
    for d in devices.iter_mut() {
        d.usb = d.out_name.as_deref().and_then(|n| usb_of_port(n, &cards))
            .or_else(|| d.in_name.as_deref().and_then(|n| usb_of_port(n, &cards)));
    }
    for r in &capture.received {
        if r.bytes.first() != Some(&0xF0) { continue; }
        let Some(ii) = in_names.iter().position(|n| *n == r.port) else { continue };
        if let Some(d) = devices.iter_mut().find(|d| d.in_index == Some(ii)) {
            if d.replies.len() >= REPLIES_KEPT { continue; }
            if d.identity.is_none() { d.identity = parse_identity_reply(&r.bytes); }
            if !d.replies.contains(&r.bytes) { d.replies.push(r.bytes.clone()); }
        }
    }
    devices
}

/// Observe the system, raw: the ports, the listing of its cards and, when `probe` is true, the
/// requests sent (the Identity Request, then each of `probes` to its ports) with the SysEx
/// received for `wait`.
pub fn observe(probe: bool, wait: Duration, probes: &[Probe], consent: &[String]) -> Result<Capture> {
    let output = MidiOutput::new("osc-bridge-detect")?;
    let out_ports = output.ports();
    let out_names: Vec<String> = out_ports.iter()
        .map(|p| output.port_name(p).unwrap_or_else(|_| "?".into())).collect();
    let input = MidiInput::new("osc-bridge-detect")?;
    let in_ports = input.ports();
    let in_names: Vec<String> = in_ports.iter()
        .map(|p| input.port_name(p).unwrap_or_else(|_| "?".into())).collect();
    let mut capture = Capture::new(out_names.clone(), in_names.clone(), Asound::machine());
    if !probe {
        return Ok(capture);
    }
    // The split of the requests per port, before anything is sent: a bad consent stops here.
    let plan = send_plan(&output_usbs(&capture), probes, &consent_indexes(&out_names, consent).map_err(|e| anyhow!(e))?);

    // Listen on every input, then ask every output who it is.
    let start = std::time::Instant::now();
    let heard: Arc<Mutex<Vec<Received>>> = Arc::new(Mutex::new(Vec::new()));
    let mut listeners: Vec<MidiInputConnection<()>> = Vec::new();
    for (ii, port) in in_ports.iter().enumerate() {
        let mut inp = MidiInput::new("osc-bridge-detect-in")?;
        inp.ignore(Ignore::None);
        let sink = heard.clone();
        let name = in_names[ii].clone();
        if let Ok(conn) = inp.connect(port, "osc-bridge-detect-in", move |_, msg, _| {
            if msg.first() == Some(&0xF0) {
                if let Ok(mut g) = sink.lock() {
                    if g.iter().filter(|r| r.port == name).count() < REPLIES_KEPT {
                        g.push(Received { port: name.clone(), bytes: msg.to_vec(), ms: start.elapsed().as_millis() as u64 });
                    }
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
    let rounds = plan.iter().map(|p| p.sent.len()).max().unwrap_or(0);
    for round in 0..rounds {
        for (oi, conn) in conns.iter_mut() {
            if let Some(bytes) = plan[*oi].sent.get(round) {
                if conn.send(bytes).is_ok() {
                    capture.requests.push(Request { port: out_names[*oi].clone(), bytes: bytes.clone() });
                }
            }
        }
        std::thread::sleep(PROBE_GAP);
    }
    std::thread::sleep(wait);
    drop(listeners);

    capture.received = heard.lock().map_err(|_| anyhow!("detect replies lock poisoned"))?.clone();
    Ok(capture)
}

// ---- the learn mode: a fingerprint of a device that may not say who it is ----

/// Listening time of the learn mode, in ms: default, and the interval accepted. Wider than
/// `timeout_ms` (100-5000) because a person needs the time to move controls.
const LEARN_DEFAULT_MS: u64 = 5000;
const LEARN_MIN_MS: u64 = 1000;
const LEARN_MAX_MS: u64 = 30000;
/// Distinct controls a fingerprint keeps (the schema's `maxItems`).
const CONTROLS_KEPT: usize = 32;

/// The ALSA client number of a raw port name: the `N` of its final `N:M`.
fn client_number(name: &str) -> Option<u32> {
    let last = name.split_whitespace().next_back()?;
    let (n, m) = last.split_once(':')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(n) && digits(m)).then(|| n.parse().ok()).flatten()
}

/// The words of a raw port name, the only writer of them: what follows the first `:`, without the
/// final `N:M`, cut into words on every character that is neither a letter nor a digit, lower
/// case, joined by a space; a run of words repeated at once is kept once.
pub fn port_words(name: &str) -> String {
    let trimmed = name.trim_end();
    let body = match (client_number(trimmed), trimmed.rfind(char::is_whitespace)) {
        (Some(_), Some(i)) => &trimmed[..i],
        (Some(_), None) => "",
        _ => trimmed,
    };
    let body = body.split_once(':').map_or(body, |(_, rest)| rest);
    let mut words: Vec<String> = body.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
    'again: loop {
        for k in 1..=words.len() / 2 {
            for i in 0..=words.len() - 2 * k {
                if words[i..i + k] == words[i + k..i + 2 * k] {
                    words.drain(i + k..i + 2 * k);
                    continue 'again;
                }
            }
        }
        break;
    }
    words.join(" ")
}

/// One physical device as its ports show it: all its inputs and outputs (raw names, with their
/// indexes in the capture) and the USB ids of its card, when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub in_indexes: Vec<usize>,
    pub out_indexes: Vec<usize>,
    pub usb: Option<(u16, u16)>,
}

impl Unit {
    /// A name for the device: its first output, else its first input.
    pub fn label(&self) -> &str {
        self.outputs.first().or(self.inputs.first()).map(String::as_str).unwrap_or("?")
    }
}

/// The devices of a capture, pure: the ports of one ALSA client number are one device; a name
/// without number groups by its client name (the text before the first `:`, squashed). The card
/// only gives the USB ids.
pub fn units(capture: &Capture) -> Vec<Unit> {
    #[derive(PartialEq)]
    enum Key { Number(u32), Name(String) }
    let key = |n: &str| match client_number(n) {
        Some(c) => Key::Number(c),
        None => Key::Name(squash(n.split(':').next().unwrap_or(n))),
    };
    let ids: Vec<(u32, &str)> = capture.usbids.iter().map(|u| (u.card, u.text.as_str())).collect();
    let cards = usb_cards(&capture.cards, &ids);
    let mut found: Vec<(Key, Unit)> = Vec::new();
    let all = capture.outputs.iter().enumerate().map(|(i, n)| (true, i, n))
        .chain(capture.inputs.iter().enumerate().map(|(i, n)| (false, i, n)));
    for (is_out, index, name) in all {
        let k = key(name);
        let at = match found.iter().position(|(f, _)| *f == k) {
            Some(at) => at,
            None => {
                found.push((k, Unit { inputs: vec![], outputs: vec![], in_indexes: vec![], out_indexes: vec![], usb: None }));
                found.len() - 1
            }
        };
        let u = &mut found[at].1;
        if is_out { u.outputs.push(name.clone()); u.out_indexes.push(index); }
        else { u.inputs.push(name.clone()); u.in_indexes.push(index); }
    }
    found.into_iter().map(|(_, mut u)| {
        u.usb = u.outputs.iter().chain(&u.inputs).find_map(|n| usb_of_port(n, &cards));
        u
    }).collect()
}

/// One message a device sent on one of its inputs, with the time since the request went out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub port: String,
    pub at: Duration,
    pub msg: Vec<u8>,
}

/// What the learn mode reads and writes of one device: the universal identity request goes
/// out on each output, then everything every input sends is heard for the time given.
pub trait Wire {
    fn send(&mut self, port: &str, msg: &[u8]);
    fn heard(&mut self, listen: Duration) -> Vec<Heard>;
}

/// The fingerprint of a device (a valid OSD 0.6 fingerprint) and, beside it, what was measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Learned {
    pub fingerprint: serde_json::Value,
    /// `identity_reply_ms` with `identity_reply_port`, and `periods_ms` (`clock`, `active_sensing`)
    /// with `periods_port`: local measures, absent when not observed.
    pub measured: serde_json::Value,
}

/// The listening time asked (ms), or the default; refused outside 1000-30000 ms.
pub fn learn_listen(ms: Option<u64>) -> Result<Duration, String> {
    let ms = ms.unwrap_or(LEARN_DEFAULT_MS);
    if (LEARN_MIN_MS..=LEARN_MAX_MS).contains(&ms) {
        Ok(Duration::from_millis(ms))
    } else {
        Err(format!("listen_ms {ms} is out of range: the learn mode listens {LEARN_MIN_MS}-{LEARN_MAX_MS} ms (default {LEARN_DEFAULT_MS})."))
    }
}

/// What the person is told before the listening starts.
pub fn learn_notice(listen: Duration) -> String {
    let ms = listen.as_millis();
    let secs = if ms.is_multiple_of(1000) { (ms / 1000).to_string() } else { format!("{}", ms as f64 / 1000.0) };
    format!("Learn mode: listening for {secs} seconds. You can move the controls of the device now (knobs, keys, sliders). \
        Nothing triggers the device for you: only the universal identity request is sent.")
}

fn hex_spaced(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// The mean gap between the messages of one kind (ms); none under two messages.
fn period_ms(times: &[Duration]) -> Option<f64> {
    if times.len() < 2 { return None; }
    let span = *times.last()? - *times.first()?;
    Some(span.as_secs_f64() * 1000.0 / (times.len() - 1) as f64)
}

/// Send the universal identity request to a device, listen for `listen`, and write down what
/// was heard: only what was heard, never a guess.
pub fn learn(device: &Unit, wire: &mut dyn Wire, listen: Duration, observed: &str) -> Learned {
    for out in &device.outputs { wire.send(out, &IDENTITY_REQUEST); }
    let heard = wire.heard(listen);

    let mut fp = serde_json::Map::new();
    fp.insert("$schema".into(), "https://w3id.org/osd/0.6/fingerprint.schema.json".into());
    fp.insert("observed".into(), observed.into());
    fp.insert("via".into(), "native".into());
    // One entry per port, in the order the system lists them; a name may repeat.
    let words = |names: &[String]| names.iter().map(|n| port_words(n)).collect::<Vec<_>>();
    let mut ports = serde_json::Map::new();
    if !device.inputs.is_empty() { ports.insert("inputs".into(), words(&device.inputs).into()); }
    if !device.outputs.is_empty() { ports.insert("outputs".into(), words(&device.outputs).into()); }
    fp.insert("ports".into(), ports.into());
    if let Some((vendor_id, product_id)) = device.usb {
        fp.insert("usb".into(), serde_json::json!({ "vendor_id": vendor_id, "product_id": product_id }));
    }

    let mut measured = serde_json::Map::new();
    if let Some(h) = heard.iter().find(|h| parse_identity_reply(&h.msg).is_some()) {
        fp.insert("identity_reply".into(), hex_spaced(&h.msg).into());
        measured.insert("identity_reply_ms".into(), (h.at.as_secs_f64() * 1000.0).into());
        measured.insert("identity_reply_port".into(), h.port.clone().into());
    }

    let times = |status: u8| -> Vec<Duration> {
        heard.iter().filter(|h| h.msg.as_slice() == [status]).map(|h| h.at).collect()
    };
    // A period is measured on one port: the first input of the device that sent the status twice.
    let period_on = |status: u8| -> Option<(f64, String)> {
        device.inputs.iter().find_map(|port| {
            let t: Vec<Duration> = heard.iter().filter(|h| &h.port == port && h.msg.as_slice() == [status]).map(|h| h.at).collect();
            period_ms(&t).map(|p| (p, port.clone()))
        })
    };
    let (clock, sensing) = (times(0xF8), times(0xFE));
    let mut sends = serde_json::Map::new();
    if !clock.is_empty() { sends.insert("clock".into(), true.into()); }
    if !sensing.is_empty() { sends.insert("active_sensing".into(), true.into()); }
    let note_end = heard.iter().find_map(|h| match h.msg.as_slice() {
        [s, _, _] if s & 0xF0 == 0x80 => Some("note_off"),
        [s, _, 0] if s & 0xF0 == 0x90 => Some("note_on_velocity_0"),
        _ => None,
    });
    if let Some(end) = note_end { sends.insert("note_off".into(), end.into()); }
    if !sends.is_empty() { fp.insert("sends".into(), sends.into()); }

    let controls = controls_heard(&heard);
    if !controls.is_empty() { fp.insert("controls".into(), controls.into()); }

    let mut periods = serde_json::Map::new();
    let mut periods_port = serde_json::Map::new();
    for (key, status) in [("clock", 0xF8), ("active_sensing", 0xFE)] {
        if let Some((p, port)) = period_on(status) {
            periods.insert(key.into(), p.into());
            periods_port.insert(key.into(), port.into());
        }
    }
    if !periods.is_empty() {
        measured.insert("periods_ms".into(), periods.into());
        measured.insert("periods_port".into(), periods_port.into());
    }

    Learned { fingerprint: fp.into(), measured: measured.into() }
}

/// The controls moved, by type, channel and number (no value), each once, the first 32 in
/// arrival order: a controller, or a parameter (NRPN) once its value was sent.
fn controls_heard(heard: &[Heard]) -> Vec<serde_json::Value> {
    let mut found: Vec<serde_json::Value> = Vec::new();
    let mut selected: [(Option<u8>, Option<u8>); 16] = [(None, None); 16];
    for h in heard {
        let [status, number, value] = h.msg.as_slice() else { continue };
        if status & 0xF0 != 0xB0 { continue; }
        let ch = (status & 0x0F) as usize;
        let control = match *number {
            99 => { selected[ch].0 = Some(*value); continue; }
            98 => { selected[ch].1 = Some(*value); continue; }
            6 | 38 if selected[ch].0.is_some() && selected[ch].1.is_some() => {
                let (msb, lsb) = (selected[ch].0, selected[ch].1);
                serde_json::json!({ "nrpn_msb": msb, "nrpn_lsb": lsb, "channel": ch })
            }
            100 | 101 | 120..=127 => continue,
            cc => serde_json::json!({ "cc": cc, "channel": ch }),
        };
        if found.len() < CONTROLS_KEPT && !found.contains(&control) { found.push(control); }
    }
    found
}

/// A device's ports on the machine: every input listened to, every output asked.
struct MidiWire {
    outs: Vec<(String, midir::MidiOutputConnection)>,
    _inputs: Vec<MidiInputConnection<()>>,
    got: Arc<Mutex<Vec<Heard>>>,
    since: Arc<Mutex<Option<std::time::Instant>>>,
}

impl MidiWire {
    fn open(device: &Unit) -> Result<Self> {
        let got: Arc<Mutex<Vec<Heard>>> = Arc::new(Mutex::new(Vec::new()));
        let since: Arc<Mutex<Option<std::time::Instant>>> = Arc::new(Mutex::new(None));
        let mut inputs = Vec::new();
        for (ii, name) in device.in_indexes.iter().zip(&device.inputs) {
            let mut inp = MidiInput::new("osc-bridge-learn-in")?;
            inp.ignore(Ignore::None);
            if let Some(port) = inp.ports().get(*ii).cloned() {
                let (sink, t0, port_name) = (got.clone(), since.clone(), name.clone());
                if let Ok(conn) = inp.connect(&port, "osc-bridge-learn-in", move |_, msg, _| {
                    let at = t0.lock().ok().and_then(|t| t.map(|t| t.elapsed())).unwrap_or_default();
                    if let Ok(mut g) = sink.lock() { g.push(Heard { port: port_name.clone(), at, msg: msg.to_vec() }); }
                }, ()) {
                    inputs.push(conn);
                }
            }
        }
        let mut outs = Vec::new();
        for (oi, name) in device.out_indexes.iter().zip(&device.outputs) {
            let o = MidiOutput::new("osc-bridge-learn-out")?;
            if let Some(port) = o.ports().get(*oi) {
                if let Ok(conn) = o.connect(port, "osc-bridge-learn-out") { outs.push((name.clone(), conn)); }
            }
        }
        Ok(MidiWire { outs, _inputs: inputs, got, since })
    }
}

impl Wire for MidiWire {
    fn send(&mut self, port: &str, msg: &[u8]) {
        if let Ok(mut t) = self.since.lock() { t.get_or_insert_with(std::time::Instant::now); }
        if let Some((_, out)) = self.outs.iter_mut().find(|(n, _)| n == port) { let _ = out.send(msg); }
    }
    fn heard(&mut self, listen: Duration) -> Vec<Heard> {
        std::thread::sleep(listen);
        self.got.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
    }
}

/// Today as `YYYY-MM-DD` (UTC).
fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86400) as i64).unwrap_or(0);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The learn mode on every device plugged in: each is asked the universal identity request on
/// each of its outputs and heard on each of its inputs for `listen`, all at the same time.
pub fn learn_all(listen: Duration) -> Result<Vec<(Unit, Learned)>> {
    let devices = units(&observe(false, Duration::ZERO, &[], &[])?);
    let observed = today();
    std::thread::scope(|s| {
        let tasks: Vec<_> = devices.iter().map(|d| {
            let observed = observed.as_str();
            s.spawn(move || MidiWire::open(d).map(|mut w| learn(d, &mut w, listen, observed)))
        }).collect();
        devices.iter().zip(tasks).map(|(d, t)| {
            let learned = t.join().map_err(|_| anyhow!("learn thread panicked"))??;
            Ok((d.clone(), learned))
        }).collect()
    })
}

#[cfg(test)]
mod tests;
