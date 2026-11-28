//! The learn mode and the sharing of a fingerprint: the listening to a device that may not say
//! who it is, its fingerprint, the report built from it, and what is sent to the service. It is
//! the link's work; the bridge keeps the recognition against the cards it is given.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::time::Duration;

use super::online::{Online, Settings};
use crate::capture::Capture;
use crate::detect::{client_key, client_kind, observe, usb_cards, usb_of_port, ClientKey, ClientKind};
use crate::mcp::{content_structured, hardware_candidates, is_possible, proofs_json, slugify, software_cards_only,
                 state_label, unlisted_text, DeviceIndex, McpOptions};
pub use crate::midi_port::{Heard, Unit, Wire};
use crate::recognition::{parse_identity_reply, IDENTITY_REQUEST};

// ---- the listening ----

/// Listening time of the learn mode, in ms: default, and the interval accepted. Wider than
/// `timeout_ms` (100-5000) because a person needs the time to move controls.
const LEARN_DEFAULT_MS: u64 = 5000;
const LEARN_MIN_MS: u64 = 1000;
const LEARN_MAX_MS: u64 = 30000;
/// Distinct controls a fingerprint keeps (the schema's `maxItems`).
const CONTROLS_KEPT: usize = 32;

/// The words of a raw port name, the only writer of them: what follows the first `:`, without the
/// final `N:M`, cut into words on every character that is neither a letter nor a digit, lower
/// case, joined by a space; a run of words repeated at once is kept once.
pub fn port_words(name: &str) -> String {
    let trimmed = name.trim_end();
    let body = match (crate::detect::client_number(trimmed), trimmed.rfind(char::is_whitespace)) {
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

/// The fingerprint of a device (a valid OSD 0.7 fingerprint) and, beside it, what was measured.
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
    fp.insert("$schema".into(), "https://w3id.org/osd/0.7/fingerprint.schema.json".into());
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
pub fn learn_all(open: crate::midi_port::Opener, listen: Duration) -> Result<Vec<(Unit, Learned)>> {
    learn_units(open, &units(&observe(open, false, Duration::ZERO, &[], &[])?), listen)
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
pub fn learn_units(open: crate::midi_port::Opener, devices: &[Unit], listen: Duration) -> Result<Vec<(Unit, Learned)>> {
    let devices: Vec<Unit> = devices.iter().filter(|d| !is_system_unit(d)).cloned().collect();
    let devices = devices.as_slice();
    if devices.is_empty() {
        return Ok(Vec::new());
    }
    let observed = today();
    let mut midi = crate::midi_port::guarded_open(open)?;
    let wires = devices.iter().map(|d| midi.open_wire(d)).collect::<Result<Vec<_>>>()?;
    std::thread::scope(|s| {
        let tasks: Vec<_> = devices.iter().zip(wires).map(|(d, mut wire)| {
            let observed = observed.as_str();
            s.spawn(move || learn(d, &mut *wire, listen, observed))
        }).collect();
        devices.iter().zip(tasks).map(|(d, t)| {
            let learned = t.join().map_err(|_| anyhow!("learn thread panicked"))?;
            Ok((d.clone(), learned))
        }).collect()
    })
}

// ---- the tool and its report ----

/// What a caller gives to share a fingerprint: the port, the vendor, model and firmware the
/// person gave, and what was learned; it says what happened.
pub(crate) type SharesFingerprint<'a> = &'a dyn Fn(&str, &str, &str, &str, &[(Unit, Learned)]) -> Result<String, String>;

/// The sharing of the fingerprint of the port the person named, as a learn mode runs it.
pub(crate) type SharesLearned<'a> = &'a dyn Fn(&[(Unit, Learned)]) -> Result<String, String>;

/// What a caller gives to know what the service says of the devices a learn mode found.
pub(crate) type MatchesOf<'a> = &'a dyn Fn(&[(Unit, Learned)]) -> ServiceMatches;

/// The one device whose port `port` names, or the refusal that asks to name exactly one port.
pub(crate) fn sharing_target<'a>(port: &str, learned: &'a [(Unit, Learned)]) -> Result<&'a (Unit, Learned), String> {
    let wanted = port.to_lowercase();
    let found: Vec<_> = learned.iter().filter(|(u, _)| u.inputs.iter().chain(&u.outputs).any(|p| p.to_lowercase().contains(&wanted))).collect();
    match found.as_slice() {
        [one] => Ok(one),
        _ => Err(format!("share_fingerprint {port:?} names {} device(s): name exactly one port", found.len())),
    }
}

/// The sharing of a copy with no link: nothing goes out.
fn not_linked_share(port: &str, _vendor: &str, _model: &str, _firmware: &str, learned: &[(Unit, Learned)]) -> Result<String, String> {
    sharing_target(port, learned)?;
    Ok("not sent: this copy is not linked to the SyntHub server".into())
}

/// `detect_hardware` with `learn: true` on a server with no link: the ports matched against the
/// local catalogue only, and a share that says it is not sent. The server receives it as its
/// `detect_mode`.
pub fn detect_offline(args: &Value, index: &DeviceIndex, opts: &McpOptions, open: crate::midi_port::Opener) -> Result<Value, String> {
    learn_tool(args, index, opts, &not_linked_share, &|_| ServiceMatches::NotLinked, open)
}

/// `detect_hardware` in learn mode as the tool is called: the arguments read, the fingerprint
/// shared through `shares` when a name is given, the ports matched through `matches`.
pub(crate) fn learn_tool(args: &Value, index: &DeviceIndex, opts: &McpOptions, shares: SharesFingerprint,
                         matches: MatchesOf, open: crate::midi_port::Opener) -> Result<Value, String> {
    let text = |k: &str| args.get(k).and_then(Value::as_str);
    let named = match text("share_fingerprint") {
        None if ["vendor", "model", "firmware"].iter().any(|k| text(k).is_some()) =>
            return Err("vendor, model and firmware go with share_fingerprint: the port whose fingerprint is shared".into()),
        None => None,
        Some(port) => {
            let (vendor, model, firmware) = share_fields(text("vendor"), text("model"), text("firmware"), false)?;
            Some((port, vendor, model, firmware))
        }
    };
    let share = named.map(|(port, vendor, model, firmware)| move |learned: &[(Unit, Learned)]|
        shares(port, vendor, model, firmware, learned));
    learn_hardware(index, matches, learn_listen(args.get("listen_ms").and_then(Value::as_u64))?,
        share.as_ref().map(|s| s as SharesLearned), opts.tier_order, open)
}

/// `detect_hardware` in learn mode: a fingerprint per device, and its measures beside it. The
/// report's text tells the person how long it listens and to move the controls.
pub(crate) fn learn_hardware(index: &DeviceIndex, matches: MatchesOf, listen: Duration,
                             share: Option<SharesLearned>,
                             order: crate::recognition::TierOrder, open: crate::midi_port::Opener) -> Result<Value, String> {
    let learned = learn_all(open, listen).map_err(|e| format!("MIDI scan: {e}"))?;
    let sharing = share.map(|s| s(&learned)).transpose()?;
    let service = matches(&learned);
    let mut report = learn_report(&learned, index, listen, &service, order);
    if let Some(said) = sharing {
        report["structuredContent"]["sharing"] = json!(said);
        let text = format!("{}\nSharing: {said}", report["content"][0]["text"].as_str().unwrap_or_default());
        report["content"][0]["text"] = json!(text);
    }
    Ok(report)
}

/// What the service said of the devices of a learn mode.
#[derive(Debug, Clone)]
pub enum ServiceMatches {
    /// The copy is not linked: only the local catalogue is looked at.
    NotLinked,
    /// Linked, but the service did not answer.
    Unreachable,
    /// What it names for each device, in the order of the devices the learn mode reports: its
    /// devices, else its devices without a card (see `port_answer`).
    Answered(Vec<Vec<Value>>),
}

/// The devices the learn mode reports: the ports of the system are no device.
pub(crate) fn learn_devices(learned: &[(Unit, Learned)]) -> Vec<&(Unit, Learned)> {
    learned.iter().filter(|(u, _)| !is_system_unit(u)).collect()
}

/// The ask that ends the next step of a device nobody knows: name it, then map its controls.
const NEXT_STEP_ASK: &str = "Ask the person to name it: its vendor (the maker), its model and its firmware version; \
    share_fingerprint needs the three, none empty.";

/// What the person is asked for the firmware: the version the identity reply carries, to
/// confirm, or nothing to propose.
fn firmware_ask(proposed: Option<&str>) -> String {
    match proposed {
        Some(v) => format!(" The identity reply carries the firmware version {v}: propose it and have the person confirm it as the firmware."),
        None => " The device sent no firmware version: ask the person for the firmware.".into(),
    }
}

/// What the person is asked of a device to be named, and what follows.
fn naming_ask(proposed_firmware: Option<&str>) -> String {
    format!("{NEXT_STEP_ASK}{} Then map its controls one by one with report_mapping, device being its vendor and model as shared (e.g. Behringer BCR-32); a first driver built from the mappings goes to submit_driver.",
        firmware_ask(proposed_firmware))
}

/// The next step of a device whose devices named by the service are all only possible: the person
/// confirms one of them, or names theirs.
fn possible_step(possible: &[&Value], proposed_firmware: Option<&str>) -> String {
    let named: Vec<String> = possible.iter().filter_map(|c| {
        let id = c["id"].as_str()?;
        Some(c["name"].as_str().map_or_else(|| id.to_string(), |name| format!("{name} ({id})")))
    }).collect();
    format!("SyntHub names this device from its name alone, possible, to confirm: {}. Ask the person to confirm which it is, or else: {} \
        Nothing is sent to the device on this account without the person's consent.", named.join(" or "), naming_ask(proposed_firmware))
}

/// The next step of an unknown device, worded by what was looked at.
fn next_step(service: &ServiceMatches, proposed_firmware: Option<&str>) -> String {
    let ask = naming_ask(proposed_firmware);
    match service {
        ServiceMatches::Answered(_) => format!("Neither the local catalogue nor SyntHub knows this device. {ask}"),
        _ => format!("Only the local catalogue was looked at, and no driver in it matches this device. {ask}"),
    }
}

/// The next step of a device the service names without a card: it is proposed to the person to
/// confirm, never named again, and nothing is sent to the device on its account without consent.
fn being_confirmed_step(unlisted: &[Value]) -> String {
    let named: Vec<String> = unlisted.iter().map(unlisted_text).collect();
    let said = unlisted.iter().find_map(|u| u["state_label"].as_str()).unwrap_or("being confirmed");
    format!("SyntHub names this device without a card: {}; {said}. Ask the person to confirm which it is, and do not ask them to name it again. \
        Nothing is sent to the device on this account without the person's consent.", named.join(" or "))
}

/// The next step once a fingerprint is shared: the mappings go to `report_mapping` under the
/// device named as the service accepts it, « maker model » as the person declared it, the firmware
/// not being part of it.
pub fn shared_next_step(vendor: &str, model: &str) -> String {
    format!("Map its controls one by one with report_mapping (device: {}); a first driver built from the mappings goes to submit_driver.",
        declared_name(vendor, model))
}

/// The name of a device without a card as the service accepts it: the model alone when it already
/// starts with the maker (whole word, case ignored), else the maker and the model; spaces reduced.
fn declared_name(vendor: &str, model: &str) -> String {
    let (v, m): (Vec<&str>, Vec<&str>) = (vendor.split_whitespace().collect(), model.split_whitespace().collect());
    let starts = !v.is_empty() && m.len() >= v.len() && v.iter().zip(&m).all(|(a, b)| a.eq_ignore_ascii_case(b));
    if starts { m.join(" ") } else { v.into_iter().chain(m).collect::<Vec<_>>().join(" ") }
}

/// The id of a device the catalogue does not describe: its vendor and model, slugged.
pub fn device_id(vendor: &str, model: &str) -> String {
    format!("{}/{}", slugify(vendor, '-'), slugify(model, '-'))
}

/// The firmware version of a universal Identity Reply (`F0 7E id 06 02 maker family model
/// version F7`: a one- or three-byte maker, two bytes of family, two of model, four of version),
/// as its bytes joined by dots; none when the reply is too short.
fn identity_firmware(hex: &str) -> Option<String> {
    let bytes: Vec<u8> = hex.split_whitespace().map(|b| u8::from_str_radix(b, 16).ok()).collect::<Option<_>>()?;
    if bytes.len() < 6 || bytes[..2] != [0xF0, 0x7E] || bytes[3..5] != [0x06, 0x02] { return None; }
    let body = bytes[5..].strip_suffix(&[0xF7]).unwrap_or(&bytes[5..]);
    let maker = if body.first() == Some(&0) { 3 } else { 1 };
    let version = body.get(maker + 4..maker + 8)?;
    Some(version.iter().map(u8::to_string).collect::<Vec<_>>().join("."))
}

/// The vendor, model and firmware of a sharing, none empty, as the person gave them; the
/// refusal names every missing one in the words of the command (`--vendor`) or the tool (`vendor`).
pub(crate) fn share_fields<'a>(vendor: Option<&'a str>, model: Option<&'a str>, firmware: Option<&'a str>, command: bool)
    -> Result<(&'a str, &'a str, &'a str), String> {
    let given = |v: Option<&'a str>| v.map(str::trim).filter(|v| !v.is_empty());
    let (v, m, f) = (given(vendor), given(model), given(firmware));
    if let (Some(v), Some(m), Some(f)) = (v, m, f) { return Ok((v, m, f)); }
    let missing: Vec<String> = [("vendor", v), ("model", m), ("firmware", f)].into_iter()
        .filter(|(_, x)| x.is_none()).map(|(k, _)| if command { format!("--{k}") } else { k.to_string() }).collect();
    Err(format!("nothing is sent: a fingerprint is shared with three fields, none empty; missing: {}. Ask the person.", missing.join(", ")))
}

