//! Hardware detection: which MIDI devices are plugged in, read from the ports and from a
//! capture. What an identity request names and how its reply is read lives in `recognition`.

use anyhow::{anyhow, Result};
use crate::capture::{Asound, Capture, Received, Request};
use crate::recognition::{parse_identity_reply, send_plan, squash, tokens, Identity, Probe, IDENTITY_REQUEST};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    /// Whether its client is a device of the system or a program, as the list of clients says.
    pub client_kind: ClientKind,
}

/// What owns the client of a port: the system (a device) or a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    Kernel,
    User,
    Unknown,
}

impl ClientKind {
    pub fn word(self) -> &'static str {
        match self {
            ClientKind::Kernel => "kernel",
            ClientKind::User => "user",
            ClientKind::Unknown => "unknown",
        }
    }
}

/// The kind of the client of `port`, read from `clients` (the text of `/proc/asound/seq/clients`):
/// the one entry whose number is the port's client number and whose name is the port's client name.
/// A system that lists no clients, a port with no number, no entry, or several entries are `Unknown`.
pub fn client_kind(clients: &str, port: &str) -> ClientKind {
    let Some(number) = client_number(port) else { return ClientKind::Unknown };
    let name = port.split(':').next().unwrap_or(port);
    // The port's name may carry one final " (digits)" the entry's name does not.
    let stripped = name.strip_suffix(')').and_then(|s| s.rsplit_once(" (")).filter(|(_, d)| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())).map(|(base, _)| base);
    let mut found = clients.lines().filter_map(|line| {
        let rest = line.strip_prefix("Client")?;
        let (n, rest) = rest.split_once(':')?;
        if n.trim().parse::<u32>().ok()? != number { return None; }
        let (quoted, tail) = rest.trim().strip_prefix('"')?.rsplit_once('"')?;
        if quoted != name && Some(quoted) != stripped { return None; }
        let inside = tail.trim().strip_prefix('[')?;
        Some(inside.split(|c: char| c.is_whitespace() || c == ']').next().unwrap_or("").to_ascii_lowercase())
    });
    match (found.next(), found.next()) {
        (Some(kind), None) if kind == "kernel" => ClientKind::Kernel,
        (Some(kind), None) if kind == "user" => ClientKind::User,
        _ => ClientKind::Unknown,
    }
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
            client_kind: client_kind(&capture.clients, oname),
        });
    }
    for (ii, iname) in in_names.iter().enumerate() {
        if !used_in.contains(&ii) {
            devices.push(Probed { out_index: None, out_name: None, in_index: Some(ii),
                                  in_name: Some(iname.clone()), identity: None, usb: None, replies: Vec::new(),
                                  client_kind: client_kind(&capture.clients, iname) });
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
    crate::midi_port::guarded_open(|| observe_unguarded(probe, wait, probes, consent))
}

/// `observe` without the guard of the opening, for a caller that guards the whole.
pub(crate) fn observe_unguarded(probe: bool, wait: Duration, probes: &[Probe], consent: &[String]) -> Result<Capture> {
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

/// What groups the ports of one device: the client number, else the squashed client name.
#[derive(PartialEq)]
enum ClientKey { Number(u32), Name(String) }

fn client_key(name: &str) -> ClientKey {
    match client_number(name) {
        Some(c) => ClientKey::Number(c),
        None => ClientKey::Name(squash(name.split(':').next().unwrap_or(name))),
    }
}

/// The ports of `found` grouped by device (the grouping of `units`), as indexes into `found`,
/// devices and ports in the order the system gave them.
pub fn group_probed(found: &[Probed]) -> Vec<Vec<usize>> {
    let mut groups: Vec<(ClientKey, Vec<usize>)> = Vec::new();
    for (i, p) in found.iter().enumerate() {
        let k = client_key(p.label());
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, ix)) => ix.push(i),
            None => groups.push((k, vec![i])),
        }
    }
    groups.into_iter().map(|(_, ix)| ix).collect()
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
    /// Whether its client is a device of the system or a program, as the list of clients says.
    pub client_kind: ClientKind,
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
    let key = client_key;
    let ids: Vec<(u32, &str)> = capture.usbids.iter().map(|u| (u.card, u.text.as_str())).collect();
    let cards = usb_cards(&capture.cards, &ids);
    let mut found: Vec<(ClientKey, Unit)> = Vec::new();
    let all = capture.outputs.iter().enumerate().map(|(i, n)| (true, i, n))
        .chain(capture.inputs.iter().enumerate().map(|(i, n)| (false, i, n)));
    for (is_out, index, name) in all {
        let k = key(name);
        let at = match found.iter().position(|(f, _)| *f == k) {
            Some(at) => at,
            None => {
                found.push((k, Unit { inputs: vec![], outputs: vec![], in_indexes: vec![], out_indexes: vec![], usb: None,
                    client_kind: ClientKind::Unknown }));
                found.len() - 1
            }
        };
        let u = &mut found[at].1;
        if is_out { u.outputs.push(name.clone()); u.out_indexes.push(index); }
        else { u.inputs.push(name.clone()); u.in_indexes.push(index); }
    }
    found.into_iter().map(|(_, mut u)| {
        u.usb = u.outputs.iter().chain(&u.inputs).find_map(|n| usb_of_port(n, &cards));
        // As the detection of a device does: a port of the system makes the unit a device of the system.
        let kinds: Vec<ClientKind> = u.outputs.iter().chain(&u.inputs).map(|n| client_kind(&capture.clients, n)).collect();
        u.client_kind = if kinds.contains(&ClientKind::Kernel) { ClientKind::Kernel }
            else { kinds.first().copied().unwrap_or(ClientKind::Unknown) };
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
        crate::midi_port::guarded_open(|| Self::open_unguarded(device))
    }

    fn open_unguarded(device: &Unit) -> Result<Self> {
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
    learn_units(&units(&observe(false, Duration::ZERO, &[], &[])?), listen)
}

/// Whether a port label is a port of the system and no device: the ALSA "Midi Through" kernel
/// client, recognised by its client name (the label before the colon, or all of it), compared
/// case-insensitively whatever numbers follow.
pub fn is_system_port(label: &str) -> bool {
    let client = label.split(':').next().unwrap_or(label);
    client.trim().eq_ignore_ascii_case("midi through")
}

/// A unit whose every port is a port of the system.
pub fn is_system_unit(unit: &Unit) -> bool {
    let mut ports = unit.inputs.iter().chain(&unit.outputs).peekable();
    ports.peek().is_some() && ports.all(|p| is_system_port(p))
}

/// The learn mode on the `devices` given and no other, the ports of the system left out: the
/// same listening as `learn_all`.
pub fn learn_units(devices: &[Unit], listen: Duration) -> Result<Vec<(Unit, Learned)>> {
    let devices: Vec<Unit> = devices.iter().filter(|d| !is_system_unit(d)).cloned().collect();
    let devices = devices.as_slice();
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
