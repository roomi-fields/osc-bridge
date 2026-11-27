//! The reading of an identity: the request a driver's identity block names and the reading
//! of the replies a device sends back. This module opens no port and reads no capture; `detect`
//! reads it for the ports, the link for the catalogue.
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
use crate::device::{kept, kept_list};
use osd::{FirmwareRule, FrameToken, PortRule};
use std::sync::LazyLock;
use std::collections::HashSet;

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
/// still match drivers that declare the same `manufacturerId`.
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

/// Which of two cards of one device comes first: a rank per source tier, lower first. A value the
/// caller gives, as a transport gives its congestion controller to `quinn-proto`: this module knows
/// no tier. A bare function pointer, since the order is a pure comparison and carries no state.
pub type TierOrder = fn(&str) -> u8;

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

/// Why `request`, received from the service, is no maker request this copy may hold: it must be
/// one SysEx message (F0, F7, no status byte between) of at most `MAKER_REQUEST_MAX` bytes.
pub fn maker_request_fault(request: &[u8]) -> Option<String> {
    let hex = request.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
    if request.first() != Some(&0xF0) || request.last() != Some(&0xF7) || request.len() < 2 {
        return Some(format!("a maker request must start with F0 and end with F7: refused {hex}"));
    }
    if request[1..request.len() - 1].iter().any(|b| *b >= 0x80) {
        return Some(format!("a maker request must be one SysEx message, with no status byte between F0 and F7: refused {hex}"));
    }
    if request.len() > MAKER_REQUEST_MAX {
        return Some(format!("a maker request is at most {MAKER_REQUEST_MAX} bytes, this one has {}: refused {hex}", request.len()));
    }
    None
}

/// The longest maker request a copy accepts from the service.
pub const MAKER_REQUEST_MAX: usize = 32;

/// What one output port receives while detecting: the requests sent (the universal one first)
/// and the maker requests held back for want of the person's consent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSends {
    pub port: String,
    pub sent: Vec<Vec<u8>>,
    pub held: Vec<Vec<u8>>,
}

