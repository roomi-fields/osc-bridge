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

use crate::device::{Bank, Device, FrameToken, Programs};
use crate::frame::{build_message, Bindings};
use rosc::{OscMessage, OscType};

/// The routes, after the device's prefix.
pub const RECALL: &str = "/program/recall";
pub const DUMP: &str = "/program/dump";
pub const IDENTITY: &str = "/identity";
pub const IDENTITY_REPLY: &str = "/identity/reply";

/// A bank of `p` by its name (any case) or its place in the list.
pub fn find_bank<'a>(p: &'a Programs, bank: &str) -> Option<(usize, &'a Bank)> {
    p.banks.iter().enumerate().find(|(_, b)| b.name.eq_ignore_ascii_case(bank))
        .or_else(|| bank.parse::<usize>().ok().and_then(|i| p.banks.get(i).map(|b| (i, b))))
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
    if p.banks.is_empty() {
        return Err("this driver lists no banks".into());
    }
    let (bank_index, b) = find_bank(p, bank).ok_or_else(|| {
        let names: Vec<&str> = p.banks.iter().map(|b| b.name.as_str()).collect();
        format!("no bank {bank:?}: {}", names.join(", "))
    })?;
    let first = b.first_number as i64;
    let last = first + b.size as i64 - 1;
    if !(first..=last).contains(&number) {
        return Err(format!("bank {} holds programs {first}..={last}", b.name));
    }
    let offset = (number - first) as u16;
    let pc = (b.first_pc as u16 + offset).min(127) as u8;
    let index = p.banks[..bank_index].iter().map(|b| b.size).sum::<u16>() + offset;
    Ok(Target { bank_index, bank: b, number, pc, index })
}

fn channel_of(dev: &Device, t: Option<&Target>, channel: Option<u8>) -> u8 {
    channel.or(t.and_then(|t| t.bank.channel)).or(dev.midi_out.as_ref().map(|m| m.default_channel)).unwrap_or(0) & 0x0F
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
    if let Some(d) = dev.identity.as_ref().and_then(|i| i.device_id.as_ref()) {
        bind.set_u7("dev", d.default);
    }
    bind
}

