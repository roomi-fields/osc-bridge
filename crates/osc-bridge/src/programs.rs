//! The addresses every driver answers the same way, whatever its maker,
//! from its `identity` and `programs` blocks:
//!
//! - `/<prefix>/program/recall <bank> <number> [channel]` sends what recalls
//!   a stored program — the `before` messages, the bank select, the program
//!   change, the `after` messages, or what `recall_script` writes. The bank
//!   is named, or given by its place in `banks` (from 0); the number is the
//!   one the device shows (from the bank's `first_number`).
//! - `/<prefix>/program/dump [<bank> <number>] [channel]` sends the request
//!   that reads a sound back (`dump.request` or `dump.script`); for an edit
//!   buffer, recall the program first and wait `delay_ms`.
//! - `/<prefix>/identity` sends the request the device answers; its reply
//!   comes back as `/<prefix>/identity/reply <firmware> <hex>`.

use crate::device::full_osc_path;
use crate::frame::{build_message, expand_identity, Bindings};
use osd::{Bank, Device, FrameToken, Programs};
use rosc::{OscMessage, OscType};

/// The arguments a `programs` frame may name (`{"arg": name}`).
pub const PLACEHOLDERS: [&str; 7] = ["channel", "msb", "lsb", "bank", "program", "index", "number"];

/// The routes, after the device's prefix.
pub const RECALL: &str = "/program/recall";
pub const DUMP: &str = "/program/dump";
pub const IDENTITY: &str = "/identity";
pub const IDENTITY_REPLY: &str = "/identity/reply";

/// A bank of `p` by its name (any case) or its place in the list.
pub fn find_bank<'a>(p: &'a Programs, bank: &str) -> Option<(usize, &'a Bank)> {
    let banks = p.banks.as_deref().unwrap_or_default();
    banks.iter().enumerate().find(|(_, b)| b.name.eq_ignore_ascii_case(bank))
        .or_else(|| bank.parse::<usize>().ok().and_then(|i| banks.get(i).map(|b| (i, b))))
}

/// One program of a bank, as its frames and scripts see it.
struct Target<'a> {
    bank_index: usize,
    bank: &'a Bank,
    number: i64,
    pc: u8,
    index: u16,
}

fn target<'a>(p: &'a Programs, bank: &str, number: i64) -> Result<Target<'a>, String> {
    let banks = p.banks.as_deref().unwrap_or_default();
    if banks.is_empty() {
        return Err("this driver lists no banks".into());
    }
    let (bank_index, b) = find_bank(p, bank).ok_or_else(|| {
        let names: Vec<&str> = banks.iter().map(|b| b.name.as_str()).collect();
        format!("no bank {bank:?}: {}", names.join(", "))
    })?;
    let first = b.first_number.unwrap_or(1);
    let last = first + b.size - 1;
    if !(first..=last).contains(&number) {
        return Err(format!("bank {} holds programs {first}..={last}", b.name));
    }
    let offset = (number - first) as u16;
    let pc = (b.first_pc.unwrap_or(0) as u16 + offset).min(127) as u8;
    let index = banks[..bank_index].iter().map(|b| b.size as u16).sum::<u16>() + offset;
    Ok(Target { bank_index, bank: b, number, pc, index })
}

fn channel_of(dev: &Device, t: Option<&Target>, channel: Option<u8>) -> u8 {
    channel.or(t.and_then(|t| t.bank.channel)).or(dev.midi_out.as_ref().and_then(|m| m.default_channel)).unwrap_or(0) & 0x0F
}

fn bindings(dev: &Device, t: Option<&Target>, ch: u8) -> Bindings {
    let mut bind = Bindings::new();
    bind.set_u7("channel", ch);
    if let Some(t) = t {
        bind.set_u7("msb", t.bank.msb.unwrap_or(0));
        bind.set_u7("lsb", t.bank.lsb.unwrap_or(0));
        bind.set_u7("bank", t.bank_index as u8);
        bind.set_u7("program", t.pc);
        bind.set_u14("index", t.index);
        bind.set_u14("number", t.number as u16);
    }
    if let Some(d) = dev.identity.as_ref().and_then(|i| i.device_id.as_ref()).and_then(|d| d.default) {
        bind.set_u7("dev", d as u8);
    }
    bind
}

