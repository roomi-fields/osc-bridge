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
    pub slug: String,
    pub name: String,
    pub vendor: String,
    pub file: String,
    pub tier: String,
    pub manufacturer_id: Vec<u8>,
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

/// Collapse "Minilab3", "mini lab 3", "MiniLab-3" to one key.
fn squash(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// Score every candidate against a port name and an optional identity;
/// return the best ones, highest score first. Zero-score drivers are dropped.
pub fn rank(port_name: &str, identity: Option<&Identity>, catalogue: &[Candidate], limit: usize) -> Vec<(u32, usize)> {
    let port_tokens = tokens(port_name);
    let port_squashed = squash(port_name);
    let id_vendor = identity.and_then(|i| manufacturer_name(&i.manufacturer)).map(squash);
    let mut scored: Vec<(u32, usize)> = catalogue.iter().enumerate().filter_map(|(i, c)| {
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
        if score == 0 {
            return None;
        }
        let vendor = vendor_sq;
        if !vendor.is_empty() && port_squashed.contains(&vendor) {
            score += 5;
        }
        if let Some(id) = identity {
            if !c.manufacturer_id.is_empty() && c.manufacturer_id.starts_with(&id.manufacturer) {
                score += 40;
            } else if id_vendor.as_deref().is_some_and(|v| vendor.contains(v) || v.contains(&vendor)) {
                score += 20;
            }
        }
        Some((score, i))
    }).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| catalogue[a.1].slug.cmp(&catalogue[b.1].slug)));
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
}

impl Probed {
    pub fn label(&self) -> &str {
        self.out_name.as_deref().or(self.in_name.as_deref()).unwrap_or("?")
    }
}

/// List MIDI ports, pair inputs with outputs by name, and — when `probe` is
/// true — send the Identity Request on every output and collect replies for
/// `wait`.
pub fn scan(probe: bool, wait: Duration) -> Result<Vec<Probed>> {
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
        });
    }
    for (ii, iname) in in_names.iter().enumerate() {
        if !used_in.contains(&ii) {
            devices.push(Probed { out_index: None, out_name: None, in_index: Some(ii),
                                  in_name: Some(iname.clone()), identity: None });
        }
    }
    if !probe {
        return Ok(devices);
    }

    // Listen on every input, then ask every output who it is.
    let replies: Arc<Mutex<Vec<(usize, Identity)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut listeners: Vec<MidiInputConnection<()>> = Vec::new();
    for (ii, port) in in_ports.iter().enumerate() {
        let mut inp = MidiInput::new("osc-bridge-detect-in")?;
        inp.ignore(Ignore::None);
        let sink = replies.clone();
        if let Ok(conn) = inp.connect(port, "osc-bridge-detect-in", move |_, msg, _| {
            if let Some(id) = parse_identity_reply(msg) {
                if let Ok(mut g) = sink.lock() { g.push((ii, id)); }
            }
        }, ()) {
            listeners.push(conn);
        }
    }
    for port in &out_ports {
        let out = MidiOutput::new("osc-bridge-detect-out")?;
        if let Ok(mut conn) = out.connect(port, "osc-bridge-detect-out") {
            let _ = conn.send(&IDENTITY_REQUEST);
        }
    }
    std::thread::sleep(wait);
    drop(listeners);

    let got = replies.lock().map_err(|_| anyhow!("identity replies lock poisoned"))?.clone();
    for (ii, id) in got {
        if let Some(d) = devices.iter_mut().find(|d| d.in_index == Some(ii)) {
            d.identity.get_or_insert(id);
        }
    }
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(slug: &str, name: &str, vendor: &str, mfr: &[u8]) -> Candidate {
        Candidate { slug: slug.into(), name: name.into(), vendor: vendor.into(),
                    file: format!("devices/{slug}.json"), tier: "pencilresearch".into(),
                    manufacturer_id: mfr.to_vec() }
    }

    #[test]
    fn parses_three_byte_manufacturer_reply() {
        // Arturia, family 0x0002, model 0x0004, firmware 1.0.4.0
        let msg = [0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x00, 0x20, 0x6B, 0x02, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0xF7];
        let id = parse_identity_reply(&msg).unwrap();
        assert_eq!(id.manufacturer, vec![0x00, 0x20, 0x6B]);
        assert_eq!((id.family, id.model), (2, 4));
        assert_eq!(id.version, [1, 0, 4, 0]);
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
        let best = rank("Minilab3 MIDI", None, &cat, 3);
        assert_eq!(cat[best[0].1].slug, "minilab3");
        let best = rank("TR-8S", None, &cat, 3);
        assert_eq!(cat[best[0].1].slug, "tr_8s");
    }

    #[test]
    fn identity_breaks_ties_between_same_named_models() {
        let cat = vec![
            cand("ms_20_korg", "MS-20", "Korg", &[]),
            cand("ms_20_arturia", "MS-20", "Arturia", &[]),
        ];
        let korg = Identity { manufacturer: vec![0x42], family: 0, model: 0, version: [0; 4] };
        let best = rank("MS-20 mini", Some(&korg), &cat, 2);
        assert_eq!(cat[best[0].1].slug, "ms_20_korg");
    }

    #[test]
    fn unrelated_port_matches_nothing() {
        let cat = vec![
            cand("minilab3", "MiniLab 3", "Arturia", &[]),
            cand("pcm_80_v1_14", "PCM 80 v1.14", "Lexicon", &[]),
        ];
        assert!(rank("Midi Through:Midi Through Port-0 14:0", None, &cat, 3).is_empty());
    }
}
