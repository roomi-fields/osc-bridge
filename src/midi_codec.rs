//! Incoming MIDI → OSC translation using the device's `midi_in` map.

use crate::device::MidiInMap;
use rosc::{OscMessage, OscType};

/// Parse a channel MIDI message and produce an OSC message using the device's template.
/// Returns None if the message isn't handled or the map has no entry.
pub fn midi_to_osc(msg: &[u8], map: &MidiInMap, osc_prefix: &str) -> Option<OscMessage> {
    if msg.is_empty() { return None; }
    let status = msg[0] & 0xF0;
    let ch = (msg[0] & 0x0F) as i32;
    match status {
        0x80 => map.note_off.as_ref().map(|t| fill_template(t, osc_prefix, &[
            ("note", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
            ("velocity", OscType::Int(msg.get(2).copied().unwrap_or(0) as i32)),
            ("channel", OscType::Int(ch)),
        ])),
        0x90 => {
            let vel = msg.get(2).copied().unwrap_or(0);
            if vel > 0 {
                map.note_on.as_ref().map(|t| fill_template(t, osc_prefix, &[
                    ("note", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
                    ("velocity", OscType::Int(vel as i32)),
                    ("channel", OscType::Int(ch)),
                ]))
            } else {
                map.note_off.as_ref().map(|t| fill_template(t, osc_prefix, &[
                    ("note", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
                    ("velocity", OscType::Int(0)),
                    ("channel", OscType::Int(ch)),
                ]))
            }
        }
        0xA0 => map.poly_aftertouch.as_ref().map(|t| fill_template(t, osc_prefix, &[
            ("note", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
            ("value", OscType::Int(msg.get(2).copied().unwrap_or(0) as i32)),
            ("channel", OscType::Int(ch)),
        ])),
        0xB0 => map.cc.as_ref().map(|t| fill_template(t, osc_prefix, &[
            ("num", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
            ("value", OscType::Int(msg.get(2).copied().unwrap_or(0) as i32)),
            ("channel", OscType::Int(ch)),
        ])),
        0xC0 => map.program_change.as_ref().map(|t| fill_template(t, osc_prefix, &[
            ("program", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
            ("channel", OscType::Int(ch)),
        ])),
        0xD0 => map.aftertouch.as_ref().map(|t| fill_template(t, osc_prefix, &[
            ("value", OscType::Int(msg.get(1).copied().unwrap_or(0) as i32)),
            ("channel", OscType::Int(ch)),
        ])),
        0xE0 => map.pitchbend.as_ref().map(|t| {
            let v14 = ((msg.get(2).copied().unwrap_or(0) as i32) << 7)
                      | (msg.get(1).copied().unwrap_or(0) as i32);
            fill_template(t, osc_prefix, &[
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
                    _ => addr.push_str("?"),
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