/// A refusal of the service that names the vendor, model or firmware, in the words of the
/// command typed (`--vendor`) when `command`, else as it is (the tool's parameters are named
/// so); any other text is returned as it is.
pub fn refusal_in_words(said: &str, command: bool) -> String {
    if !command || !said.contains("device:") { return said.to_string(); }
    said.split(' ').map(|w| {
        let bare = w.trim_matches(|c: char| !c.is_ascii_alphabetic());
        if ["vendor", "model", "firmware"].contains(&bare) { w.replacen(bare, &format!("--{bare}"), 1) } else { w.to_string() }
    }).collect::<Vec<_>>().join(" ")
}

/// The learn mode's result for `learned`: the structured devices and their text. A device
/// carries its `driver`, the service's first device that is not only possible, else the card of
/// `catalogue` that recognises it, and no `next_step`; any other carries a `next_step`, worded
/// by what was named or looked at. A port of the system is left out. Pure: no MIDI.
pub fn learn_report(learned: &[(Unit, Learned)], catalogue: &DeviceIndex, listen: Duration,
                    service: &ServiceMatches, order: crate::recognition::TierOrder) -> Value {
    let candidates = hardware_candidates(catalogue);
    let devices: Vec<Value> = learn_devices(learned).into_iter().enumerate().map(|(at, (d, l))| {
        let replies: Vec<Vec<u8>> = l.fingerprint["identity_reply"].as_str().map(|h| {
            h.split_whitespace().filter_map(|b| u8::from_str_radix(b, 16).ok()).collect()
        }).into_iter().collect();
        // Only a strong proof names a driver; a name alone leaves the device unknown.
        let program = d.client_kind == ClientKind::User;
        let local = d.inputs.iter().chain(&d.outputs).filter(|_| !program)
            .find_map(|p| crate::recognition::rank(p, d.usb, &replies, &candidates, 1, order).into_iter().find(|r| r.is_strong()))
            .map(|r| (&candidates[r.index], proofs_json(&r.proofs)));
        let named: Vec<Value> = match service {
            ServiceMatches::Answered(all) => all.get(at).cloned().unwrap_or_default(),
            _ => Vec::new(),
        };
        let named = if program { software_cards_only(catalogue, named) } else { named };
        let with_card: Vec<&Value> = named.iter().filter(|c| c["id"].is_string()).collect();
        // A device named by its name alone makes no driver.
        let remote = with_card.iter().find(|c| !is_possible(c)).map(|c| {
            let mut driver = json!({ "id": c["id"], "name": c["name"].as_str().unwrap_or_default() });
            for k in ["proofs", "probability", "confirmations", "disputed", "state"] {
                if let Some(v) = c.get(k) { driver[k] = v.clone(); }
            }
            driver["state_label"] = json!(state_label(c));
            driver
        });
        let unlisted: Vec<Value> = if remote.is_none() { named.iter().filter(|u| u["kind"] == "unlisted").cloned().collect() } else { Vec::new() };
        // The local driver is the one its slug resolves to (the most trustworthy file of the
        // device), not a firmware variant that only ranked higher by recognising the port.
        let local = local.map(|(c, proofs)| {
            let mut driver = catalogue.find(&c.slug).map(|e| json!({ "id": e.id, "name": e.name }))
                .unwrap_or_else(|| json!({ "id": c.id, "name": c.name }));
            driver["proofs"] = proofs;
            driver["state_label"] = json!(state_label(&driver));
            driver
        });
        let driver = remote.or(local);
        let mut out = json!({
            "port": d.label(),
            "inputs": d.inputs,
            "outputs": d.outputs,
            "fingerprint": l.fingerprint,
            "measured": l.measured,
        });
        match driver {
            Some(driver) => out["driver"] = driver,
            None if !unlisted.is_empty() => {
                out["next_step"] = json!(being_confirmed_step(&unlisted));
                out["unlisted"] = json!(unlisted);
            }
            None => {
                let proposed = l.fingerprint["identity_reply"].as_str().and_then(identity_firmware);
                if let Some(v) = &proposed { out["proposed_firmware"] = json!(v); }
                out["next_step"] = json!(if with_card.is_empty() {
                    next_step(service, proposed.as_deref())
                } else {
                    possible_step(&with_card, proposed.as_deref())
                });
            }
        }
        out
    }).collect();
    let mut notice = learn_notice(listen);
    if matches!(service, ServiceMatches::Unreachable) {
        notice.push_str(" The SyntHub service did not answer: only the local catalogue was looked at.");
    }
    let structured = json!({ "probed": true, "learn": true, "listen_ms": listen.as_millis() as u64,
        "notice": notice, "devices": devices });
    let text = format!("{notice}\n{}", serde_json::to_string_pretty(&structured).unwrap_or_default());
    content_structured(&text, structured)
}