/// What a `recall_script` or `dump.script` reads as `ctx`.
fn script_ctx(dev: &Device, t: Option<&Target>, ch: u8) -> serde_json::Value {
    let id = dev.identity.as_ref();
    serde_json::json!({
        "channel": ch,
        "dev": id.and_then(|i| i.device_id.as_ref()).and_then(|d| d.default),
        "family": id.and_then(|i| i.family.clone()).unwrap_or_default(),
        "member": id.and_then(|i| i.member.clone()).unwrap_or_default(),
        "bank": t.map(|t| t.bank_index), "bank_name": t.map(|t| t.bank.name.clone()),
        "msb": t.and_then(|t| t.bank.msb), "lsb": t.and_then(|t| t.bank.lsb),
        "program": t.map(|t| t.pc), "index": t.map(|t| t.index), "number": t.map(|t| t.number),
    })
}

/// The messages a script returns: a list of byte lists (or one byte list).
fn run_script(code: &str, ctx: &serde_json::Value) -> Result<Vec<Vec<u8>>, String> {
    let engine = crate::scripting::ScriptEngine::new().map_err(|e| e.to_string())?;
    let out = engine.call_json(code, ctx).map_err(|e| format!("script: {e:#}"))?;
    let bytes = |v: &serde_json::Value| -> Option<Vec<u8>> {
        v.as_array()?.iter().map(|b| b.as_u64().filter(|b| *b < 256).map(|b| b as u8)).collect()
    };
    match &out {
        serde_json::Value::Array(a) if a.iter().all(|m| m.is_array()) => a.iter().map(|m| bytes(m).ok_or("script: a message is not a byte list".to_string())).collect(),
        serde_json::Value::Array(_) => Ok(vec![bytes(&out).ok_or("script: not a byte list")?]),
        _ => Err("script: return a list of messages (byte lists)".into()),
    }
}

/// `frames` with the model's bytes written in, then filled in.
fn build(dev: &Device, frames: &[Vec<FrameToken>], bind: &Bindings) -> Result<Vec<Vec<u8>>, String> {
    frames.iter().map(|f| {
        let f = match &dev.identity { Some(id) => expand_identity(id, f), None => f.clone() };
        build_message(&f, bind).map_err(|e| format!("{}: programs: {e}", dev.device.name))
    }).collect()
}

/// The MIDI messages that recall program `number` of `bank`, on `channel`
/// (0-15; else the bank's, else `midi_out.default_channel`).
pub fn recall(dev: &Device, bank: &str, number: i64, channel: Option<u8>) -> Result<Vec<Vec<u8>>, String> {
    let p = dev.programs.as_ref().ok_or("this driver says nothing of its programs")?;
    let t = target(p, bank, number)?;
    let ch = channel_of(dev, Some(&t), channel);
    if let Some(code) = &p.recall_script {
        return run_script(code, &script_ctx(dev, Some(&t), ch));
    }
    let bind = bindings(dev, Some(&t), ch);
    let mut out = build(dev, p.before.as_deref().unwrap_or_default(), &bind)?;
    if p.program_change.unwrap_or(true) {
        if let Some(msb) = t.bank.msb { out.push(vec![0xB0 | ch, 0, msb & 0x7F]); }
        if let Some(lsb) = t.bank.lsb { out.push(vec![0xB0 | ch, 32, lsb & 0x7F]); }
        out.push(vec![0xC0 | ch, t.pc & 0x7F]);
    }
    out.extend(build(dev, p.after.as_deref().unwrap_or_default(), &bind)?);
    Ok(out)
}