/// What a `recall_script` or `dump.script` reads as `ctx`.
fn script_ctx(dev: &Device, t: Option<&Target>, ch: u8) -> serde_json::Value {
    let id = dev.identity.as_ref();
    serde_json::json!({
        "channel": ch,
        "dev": id.and_then(|i| i.device_id.as_ref()).map(|d| d.default),
        "family": id.map(|i| i.family.clone()).unwrap_or_default(),
        "member": id.map(|i| i.member.clone()).unwrap_or_default(),
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
        let f = match &dev.identity { Some(id) => id.expand(f), None => f.clone() };
        build_message(&f, bind).map_err(|e| e.to_string())
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
    let mut out = build(dev, &p.before, &bind)?;
    if p.program_change {
        if let Some(msb) = t.bank.msb { out.push(vec![0xB0 | ch, 0, msb & 0x7F]); }
        if let Some(lsb) = t.bank.lsb { out.push(vec![0xB0 | ch, 32, lsb & 0x7F]); }
        out.push(vec![0xC0 | ch, t.pc & 0x7F]);
    }
    out.extend(build(dev, &p.after, &bind)?);
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
    let frame = d.request.as_ref().ok_or_else(|| format!("the {} request's bytes are not in this driver", d.kind))?;
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
    Some(OscMessage { addr: dev.full_osc_path(IDENTITY_REPLY),
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
    if addr == dev.full_osc_path(IDENTITY) && dev.device.transport.as_ref().is_none_or(|t| t.kind != "osc") {
        return Some(Ok(vec![identity_request(dev)]));
    }
    dev.programs.as_ref()?;
    if addr == dev.full_osc_path(RECALL) {
        let Some(bank) = bank_arg(args.first()) else { return Some(Err(format!("{RECALL} <bank> <number> [channel]"))) };
        let Some(number) = args.get(1).and_then(int) else { return Some(Err("number: the program's number".into())) };
        return Some(recall(dev, &bank, number, channel(2)));
    }
    if addr == dev.full_osc_path(DUMP) {
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
mod tests {
    use super::*;

    fn dev(programs: &str) -> Device {
        serde_json::from_str(&format!(r#"{{"device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box"}},
            "midi_out":{{"default_channel":2}},"identity":{{"device_id":{{"default":16}},"family":[81,1],"reply":[240,126]}},
            "programs":{programs}}}"#)).unwrap()
    }

    #[test]
    fn a_bank_and_a_number_become_the_bank_select_and_the_program_change() {
        let d = dev(r#"{"banks":[{"name":"User","msb":81,"lsb":0,"size":128},
            {"name":"Preset A","msb":81,"lsb":64,"size":128,"display":"001-128"},
            {"name":"B","lsb":1,"first_pc":1,"size":72,"first_number":129}]}"#);
        assert_eq!(recall(&d, "preset a", 1, None).unwrap(), vec![vec![0xB2, 0, 81], vec![0xB2, 32, 64], vec![0xC2, 0]]);
        assert_eq!(recall(&d, "1", 128, Some(0)).unwrap(), vec![vec![0xB0, 0, 81], vec![0xB0, 32, 64], vec![0xC0, 127]]);
        assert_eq!(recall(&d, "B", 130, None).unwrap(), vec![vec![0xB2, 32, 1], vec![0xC2, 2]], "CC 32 only, PC from 1");
        assert!(recall(&d, "B", 128, None).unwrap_err().contains("129..=200"));
        assert!(recall(&d, "Card", 1, None).unwrap_err().contains("User, Preset A, B"));
    }

    #[test]
    fn a_recall_sequence_wraps_the_program_change_or_replaces_it() {
        let d = dev(r#"{"before":[[240,66,"{channel:u7+0x30}",117,78,2,247]],"after":[[240,65,"{dev}","{index:u14_lsb}","{index:u14_msb}",247]],
            "banks":[{"name":"A","size":100},{"name":"B","size":100}]}"#);
        let m = recall(&d, "B", 100, None).unwrap();
        assert_eq!(m, vec![vec![0xF0, 0x42, 0x32, 0x75, 0x4E, 2, 0xF7], vec![0xC2, 99], vec![0xF0, 0x41, 16, 199 & 0x7F, 1, 0xF7]]);
        let song = dev(r#"{"program_change":false,"before":[[243,"{program}"]],"banks":[{"name":"Songs","size":100,"first_number":0}]}"#);
        assert_eq!(recall(&song, "Songs", 42, None).unwrap(), vec![vec![0xF3, 42]], "Song Select, no program change");
    }

    #[test]
    fn a_dump_request_takes_the_models_bytes_and_a_checksum() {
        let d = dev(r#"{"banks":[{"name":"A","size":64}],
            "dump":{"kind":"request","request":[240,66,"{channel:u7+0x30}","{family:0}",28,"{index}","{checksum:twos7@3}",247]}}"#);
        // 51 + 1C + 05 = 0x72; 128 - 0x72 = 0x0E.
        assert_eq!(dump(&d, Some(("A", 6)), None).unwrap(), vec![vec![0xF0, 0x42, 0x32, 81, 0x1C, 5, 0x0E, 0xF7]]);
        let edit = dev(r#"{"dump":{"kind":"edit_buffer","request":[240,66,"{channel:u7+0x30}","{family:0}",16,247]}}"#);
        assert_eq!(dump(&edit, None, Some(0)).unwrap(), vec![vec![0xF0, 0x42, 0x30, 81, 0x10, 0xF7]]);
        assert!(dump(&dev(r#"{"dump":{"kind":"request"}}"#), None, None).unwrap_err().contains("not in this driver"));
    }

    #[test]
    fn lua_writes_a_recall_no_frame_can() {
        // A file named by its bank letter and number, as text (Behringer UB-Xa style).
        let d = dev(r#"{"banks":[{"name":"A","msb":0,"lsb":0,"size":128}],
            "recall_script":"local name = string.format('%s%03d', bank_name, number)\nlocal m = {0xF0, 0x7D}\nfor i = 1, #name do m[#m + 1] = name:byte(i) end\nm[#m + 1] = 0xF7\nreturn {m}"}"#);
        assert_eq!(recall(&d, "A", 1, None).unwrap(), vec![[&[0xF0, 0x7D][..], b"A001", &[0xF7]].concat()]);
    }

    #[test]
    fn the_same_addresses_on_every_device() {
        let d = dev(r#"{"banks":[{"name":"A","size":16},{"name":"B","first_pc":16,"size":16}],"dump":{"kind":"edit_buffer","request":[240,1,"{channel}",247]}}"#);
        let call = |addr: &str, args: Vec<OscType>| uniform_osc(&d, addr, &args);
        assert_eq!(call("/box/program/recall", vec![OscType::String("B".into()), OscType::Int(3)]).unwrap().unwrap(), vec![vec![0xC2, 18]]);
        assert_eq!(call("/box/program/recall", vec![OscType::Int(1), OscType::Int(3), OscType::Int(9)]).unwrap().unwrap(), vec![vec![0xC9, 18]]);
        assert!(call("/box/program/recall", vec![OscType::String("B".into())]).unwrap().is_err());
        assert_eq!(call("/box/program/dump", vec![]).unwrap().unwrap(), vec![vec![0xF0, 1, 2, 0xF7]]);
        assert_eq!(call("/box/identity", vec![]).unwrap().unwrap(), vec![crate::detect::IDENTITY_REQUEST.to_vec()]);
        assert!(call("/box/program_change", vec![]).is_none());
        let reply = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x41];
        let m = identity_reply(&d, &reply).unwrap();
        assert_eq!((m.addr.as_str(), &m.args[1]), ("/box/identity/reply", &OscType::String("F0 7E 00 06 02 41".into())));
        assert!(identity_reply(&d, &[0xF0, 0x41, 0xF7]).is_none());
    }
}