// ---- the link's side: the service's answer and the sharing ----

/// Asks the service to match the ports and identity reply of each device of `learned` (the
/// system ports left out). Never fails: an unreachable service is `Unreachable`.
pub fn learn_service_matches(server: Option<&Online>, learned: &[(Unit, Learned)]) -> ServiceMatches {
    let Some(server) = server else { return ServiceMatches::NotLinked };
    let devices = learn_devices(learned);
    if devices.is_empty() { return ServiceMatches::Answered(Vec::new()); }
    let ports: Vec<Value> = devices.iter().map(|(d, l)| crate::mcp::detect_port(d.label(),
        l.fingerprint["identity_reply"].as_str().into_iter().map(str::to_string).collect(), d.usb)).collect();
    let alone: Vec<Vec<usize>> = (0..devices.len()).map(|i| vec![i]).collect();
    match server.detect_entries(&ports, &alone) {
        Ok(answers) => ServiceMatches::Answered((0..devices.len()).map(|i| answers.get(i).map(crate::mcp::port_answer).unwrap_or_default()).collect()),
        Err(_) => ServiceMatches::Unreachable,
    }
}

/// What a fingerprint sent to the service says it carries.
const SENT_FIELDS: &str = "sent: the vendor, model and firmware, USB vendor and product ids, identity_reply, ports, what it sends, its controls; no serial number, host name or measure";

