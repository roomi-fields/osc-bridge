//! Incoming MIDI → OSC translation using the device's `midi_in` map.

use osd::{MidiIn, MidiType};
use rosc::{OscMessage, OscType};

/// The template a device reports `t` with: none when its `midi_in` does not list it.
/// A listed type takes the default template `osd` publishes.
pub fn template(map: Option<&MidiIn>, t: MidiType) -> Option<String> {
    match map? {
        MidiIn::Types(l) => {
            if !l.contains(&t) { return None; }
            Some(t.template().to_string())
        }
        MidiIn::Templates(m) => match t {
            MidiType::NoteOn => m.note_on.clone(),
            MidiType::NoteOff => m.note_off.clone(),
            MidiType::Cc => m.cc.clone(),
            MidiType::Pitchbend => m.pitchbend.clone(),
            MidiType::Aftertouch => m.aftertouch.clone(),
            MidiType::PolyAftertouch => m.poly_aftertouch.clone(),
            MidiType::ProgramChange => m.program_change.clone(),
        },
    }
}

/// Parse a channel MIDI message and produce an OSC message using the device's template.
/// Returns None if the message isn't handled or the map has no entry.
pub fn midi_to_osc(msg: &[u8], map: Option<&MidiIn>, osc_prefix: &str) -> Option<OscMessage> {
    if msg.is_empty() { return None; }
    let status = msg[0] & 0xF0;
    let ch = (msg[0] & 0x0F) as i32;
    let tpl = |t: MidiType| template(map, t);
    match status {
        0x80 => tpl(MidiType::NoteOff).map(|t| fill_template(&t, osc_prefix, &[
            ("note", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
            ("velocity", OscType::Int(msg.get(2).copied().unwrap_or(0) as i32)),
            ("channel", OscType::Int(ch)),
        ])),
        0x90 => {
            let vel = msg.get(2).copied().unwrap_or(0);
            if vel > 0 {
                tpl(MidiType::NoteOn).map(|t| fill_template(&t, osc_prefix, &[
                    ("note", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
                    ("velocity", OscType::Int(vel as i32)),
                    ("channel", OscType::Int(ch)),
                ]))
            } else {
                tpl(MidiType::NoteOff).map(|t| fill_template(&t, osc_prefix, &[
                    ("note", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
                    ("velocity", OscType::Int(0)),
                    ("channel", OscType::Int(ch)),
                ]))
            }
        }
        0xA0 => tpl(MidiType::PolyAftertouch).map(|t| fill_template(&t, osc_prefix, &[
            ("note", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
            ("value", OscType::Int(msg.get(2).copied().unwrap_or(0) as i32)),
            ("channel", OscType::Int(ch)),
        ])),
        0xB0 => tpl(MidiType::Cc).map(|t| fill_template(&t, osc_prefix, &[
            ("num", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
            ("value", OscType::Int(msg.get(2).copied().unwrap_or(0) as i32)),
            ("channel", OscType::Int(ch)),
        ])),
        0xC0 => tpl(MidiType::ProgramChange).map(|t| fill_template(&t, osc_prefix, &[
            ("program", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
            ("channel", OscType::Int(ch)),
        ])),
        0xD0 => tpl(MidiType::Aftertouch).map(|t| fill_template(&t, osc_prefix, &[
            ("value", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
            ("channel", OscType::Int(ch)),
        ])),
        0xE0 => tpl(MidiType::Pitchbend).map(|t| {
            let v14 = ((msg.get(2).copied().unwrap_or(0) as i32) << 7)
                      | (msg.get(1).copied().unwrap_or(0) as i32);
            fill_template(&t, osc_prefix, &[
                ("value_u14", OscType::Int(v14)),
                ("channel", OscType::Int(ch)),
            ])
        }),
        _ => None,
    }
}

/// Substitute `{name}` tokens in the OSC template with their values.
/// The template is of the form `"/path/with/{placeholders} arg1 arg2 ..."`.
/// Tokens inside the path part are inlined into the address; those in the trailing
/// position are produced as OSC args.
fn fill_template(tpl: &str, prefix: &str, vars: &[(&str, OscType)]) -> OscMessage {
    let mut parts = tpl.splitn(2, ' ');
    let addr_tpl = parts.next().unwrap_or("");
    let args_tpl = parts.next().unwrap_or("");

    // Replace placeholders in address
    let mut addr = String::new();
    if !addr_tpl.starts_with(prefix) {
        addr.push_str(prefix);
    }
    let mut rest = addr_tpl;
    while let Some(open) = rest.find('{') {
        addr.push_str(&rest[..open]);
        if let Some(close) = rest[open..].find('}') {
            let key = &rest[open + 1..open + close];
            if let Some((_, v)) = vars.iter().find(|(k, _)| *k == key) {
                match v {
                    OscType::Int(i) => addr.push_str(&i.to_string()),
                    OscType::Long(i) => addr.push_str(&i.to_string()),
                    OscType::String(s) => addr.push_str(s),
                    _ => addr.push('?'),
                }
            }
            rest = &rest[open + close + 1..];
        } else {
            addr.push_str(&rest[open..]);
            break;
        }
    }
    addr.push_str(rest);

    // Build args
    let mut args: Vec<OscType> = Vec::new();
    for tok in args_tpl.split_whitespace() {
        let key = tok.trim_matches(|c| c == '{' || c == '}');
        if let Some((_, v)) = vars.iter().find(|(k, _)| *k == key) {
            args.push(v.clone());
        }
    }
    OscMessage { addr, args }
}