/// The request that reads a sound back: of program `number` of `bank` when
/// given (a request naming the stored program), else of the edit buffer.
pub fn dump(dev: &Device, program: Option<(&str, i64)>, channel: Option<u8>) -> Result<Vec<Vec<u8>>, String> {
    let p = dev.programs.as_ref().ok_or("this driver says nothing of its programs")?;
    let d = p.dump.as_ref().ok_or("this driver does not say how a sound is read back")?;
    let t = program.map(|(b, n)| target(p, b, n)).transpose()?;
    let ch = channel_of(dev, t.as_ref(), channel);
    if let Some(code) = &d.script {
        return run_script(code, &script_ctx(dev, t.as_ref(), ch));
    }
    let kind = match d.kind {
        osd::DumpKind::None => "none",
        osd::DumpKind::Request => "request",
        osd::DumpKind::EditBuffer => "edit_buffer",
        osd::DumpKind::Manual => "manual",
    };
    let frame = d.request.as_ref().ok_or_else(|| format!("the {kind} request's bytes are not in this driver"))?;
    build(dev, std::slice::from_ref(frame), &bindings(dev, t.as_ref(), ch))
}

/// The request the device answers with its identity, its `{dev}` the
/// device-id rule's default; the Universal Identity Request when the driver
/// names none.
pub fn identity_request(dev: &Device) -> Vec<u8> {
    dev.identity.as_ref().and_then(|i| crate::detect::probe_of(i).ok().flatten())
        .map(|p| p.request).unwrap_or_else(|| crate::detect::IDENTITY_REQUEST.to_vec())
}

/// `/<prefix>/identity/reply <firmware> <hex>` when `msg` is the device's
/// identity reply, as its identity block reads it.
pub fn identity_reply(dev: &Device, msg: &[u8]) -> Option<OscMessage> {
    let r = crate::detect::recognise(dev.identity.as_ref()?, "", &[msg.to_vec()]).filter(|r| r.by_reply)?;
    let hex = msg.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
    Some(OscMessage { addr: full_osc_path(dev, IDENTITY_REPLY),
                      args: vec![OscType::String(r.firmware.unwrap_or_default()), OscType::String(hex)] })
}

/// The messages one of these addresses asks for, or None when `addr` is
/// none of this device's.
pub fn uniform_osc(dev: &Device, addr: &str, args: &[OscType]) -> Option<Result<Vec<Vec<u8>>, String>> {
    let int = |v: &OscType| match v {
        OscType::Int(i) => Some(*i as i64),
        OscType::Long(i) => Some(*i),
        OscType::Float(f) => Some(*f as i64),
        OscType::Double(f) => Some(*f as i64),
        _ => None,
    };
    let bank_arg = |v: Option<&OscType>| -> Option<String> {
        match v? { OscType::String(s) => Some(s.clone()), v => int(v).map(|i| i.to_string()) }
    };
    let channel = |i: usize| args.get(i).and_then(int).map(|c| c.clamp(0, 15) as u8);
    if addr == full_osc_path(dev, IDENTITY) && dev.device.transport.as_ref().is_none_or(|t| t.kind != osd::TransportKind::Osc) {
        return Some(Ok(vec![identity_request(dev)]));
    }
    dev.programs.as_ref()?;
    if addr == full_osc_path(dev, RECALL) {
        let Some(bank) = bank_arg(args.first()) else { return Some(Err(format!("{RECALL} <bank> <number> [channel]"))) };
        let Some(number) = args.get(1).and_then(int) else { return Some(Err("number: the program's number".into())) };
        return Some(recall(dev, &bank, number, channel(2)));
    }
    if addr == full_osc_path(dev, DUMP) {
        return Some(match (args.first(), args.get(1).and_then(int)) {
            (Some(OscType::String(bank)), Some(n)) => dump(dev, Some((bank, n)), channel(2)),
            (Some(b), Some(n)) if args.len() >= 2 => match bank_arg(Some(b)) {
                Some(bank) => dump(dev, Some((&bank, n)), channel(2)),
                None => Err(format!("{DUMP} [<bank> <number>] [channel]")),
            },
            _ => dump(dev, None, channel(0)),
        });
    }
    None
}

#[cfg(test)]
mod tests;