/// The fingerprint to share: the port, the name the person gave and the link and private list
/// that decide whether anything goes out.
pub(super) struct Share<'a> {
    pub(super) port: &'a str,
    pub(super) vendor: &'a str,
    pub(super) model: &'a str,
    pub(super) firmware: &'a str,
    /// Typed on the command line: a refusal is worded with its flags.
    pub(super) command: bool,
    pub(super) client: Option<&'a Online>,
    pub(super) private: &'a [String],
}

/// Shares the fingerprint of the one device `share.port` names, and says what happened. A
/// failure of the service is said here and on the standard error, and the detection goes on.
pub(super) fn share_learned(share: &Share, learned: &[(Unit, Learned)]) -> Result<String, String> {
    let (unit, one) = sharing_target(share.port, learned)?;
    let name = format!("{} {}", share.vendor, share.model);
    let id = device_id(share.vendor, share.model);
    if Settings::covers(share.private, &id, &id) {
        return Ok(format!("not sent: {name} is private on this machine"));
    }
    let (Some(client), Some(file)) = (share.client, super::online::settings_file()) else {
        return Ok("not sent: this copy is not linked to the SyntHub server".into());
    };
    Ok(match client.share_fingerprint(&file, share.vendor, share.model, share.firmware, &one.fingerprint) {
        super::online::Sharing::Sent => format!("the fingerprint of {name} (firmware {}, {}) was sent to SyntHub ({}). {}",
            share.firmware, unit.label(), SENT_FIELDS, shared_next_step(share.vendor, share.model)),
        super::online::Sharing::Withheld(why) => format!("not sent: {why}"),
        super::online::Sharing::Failed(why) => {
            let why = refusal_in_words(&why, share.command);
            eprintln!("osc-bridge: the fingerprint of {name} was not sent, it stays on this machine: {why}");
            format!("not sent, it stays on this machine: {why}")
        }
    })
}

