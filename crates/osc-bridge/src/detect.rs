//! Hardware detection: which MIDI devices are plugged in, read from the ports and from a
//! capture. What an identity request names and how its reply is read lives in `recognition`.

use anyhow::{anyhow, Result};
use crate::capture::{Asound, Capture, Received, Request};
use crate::recognition::{parse_identity_reply, send_plan, squash, tokens, Identity, Probe};
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
    let cards = capture_cards(capture);
    capture.outputs.iter().map(|n| (n.clone(), usb_of_capture(capture, &cards, n))).collect()
}

/// The sound cards of a capture, with their USB ids.
fn capture_cards(capture: &Capture) -> Vec<(String, u16, u16)> {
    let ids: Vec<(u32, &str)> = capture.usbids.iter().map(|u| (u.card, u.text.as_str())).collect();
    usb_cards(&capture.cards, &ids)
}

/// The USB ids of the port `name` of a capture: the ids the system read for it first, else
/// those of the sound cards.
fn usb_of_capture(capture: &Capture, cards: &[(String, u16, u16)], name: &str) -> Option<(u16, u16)> {
    capture.system_usb.get(name).copied().or_else(|| usb_of_port(name, cards))
}

/// The USB ids a CoreMIDI `kMIDIPropertyUSBVendorProduct` value holds: the vendor on the 16 high
/// bits, the product on the 16 low ones; zero (no USB device) gives none.
pub fn usb_from_coremidi(value: i32) -> Option<(u16, u16)> {
    let bits = value as u32;
    (bits != 0).then_some(((bits >> 16) as u16, (bits & 0xffff) as u16))
}

/// The USB ids a Windows device interface path names (`VID_xxxx&PID_yyyy`, any case, with or
/// without `&MI_nn`); a path without them gives none.
pub fn usb_from_interface_path(path: &str) -> Option<(u16, u16)> {
    let lower = path.to_ascii_lowercase();
    let hex4 = |marker: &str| -> Option<u16> {
        let at = lower.find(marker)? + marker.len();
        let digits = lower.get(at..at + 4)?;
        digits.chars().all(|c| c.is_ascii_hexdigit()).then(|| u16::from_str_radix(digits, 16).ok()).flatten()
    };
    Some((hex4("vid_")?, hex4("pid_")?))
}

/// The longest device name WinMM gives (`MAXPNAMELEN` - 1).
const WINMM_NAME_MAX: usize = 31;

/// Whether the WinMM device name `winmm` and the midir port name name the same device: equal, or
/// one is the other cut at WinMM's 31 characters.
pub fn winmm_name_matches(winmm: &str, port: &str) -> bool {
    if winmm.is_empty() || port.is_empty() {
        return false;
    }
    if winmm == port {
        return true;
    }
    let (short, long) = if winmm.chars().count() <= port.chars().count() { (winmm, port) } else { (port, winmm) };
    short.chars().count() == WINMM_NAME_MAX && long.starts_with(short)
}

/// The USB ids the system itself reads for the ports `outputs` and `inputs` (CoreMIDI on macOS,
/// the WinRT device enumeration on Windows), by port name; empty elsewhere and for a port the system gives none for.
#[cfg(target_os = "macos")]
fn read_system_usb(outputs: &[String], inputs: &[String]) -> std::collections::BTreeMap<String, (u16, u16)> {
    let mut found = std::collections::BTreeMap::new();
    let mut note = |name: Option<String>, object: &coremidi::Object| {
        let Some(name) = name else { return };
        if !outputs.contains(&name) && !inputs.contains(&name) {
            return;
        }
        if let Some(ids) = object.get_property_integer("USBVendorProduct").ok().and_then(usb_from_coremidi) {
            found.insert(name, ids);
        }
    };
    for d in coremidi::Destinations {
        note(d.display_name(), &d);
    }
    for s in coremidi::Sources {
        note(s.display_name(), &s);
    }
    found
}

#[cfg(windows)]
fn read_system_usb(outputs: &[String], inputs: &[String]) -> std::collections::BTreeMap<String, (u16, u16)> {
    let all: Vec<String> = outputs.iter().chain(inputs).cloned().collect();
    usb_of_winrt_devices(&all, &enumerate_winrt_midi().unwrap_or_default())
}

