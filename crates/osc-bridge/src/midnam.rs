//! MIDNAM — a driver's MIDI names as a MIDI Name Document 1.0
//! (`MIDINameDocument10.dtd`), the file DAWs such as Ardour, Logic and
//! Digital Performer read to name a device's controllers and notes.
//!
//! Written: the controllers of `cc_params` (7-bit and 14-bit CC, NRPN, RPN,
//! with their range and the named values the source states), the notes
//! of `triggers` (drum maps), per MIDI channel, and the program banks of
//! `programs` with their bank select. Not representable, left out: SysEx
//! parameters and commands, OSC routes, recall messages other than bank
//! select and program change, sound names (programs are named by bank and
//! number).

use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::fmt::Write;

use osd::{CcEntry as CcParamEntry, Device, Trigger as TriggerEntry};
use crate::export::{credit, credits, device_title, label};

const DOCTYPE: &str = r#"<!DOCTYPE MIDINameDocument PUBLIC "-//MIDI Manufacturers Association//DTD MIDINameDocument 1.0//EN" "http://www.midi.org/dtds/MIDINameDocument10.dtd">"#;

/// The MIDI Name Document of one loaded driver. Fails for a device that
/// speaks OSC only.
pub fn midnam(dev: &Device) -> Result<String> {
    if dev.device.transport.as_ref().is_some_and(|t| t.kind == osd::TransportKind::Osc) {
        bail!("{} speaks OSC, not MIDI: it has no MIDI names", device_title(dev));
    }
    let controls = controls(dev);
    // The channels that hear the same notes share one channel name set.
    let mut sets: BTreeMap<Vec<(u8, String)>, Vec<u8>> = BTreeMap::new();
    for ch in 0..16u8 {
        sets.entry(notes_on(dev, ch)).or_default().push(ch);
    }
    // The set of most channels is "Names", the others are named by channel.
    let widest = sets.values().map(Vec::len).max().unwrap_or(16);
    let mut named_main = false;
    let sets: Vec<NameSet> = sets.into_iter().map(|(notes, chans)| {
        let main = chans.len() == widest && !named_main;
        named_main |= main;
        (if main { "Names".to_string() } else { channel_names(&chans) }, notes, chans)
    }).collect();

    let mut x = String::new();
    let _ = writeln!(x, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(x, "{DOCTYPE}");
    let _ = writeln!(x, "<!--\n  Written by osc-bridge from its driver for {}.", comment(&device_title(dev)));
    for line in credits(dev) {
        let _ = writeln!(x, "  {}", comment(&line));
    }
    let _ = writeln!(x, "-->");
    let _ = writeln!(x, "<MIDINameDocument>");
    let _ = writeln!(x, "  <Author>{}</Author>", esc(&author(dev)));
    let _ = writeln!(x, "  <MasterDeviceNames>");
    let _ = writeln!(x, "    <Manufacturer>{}</Manufacturer>", esc(dev.device.vendor.as_deref().unwrap_or_default()));
    // Firmwares of one device map MIDI differently: the model names its own.
    let model = match dev.sources.iter().flatten().next().and_then(|s| s.firmware.as_deref()).filter(|f| *f != "unknown") {
        Some(fw) => format!("{} (firmware {fw})", dev.device.name),
        None => dev.device.name.clone(),
    };
    let _ = writeln!(x, "    <Model>{}</Model>", esc(&model));
    let _ = writeln!(x, r#"    <CustomDeviceMode Name="Default">"#);
    let _ = writeln!(x, "      <ChannelNameSetAssignments>");
    for ch in 0..16u8 {
        let set = sets.iter().find(|(_, _, c)| c.contains(&ch)).map(|(n, ..)| n.as_str()).unwrap_or("Names");
        let _ = writeln!(x, r#"        <ChannelNameSetAssign Channel="{}" NameSet="{}"/>"#, ch + 1, esc(set));
    }
    let _ = writeln!(x, "      </ChannelNameSetAssignments>");
    let _ = writeln!(x, "    </CustomDeviceMode>");
    for (name, notes, chans) in &sets {
        let _ = writeln!(x, r#"    <ChannelNameSet Name="{}">"#, esc(name));
        let _ = writeln!(x, "      <AvailableForChannels>");
        for ch in 0..16u8 {
            let _ = writeln!(x, r#"        <AvailableChannel Channel="{}" Available="{}"/>"#, ch + 1, chans.contains(&ch));
        }
        let _ = writeln!(x, "      </AvailableForChannels>");
        if !notes.is_empty() {
            let _ = writeln!(x, r#"      <UsesNoteNameList Name="Notes {}"/>"#, esc(name));
        }
        if !controls.is_empty() {
            let _ = writeln!(x, r#"      <UsesControlNameList Name="Controls"/>"#);
        }
        write_banks(&mut x, dev, chans);
        let _ = writeln!(x, "    </ChannelNameSet>");
    }
    for (name, notes, _) in sets.iter().filter(|(_, n, _)| !n.is_empty()) {
        let _ = writeln!(x, r#"    <NoteNameList Name="Notes {}">"#, esc(name));
        for (n, label) in notes {
            let _ = writeln!(x, r#"      <Note Number="{n}" Name="{}"/>"#, esc(label));
        }
        let _ = writeln!(x, "    </NoteNameList>");
    }
    if !controls.is_empty() {
        let _ = writeln!(x, r#"    <ControlNameList Name="Controls">"#);
        for c in &controls {
            let head = format!(r#"<Control Type="{}" Number="{}" Name="{}""#, c.kind, c.number, esc(&c.name));
            let _ = writeln!(x, "      {head}>");
            let _ = writeln!(x, r#"        <Values Min="{}" Max="{}">"#, c.min, c.max);
            if !c.values.is_empty() {
                let _ = writeln!(x, "          <ValueNameList>");
                for (v, n) in &c.values {
                    let _ = writeln!(x, r#"            <Value Number="{v}" Name="{}"/>"#, esc(n));
                }
                let _ = writeln!(x, "          </ValueNameList>");
            }
            let _ = writeln!(x, "        </Values>");
            let _ = writeln!(x, "      </Control>");
        }
        let _ = writeln!(x, "    </ControlNameList>");
    }
    let _ = writeln!(x, "  </MasterDeviceNames>");
    let _ = writeln!(x, "</MIDINameDocument>");
    Ok(x)
}

/// The program banks the channels `chans` recall, each with its bank select
/// and program changes; programs are named by bank and number, as the
/// driver holds no sound names. The DTD wants a bank: an empty one when the
/// driver lists none, or recalls without a program change.
fn write_banks(x: &mut String, dev: &Device, chans: &[u8]) {
    let banks: Vec<_> = dev.programs.iter().filter(|p| p.program_change.unwrap_or(true))
        .flat_map(|p| p.banks.iter().flatten())
        .filter(|b| b.channel.is_none_or(|c| chans.contains(&c))).collect();
    if banks.is_empty() {
        let _ = writeln!(x, r#"      <PatchBank Name="Programs">"#);
        let _ = writeln!(x, "        <PatchNameList/>");
        let _ = writeln!(x, "      </PatchBank>");
        return;
    }
    for b in banks {
        let _ = writeln!(x, r#"      <PatchBank Name="{}">"#, esc(&b.name));
        if b.msb.is_some() || b.lsb.is_some() {
            let _ = writeln!(x, "        <MIDICommands>");
            for (cc, v) in [(0, b.msb), (32, b.lsb)] {
                if let Some(v) = v {
                    let _ = writeln!(x, r#"          <ControlChange Control="{cc}" Value="{v}"/>"#);
                }
            }
            let _ = writeln!(x, "        </MIDICommands>");
        }
        let _ = writeln!(x, r#"        <PatchNameList Name="{}">"#, esc(&b.name));
        for i in 0..b.size as u16 {
            let n = b.first_number.unwrap_or(1) as u16 + i;
            let _ = writeln!(x, r#"          <Patch Number="{n}" Name="{} {n}" ProgramChange="{}"/>"#, esc(&b.name), b.first_pc.unwrap_or(0) as u16 + i);
        }
        let _ = writeln!(x, "        </PatchNameList>");
        let _ = writeln!(x, "      </PatchBank>");
    }
}

/// A channel name set: its name, the notes it names, its channels (0-15).
type NameSet = (String, Vec<(u8, String)>, Vec<u8>);

/// "Channel 10" / "Channels 1, 2": a name set for some channels only.
fn channel_names(chans: &[u8]) -> String {
    let list: Vec<String> = chans.iter().map(|c| (c + 1).to_string()).collect();
    format!("Channel{} {}", if list.len() > 1 { "s" } else { "" }, list.join(", "))
}

/// The document's author: who the data is credited to, with the licence.
fn author(dev: &Device) -> String {
    let shared = crate::export::shared_sources(dev);
    let all: Vec<String> = dev.sources.iter().flatten().chain(&shared).map(credit).collect();
    if all.is_empty() { "osc-bridge catalogue (source not stated)".into() } else { all.join("; ") }
}

/// The notes channel `ch` (0-15) names: the triggers fixed on it, then
/// those on whatever channel the device listens to. One name per note,
/// the first trigger's; a trigger without a natural note names its range.
fn notes_on(dev: &Device, ch: u8) -> Vec<(u8, String)> {
    let Some(t) = &dev.triggers else { return vec![] };
    let mut out: BTreeMap<u8, String> = BTreeMap::new();
    let fixed = t.entries.iter().filter(|e| e.channel == Some(ch));
    let floating = t.entries.iter().filter(|e| e.channel.is_none());
    for e in fixed.chain(floating) {
        for (n, name) in note_names(e) {
            out.entry(n).or_insert(name);
        }
    }
    out.into_iter().collect()
}

fn note_names(e: &TriggerEntry) -> Vec<(u8, String)> {
    match e.note {
        Some(n) => vec![(n, e.sound.clone())],
        None => (e.note_range[0] as u8..=e.note_range[1] as u8).map(|n| (n, format!("{} {n}", e.sound))).collect(),
    }
}

/// One `<Control>`.
struct Control {
    kind: &'static str,
    number: u16,
    name: String,
    min: i64,
    max: i64,
    values: Vec<(i64, String)>,
}

/// The controllers of `cc_params`, each MIDI address once (the first).
fn controls(dev: &Device) -> Vec<Control> {
    let Some(t) = &dev.cc_params else { return vec![] };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for e in &t.entries {
        for c in controls_of(e) {
            if seen.insert((c.kind, c.number)) { out.push(c); }
        }
    }
    out
}

fn controls_of(e: &CcParamEntry) -> Vec<Control> {
    let name = label(&e.osc);
    let values = e.remarks.get("_usage").and_then(|v| v.as_str()).and_then(value_names).unwrap_or_default();
    let range = e.range.unwrap_or([0, 127]);
    let make = |kind, number: u16, name: String| Control { kind, number, name, min: range[0], max: range[1], values: values.clone() };
    let lsb_from_call = e.nrpn_lsb_range.filter(|_| e.nrpn_msb.is_some() && e.nrpn_lsb.is_none());
    match (e.nrpn_msb, e.nrpn_lsb, lsb_from_call, e.rpn_msb, e.rpn_lsb, e.cc) {
        (Some(m), Some(l), ..) => vec![make("NRPN", (m as u16) << 7 | l as u16, name)],
        (Some(m), None, Some([lo, hi]), ..) => (lo as u8..=hi as u8)
            .map(|l| make("NRPN", (m as u16) << 7 | l as u16, format!("{name} {l}"))).collect(),
        (_, _, _, Some(m), Some(l), _) => vec![make("RPN", (m as u16) << 7 | l as u16, name)],
        // MIDNAM's 14-bit controller is CC 0-31 with its LSB 32 above.
        (.., Some(cc)) if range[1] > 127 && cc < 32 && e.cc_lsb == Some(cc + 32) => vec![make("14bit", cc as u16, name)],
        (.., Some(cc)) => {
            let mut c = make("7bit", cc as u16, name);
            c.max = c.max.min(127);
            vec![c]
        }
        _ => vec![],
    }
}

/// Named values from a source's usage text (`0: Off; 1-127: On`): each item
/// `N: name` or `N-M: name`, named at N. None unless every item reads so
/// and there are two at least (one range only describes the scale). Text
/// cut at the import's 150 characters loses its last item.
pub fn value_names(usage: &str) -> Option<Vec<(i64, String)>> {
    let mut items: Vec<&str> = usage.split(';').map(str::trim).filter(|s| !s.is_empty()).collect();
    if usage.chars().count() >= 150 { items.pop(); }
    let out: Option<Vec<(i64, String)>> = items.iter().map(|item| {
        let (num, name) = item.split_once(':')?;
        let num = num.trim();
        let low = match num.get(1..).and_then(|rest| rest.find(['-', '~'])) {
            Some(i) => &num[..i + 1],
            None => num,
        };
        let name = name.trim();
        (!name.is_empty()).then_some((low.trim().parse::<i64>().ok()?, name.to_string()))
    }).collect();
    let out = out?;
    let distinct: std::collections::HashSet<i64> = out.iter().map(|(n, _)| *n).collect();
    (out.len() >= 2 && distinct.len() == out.len()).then_some(out)
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Text safe inside `<!-- -->`, which may not hold `--`.
fn comment(s: &str) -> String {
    let mut out = s.replace("--", "- -");
    if out.ends_with('-') { out.push(' '); }
    out
}

#[cfg(test)]
mod tests;