/// `osc-bridge detect --learn`: the notice first (the person moves the controls while it
/// listens), then the fingerprint of each port and its measures.
/// With `share` (the port, then the vendor, model and firmware the person gives), that
/// fingerprint is sent to SyntHub when linked and not private; naming the port is the person's
/// agreement. A missing vendor, model or firmware is refused before anything is listened to.
pub fn print_learning(open: crate::midi_port::Opener, listen_ms: Option<u64>, share: Option<(&str, [Option<&str>; 3])>, order: crate::recognition::TierOrder) -> Result<()> {
    let share = share.map(|(port, [vendor, model, firmware])| {
        share_fields(vendor, model, firmware, true).map(|(vendor, model, firmware)| (port, vendor, model, firmware))
    }).transpose().map_err(|e| anyhow::anyhow!(e))?;
    let listen = learn_listen(listen_ms).map_err(|e| anyhow::anyhow!(e))?;
    println!("{}", learn_notice(listen));
    let client = super::online::linked().then(|| Online::new(super::online::SERVER));
    let private = super::online::settings_file().and_then(|f| Settings::load(Some(&f)).ok()).map(|s| s.private).unwrap_or_default();
    let (linked, kept) = (client.as_ref(), private.as_slice());
    let share = share.map(|(port, vendor, model, firmware)| move |learned: &[(Unit, Learned)]|
        share_learned(&Share { port, vendor, model, firmware, command: true, client: linked, private: kept }, learned));
    let catalogue = DeviceIndex::build(&super::online::cache_dir().unwrap_or_default(), order);
    let matches = |learned: &[(Unit, Learned)]| learn_service_matches(client.as_ref(), learned);
    let report = learn_hardware(&catalogue, &matches, listen, share.as_ref().map(|s| s as SharesLearned), order, open)
        .map_err(|e| anyhow::anyhow!(e))?;
    if let Some(said) = report["structuredContent"]["sharing"].as_str() { println!("Sharing: {said}"); }
    let devices = report["structuredContent"]["devices"].as_array().cloned().unwrap_or_default();
    if devices.is_empty() {
        println!("No MIDI port found.");
    }
    for d in devices {
        let names = |k: &str| d[k].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
            .filter(|s| !s.is_empty()).unwrap_or("-".into());
        println!("{}  (out {}; in {})", d["port"].as_str().unwrap_or("?"), names("outputs"), names("inputs"));
        println!("  fingerprint: {}", serde_json::to_string_pretty(&d["fingerprint"]).unwrap_or_default().replace('\n', "\n  "));
        println!("  measured: {}", d["measured"]);
        if let Some(step) = d["next_step"].as_str() { println!("  next step: {step}"); }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