/// The requests each of `ports` (name, USB ids read) receives: the universal request, the maker
/// requests of the makers only when its index is in `consent`, whatever designates the port
/// (its USB ids included, which the plan does not read). Computed once, before any send.
pub fn send_plan(ports: &[(String, Option<(u16, u16)>)], probes: &[Probe], consent: &[usize]) -> Vec<PortSends> {
    ports.iter().enumerate().map(|(i, (port, _usb))| {
        let mut sent = vec![IDENTITY_REQUEST.to_vec()];
        let mut held: Vec<Vec<u8>> = Vec::new();
        for p in probes {
            if consent.contains(&i) {
                if !sent.contains(&p.request) { sent.push(p.request.clone()); }
            } else if !held.contains(&p.request) {
                held.push(p.request.clone());
            }
        }
        held.retain(|h| !sent.contains(h));
        PortSends { port: port.clone(), sent, held }
    }).collect()
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
    // A probe holds the texts of the port rules.
    let match_ports: Vec<String> = kept_list(&id.ports).iter().map(|r| r.value.clone()).collect();
    let ports = if kept(&id.to) == Some(&osd::IdentityTo::NamedPorts) { match_ports.clone() } else { vec![] };
    let usb = kept_list(&id.usb).iter().map(|u| (u.vendor_id, u.product_id)).collect();
    Ok(Some(Probe { request: bytes, ports, usb, match_ports }))
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

/// Whether one of the port rules of a driver names a port of this machine whose
/// names are `inputs` and `output`, as `osd` judges them: case ignored, a rule of
/// another platform or of a naming stack (the machine's is not known) naming
/// nothing. Without `inputs`, a rule of the input direction names nothing.
pub(crate) fn rules_match(rules: &[PortRule], inputs: Option<&str>, output: &str) -> bool {
    // The signs `osd` judges are those of a device and of a fingerprint: blank
    // ones, given the rules and the names.
    static BLANK: LazyLock<Option<(osd::Device, osd::Fingerprint)>> = LazyLock::new(|| {
        let blank = |text: &str, kind| osd::read(text, kind).ok();
        let device = blank(r#"{"$schema":"https://w3id.org/osd/0.7/device.schema.json",
            "device":{"name":"-","osc_prefix":"/-"},"identity":{"ports":[]}}"#, osd::Kind::Device)?;
        let fingerprint = blank(r#"{"$schema":"https://w3id.org/osd/0.7/fingerprint.schema.json","observed":"1970-01-01",
            "device":{"vendor":"-","model":"-","firmware":"-"},"ports":{}}"#, osd::Kind::Fingerprint)?;
        match (device, fingerprint) {
            (osd::Descriptor::Device(d), osd::Descriptor::Fingerprint(f)) => Some((*d, *f)),
            _ => None,
        }
    });
    let Some((mut device, mut fingerprint)) = BLANK.clone() else { return false };
    let Some(identity) = device.identity.as_mut() else { return false };
    identity.ports = Some(Some(rules.to_vec()));
    fingerprint.platform = machine_platform();
    if let Some(ports) = fingerprint.ports.as_mut() {
        ports.inputs = inputs.map(|name| vec![name.to_string()]);
        ports.outputs = Some(vec![output.to_string()]);
    }
    osd::match_identity(&device, &fingerprint).iter().any(|m| m.verdict == osd::MatchVerdict::Matched)
}

/// The platform of this machine, in OSD's words.
fn machine_platform() -> Option<osd::Platform> {
    if cfg!(target_os = "windows") {
        Some(osd::Platform::Windows)
    } else if cfg!(target_os = "macos") {
        Some(osd::Platform::Macos)
    } else if cfg!(target_os = "linux") {
        Some(osd::Platform::Linux)
    } else {
        None
    }
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
    let by_port = !ports.is_empty() && rules_match(ports, Some(port), port);
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

/// The words of a name that identify a device (the `NOISE` words and bare numbers left out).
/// `detect` reads it to pair an input with its output.
pub fn tokens(s: &str) -> HashSet<String> {
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

/// Collapse "Minilab3", "mini lab 3", "MiniLab-3" to one key.
/// `detect` reads it to match a card name to a port's client.
pub fn squash(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// Words too common to name a device: with the `NOISE` words, a name made only of them is no proof.
const COMMON: &[&str] = &["synth", "control", "controller", "mini"];

/// Like `tokens`, without the common words.
fn name_tokens(s: &str) -> HashSet<String> {
    tokens(s).into_iter().filter(|t| !COMMON.contains(&t.as_str())).collect()
}

/// What a proof is worth: a strong proof recognises the device, a medium one points at its
/// maker, a weak one (the name) only suggests it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nature { Strong, Medium, Weak }

impl Nature {
    pub fn as_str(self) -> &'static str {
        match self { Nature::Strong => "strong", Nature::Medium => "medium", Nature::Weak => "weak" }
    }
}

/// One proof that a candidate is the device of a port: its nature and what it is
/// (`identity reply`, `usb id`, `maker`, `name`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proof {
    pub nature: Nature,
    pub kind: &'static str,
}

/// A candidate driver for a port: its score, its place in the catalogue,
/// what recognised it and the proofs it rests on.
#[derive(Debug, Clone)]
pub struct Ranked {
    pub score: u32,
    pub index: usize,
    pub recognised: Option<Recognised>,
    pub proofs: Vec<Proof>,
    /// The other recognised candidates with the same score (indexes in the
    /// catalogue), which the device's replies match as well.
    pub equally: Vec<usize>,
}

impl Ranked {
    /// Whether one of its proofs is strong.
    pub fn is_strong(&self) -> bool {
        self.proofs.iter().any(|p| p.nature == Nature::Strong)
    }
}

/// Score every candidate against a port name and the SysEx the device sent
/// back while detected (its Identity Reply, its answers to maker requests);
/// return the best ones, highest score first (then the most trustworthy
/// source). A driver whose identity block
/// recognises the device ranks first, the one for the firmware it reported
/// above the others; drivers matching neither the name nor the identity are
/// dropped.
pub fn rank(port_name: &str, usb: Option<(u16, u16)>, replies: &[Vec<u8>], catalogue: &[Candidate], limit: usize, order: TierOrder) -> Vec<Ranked> {
    let port_tokens = name_tokens(port_name);
    let port_squashed = squash(port_name);
    let identity = replies.iter().find_map(|r| parse_identity_reply(r));
    let id_vendor = identity.as_ref().and_then(|i| manufacturer_name(&i.manufacturer)).map(squash);
    let mut scored: Vec<Ranked> = catalogue.iter().enumerate().filter_map(|(i, c)| {
        let mut score = 0u32;
        let card_tokens = name_tokens(&c.name);
        score += 10 * card_tokens.intersection(&port_tokens).count() as u32;
        // The model name, with or without the vendor in front ("Arturia
        // MiniLab 3" vs port "Minilab3 MIDI"), or the driver slug. A key made only of
        // common words names nothing.
        let vendor_sq = squash(&c.vendor);
        let full = squash(&c.name);
        let model = full.strip_prefix(vendor_sq.as_str()).filter(|m| !m.is_empty()).unwrap_or(&full).to_string();
        let slug = squash(&c.slug);
        let name_names = !card_tokens.is_empty();
        let slug_names = !name_tokens(&c.slug).is_empty();
        // A longer match is a more specific one: "minilab3" beats "mini".
        if let Some(len) = [(full, name_names), (model, name_names), (slug, slug_names)].iter()
            .filter(|(k, names)| *names && k.len() >= 3 && port_squashed.contains(k.as_str()))
            .map(|(k, _)| k.len()).max()
        {
            score += 10 + 3 * len as u32;
        }
        let by_name = score > 0;
        let recognised = c.identity.as_ref().and_then(|id| recognise(id, port_name, usb, replies));
        if !by_name && recognised.is_none() {
            return None;
        }
        let vendor = vendor_sq;
        if !vendor.is_empty() && port_squashed.contains(&vendor) {
            score += 5;
        }
        let mut proofs = Vec::new();
        if let Some(r) = &recognised {
            if r.by_reply { proofs.push(Proof { nature: Nature::Strong, kind: "identity reply" }); }
            if r.by_usb { proofs.push(Proof { nature: Nature::Strong, kind: "usb id" }); }
        }
        if let Some(id) = &identity {
            if !c.manufacturer_id.is_empty() && c.manufacturer_id.starts_with(&id.manufacturer) {
                score += 40;
                proofs.push(Proof { nature: Nature::Medium, kind: "maker" });
            } else if id_vendor.as_deref().is_some_and(|v| vendor.contains(v) || v.contains(&vendor)) {
                score += 20;
                proofs.push(Proof { nature: Nature::Medium, kind: "maker" });
            }
        }
        if by_name || recognised.as_ref().is_some_and(|r| r.by_port) {
            proofs.push(Proof { nature: Nature::Weak, kind: "name" });
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
        Some(Ranked { score, index: i, recognised, proofs, equally: vec![] })
    }).collect();
    // Equal scores: the driver the others share their MIDI with first (they
    // are built on it), then the most trustworthy source, as a slug resolves.
    let shared: HashSet<(u32, &str)> = scored.iter()
        .filter_map(|r| catalogue[r.index].midi_of.as_deref().map(|m| (r.score, m))).collect();
    let builds_others = |r: &Ranked| !shared.contains(&(r.score, catalogue[r.index].id.as_str()));
    scored.sort_by(|a, b| b.score.cmp(&a.score)
        .then_with(|| builds_others(a).cmp(&builds_others(b)))
        .then_with(|| order(&catalogue[a.index].tier).cmp(&order(&catalogue[b.index].tier)))
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

#[cfg(test)]
mod tests;