/// The MIDI devices WinRT lists (the ports of `MidiInPort` and `MidiOutPort`), each with the
/// instance ids of the devices of its container; an enumeration that fails gives none.
#[cfg(windows)]
fn enumerate_winrt_midi() -> windows::core::Result<Vec<WinrtDevice>> {
    use windows::core::{IInspectable, Interface, GUID, HSTRING};
    use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind};
    use windows::Devices::Midi::{MidiInPort, MidiOutPort};
    use windows::Foundation::Collections::IIterable;
    use windows::Foundation::IReference;
    let container_key = HSTRING::from("System.Devices.ContainerId");
    let with_container = || IIterable::<HSTRING>::try_from(vec![container_key.clone()]);
    let mut devices = Vec::new();
    for selector in [MidiInPort::GetDeviceSelector()?, MidiOutPort::GetDeviceSelector()?] {
        let ports = DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&selector, &with_container()?)?.get()?;
        for port in ports {
            let container = port.Properties()?.Lookup(&container_key).ok()
                .and_then(|v: IInspectable| v.cast::<IReference<GUID>>().ok())
                .and_then(|r| r.Value().ok());
            let mut instance_ids = Vec::new();
            if let Some(guid) = container {
                let filter = HSTRING::from(format!("System.Devices.ContainerId:=\"{{{guid:?}}}\""));
                let none = IIterable::<HSTRING>::try_from(Vec::new())?;
                let members = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&filter, &none, DeviceInformationKind::Device)
                    .and_then(|op| op.get());
                for member in members.into_iter().flatten() {
                    if let Ok(id) = member.Id() {
                        instance_ids.push(id.to_string());
                    }
                }
            }
            devices.push(WinrtDevice { name: port.Name()?.to_string(), id: port.Id()?.to_string(), instance_ids });
        }
    }
    Ok(devices)
}

/// A MIDI device as WinRT lists it: its full name, its id, and the instance ids of the devices
/// of its container (the USB device that carries `VID_xxxx&PID_yyyy` among them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinrtDevice {
    pub name: String,
    pub id: String,
    pub instance_ids: Vec<String>,
}

/// The USB ids of each of `ports` (WinMM names, cut at 31 characters) from the WinRT `devices`
/// whose name matches: the first of the device's paths (its id, then its instance ids) that
/// carries them. A port none of whose devices has ids gets none, and so does one whose devices
/// with ids disagree.
pub fn usb_of_winrt_devices(ports: &[String], devices: &[WinrtDevice]) -> std::collections::BTreeMap<String, (u16, u16)> {
    let mut found = std::collections::BTreeMap::new();
    for port in ports {
        let mut ids = devices.iter().filter(|d| winmm_name_matches(&d.name, port)).filter_map(|d| {
            std::iter::once(&d.id).chain(&d.instance_ids).find_map(|path| usb_from_interface_path(path))
        });
        let Some(first) = ids.next() else { continue };
        if ids.all(|i| i == first) {
            found.insert(port.clone(), first);
        }
    }
    found
}

#[cfg(not(any(target_os = "macos", windows)))]
fn read_system_usb(_outputs: &[String], _inputs: &[String]) -> std::collections::BTreeMap<String, (u16, u16)> {
    std::collections::BTreeMap::new()
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

/// List the MIDI ports `open` gives, pair inputs with outputs by name, and — when `probe` is
/// true — send the Identity Request, then each of `probes` (the maker
/// requests the catalogue's identity blocks name, each to its ports), on
/// every output and collect the SysEx that comes back for `wait`.
pub fn scan(open: crate::midi_port::Opener, probe: bool, wait: Duration, probes: &[Probe]) -> Result<Vec<Probed>> {
    Ok(replay(&observe(open, probe, wait, probes, &[])?))
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
    let cards = capture_cards(capture);
    for d in devices.iter_mut() {
        d.usb = d.out_name.as_deref().and_then(|n| usb_of_capture(capture, &cards, n))
            .or_else(|| d.in_name.as_deref().and_then(|n| usb_of_capture(capture, &cards, n)));
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
pub fn observe(open: crate::midi_port::Opener, probe: bool, wait: Duration, probes: &[Probe], consent: &[String]) -> Result<Capture> {
    let mut midi = crate::midi_port::guarded_open(open)?;
    midi.capture(probe, wait, probes, consent)
}

/// The observation of this machine's own MIDI system, through midir and the machine's listings:
/// what the hardware's `capture` does, with no guard of the opening (the caller guards).
pub(crate) fn observe_hardware(probe: bool, wait: Duration, probes: &[Probe], consent: &[String]) -> Result<Capture> {
    let output = MidiOutput::new("osc-bridge-detect")?;
    let out_ports = output.ports();
    let out_names: Vec<String> = out_ports.iter()
        .map(|p| output.port_name(p).unwrap_or_else(|_| "?".into())).collect();
    let input = MidiInput::new("osc-bridge-detect")?;
    let in_ports = input.ports();
    let in_names: Vec<String> = in_ports.iter()
        .map(|p| input.port_name(p).unwrap_or_else(|_| "?".into())).collect();
    let mut capture = Capture::new(out_names.clone(), in_names.clone(), Asound::machine());
    capture.system_usb = read_system_usb(&out_names, &in_names);
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

/// The ALSA client number of a raw port name: the `N` of its final `N:M`.
pub fn client_number(name: &str) -> Option<u32> {
    let last = name.split_whitespace().next_back()?;
    let (n, m) = last.split_once(':')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(n) && digits(m)).then(|| n.parse().ok()).flatten()
}

/// What groups the ports of one device: the client number, else the squashed client name.
#[derive(PartialEq)]
pub enum ClientKey { Number(u32), Name(String) }

pub fn client_key(name: &str) -> ClientKey {
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

#[cfg(test)]
mod tests;
