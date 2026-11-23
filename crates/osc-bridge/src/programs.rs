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

use crate::device::{full_osc_path, kept, kept_list};
use crate::frame::{build_message, expand_identity, Bindings};
use osd::{Bank, Device, FrameToken, Programs};
use rosc::{OscMessage, OscType};
use std::time::Duration;

/// The arguments a `programs` frame may name (`{"arg": name}`).
pub const PLACEHOLDERS: [&str; 7] = ["channel", "msb", "lsb", "bank", "program", "index", "number"];

/// The routes, after the device's prefix.
pub const RECALL: &str = "/program/recall";
pub const DUMP: &str = "/program/dump";
pub const IDENTITY: &str = "/identity";
pub const IDENTITY_REPLY: &str = "/identity/reply";

/// A bank of `p` by its name (any case) or its place in the list.
pub fn find_bank<'a>(p: &'a Programs, bank: &str) -> Option<(usize, &'a Bank)> {
    let banks = kept_list(&p.banks);
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
    let banks = kept_list(&p.banks);
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
    if let Some(d) = dev.identity.as_ref().and_then(|i| kept(&i.device_id)).and_then(|d| d.default) {
        bind.set_u7("dev", d as u8);
    }
    bind
}

/// What a `recall_script` or `dump.script` reads as `ctx`.
fn script_ctx(dev: &Device, t: Option<&Target>, ch: u8) -> serde_json::Value {
    let id = dev.identity.as_ref();
    serde_json::json!({
        "channel": ch,
        "dev": id.and_then(|i| kept(&i.device_id)).and_then(|d| d.default),
        "family": id.map(|i| kept_list(&i.family)).unwrap_or_default(),
        "member": id.map(|i| kept_list(&i.member)).unwrap_or_default(),
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
    if let Some(code) = kept(&p.recall_script) {
        return run_script(code, &script_ctx(dev, Some(&t), ch));
    }
    let bind = bindings(dev, Some(&t), ch);
    let mut out = build(dev, kept_list(&p.before), &bind)?;
    if kept(&p.program_change).copied().unwrap_or(true) {
        if let Some(msb) = t.bank.msb { out.push(vec![0xB0 | ch, 0, msb & 0x7F]); }
        if let Some(lsb) = t.bank.lsb { out.push(vec![0xB0 | ch, 32, lsb & 0x7F]); }
        out.push(vec![0xC0 | ch, t.pc & 0x7F]);
    }
    out.extend(build(dev, kept_list(&p.after), &bind)?);
    Ok(out)
}

/// The request that reads a sound back: of program `number` of `bank` when
/// given (a request naming the stored program), else of the edit buffer.
pub fn dump(dev: &Device, program: Option<(&str, i64)>, channel: Option<u8>) -> Result<Vec<Vec<u8>>, String> {
    let p = dev.programs.as_ref().ok_or("this driver says nothing of its programs")?;
    let d = kept(&p.dump).ok_or("this driver does not say how a sound is read back")?;
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

/// What waits for a pause: the real clock sleeps, a test's records.
pub trait Clock {
    fn sleep(&mut self, d: Duration);
}

/// The clock of the machine.
pub struct SystemClock;
impl Clock for SystemClock {
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// The pause `programs.delay_ms` declares between two messages; none when absent or 0.
fn pause_of(dev: &Device) -> Option<Duration> {
    kept(&dev.programs.as_ref()?.delay_ms).copied().filter(|d| *d > 0).map(|d| Duration::from_millis(d as u64))
}

/// `recall`, the declared pause (`delay_ms`) asked of `clock` between two
/// messages, none after the last. A driver stating no pause waits nothing.
pub fn recall_paced(dev: &Device, bank: &str, number: i64, channel: Option<u8>, clock: &mut dyn Clock) -> Result<Vec<Vec<u8>>, String> {
    let out = recall(dev, bank, number, channel)?;
    if let Some(pause) = pause_of(dev) {
        for _ in 1..out.len() {
            clock.sleep(pause);
        }
    }
    Ok(out)
}

/// `dump`, then the reply bound (`dump.reply_timeout_ms`) asked of `clock`
/// once for the sequence. A driver stating none waits nothing.
pub fn dump_paced(dev: &Device, program: Option<(&str, i64)>, channel: Option<u8>, clock: &mut dyn Clock) -> Result<Vec<Vec<u8>>, String> {
    let out = dump(dev, program, channel)?;
    let timeout = dev.programs.as_ref().and_then(|p| kept(&p.dump)).and_then(|d| d.reply_timeout_ms).filter(|t| *t > 0);
    if let Some(t) = timeout {
        clock.sleep(Duration::from_millis(t as u64));
    }
    Ok(out)
}

/// The messages that select the slot `number` of `bank` for a write, refused
/// before any message when the descriptor says the bank cannot be written
/// (`writable: false`; absent means writable). The error names the bank.
pub fn store(dev: &Device, bank: &str, number: i64, channel: Option<u8>) -> Result<Vec<Vec<u8>>, String> {
    let p = dev.programs.as_ref().ok_or("this driver says nothing of its programs")?;
    let t = target(p, bank, number)?;
    if t.bank.writable == Some(false) {
        return Err(format!("bank {} cannot be written: the driver says writable is false", t.bank.name));
    }
    recall(dev, bank, number, channel)
}

/// The byte spans of the parts of a transfer that vary from one message to
/// the next (channel, device number, header, checksum, bank, program).
fn transport_spans(t: &osd::SoundTransfer) -> Vec<(usize, usize)> {
    let e = &t.elements;
    [&e.channel, &e.device_number, &e.header, &e.checksum, &e.bank_number, &e.program_number]
        .into_iter()
        .flatten()
        .filter_map(|el| Some((usize::try_from(el.from?).ok()?, usize::try_from(el.length?).ok()?)))
        .collect()
}

fn transfer_of<'a>(dev: &'a Device, frame: &[u8]) -> Option<&'a osd::SoundTransfer> {
    dev.sound_transfers.iter().flatten().find(|t| {
        let spans = transport_spans(t);
        let masked = |at: usize| spans.iter().any(|(f, l)| (*f..f + l).contains(&at));
        frame.len() == t.match_frame.len()
            && t.match_frame.iter().zip(frame).enumerate()
                .all(|(at, (token, b))| masked(at) || !matches!(token, FrameToken::Byte(x) if x != b))
    })
}

/// The id of the sound transfer `frame` is, whatever its channel and device
/// number (and the other parts that vary); None when it is none of them.
pub fn recognise_transfer(dev: &Device, frame: &[u8]) -> Option<String> {
    transfer_of(dev, frame).map(|t| t.id.clone())
}

/// `frame` ready to send: its channel written as `channel` and, when given,
/// its device number as `device_number`; the rest as received. Refused when
/// no declared transfer recognises it.
pub fn normalise_transfer(dev: &Device, frame: &[u8], channel: u8, device_number: Option<u8>) -> Result<Vec<u8>, String> {
    let t = transfer_of(dev, frame).ok_or("this message is none of the driver's sound transfers")?;
    let mut out = frame.to_vec();
    let mut put = |el: &Option<osd::Element>, v: u8| {
        if let Some(from) = el.as_ref().and_then(|e| e.from).and_then(|f| usize::try_from(f).ok()) {
            if let Some(b) = out.get_mut(from) { *b = v; }
        }
    };
    put(&t.elements.channel, channel);
    if let Some(n) = device_number { put(&t.elements.device_number, n); }
    Ok(out)
}

/// Writes a file's messages to a device: the slot selection of `target` (a
/// bank and number; refused before any message when the bank cannot be
/// written), then each message of `msgs` with its channel (and device
/// number) written as the configured ones when a declared sound transfer
/// recognises it, else as it is. The pause the driver declares
/// (`delay_ms`) is asked of `clock` between two messages. The number of
/// messages sent, or the first refusal.
pub fn restore(dev: &Device, msgs: &[Vec<u8>], target: Option<(&str, i64)>, channel: u8, device_number: Option<u8>,
               clock: &mut dyn Clock, send: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<usize, String> {
    let mut out = match target {
        Some((bank, number)) => store(dev, bank, number, Some(channel))?,
        None => Vec::new(),
    };
    for m in msgs {
        out.push(normalise_transfer(dev, m, channel, device_number).unwrap_or_else(|_| m.clone()));
    }
    let pause = pause_of(dev);
    for (i, m) in out.iter().enumerate() {
        if i > 0 {
            if let Some(p) = pause { clock.sleep(p); }
        }
        send(m).map_err(|e| format!("send message {}: {e}", i + 1))?;
    }
    Ok(out.len())
}

/// The messages of `received` the driver's sound transfers recognise, on
/// any channel and from any device number; all of them when the driver
/// declares no sound transfer.
pub fn backup_filter(dev: &Device, received: &[Vec<u8>]) -> Vec<Vec<u8>> {
    if dev.sound_transfers.as_ref().is_none_or(|t| t.is_empty()) {
        return received.to_vec();
    }
    received.iter().filter(|m| recognise_transfer(dev, m).is_some()).cloned().collect()
}

/// The request the device answers with its identity, its `{dev}` the
/// device-id rule's default; the Universal Identity Request when the driver
/// names none.
pub fn identity_request(dev: &Device) -> SentRequest {
    let universal = |fault| SentRequest { bytes: crate::recognition::IDENTITY_REQUEST.to_vec(), own: false, fault };
    match dev.identity.as_ref().map(crate::recognition::probe_of) {
        Some(Ok(Some(p))) => SentRequest { bytes: p.request, own: true, fault: None },
        Some(Err(_)) => universal(crate::device::faults(dev).into_iter().next()),
        _ => universal(None),
    }
}

/// The identity request sent, which of the two it is (`own`: the driver's,
/// else the universal one), and the driver's fault when the universal one
/// stands in for a request that could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentRequest {
    pub bytes: Vec<u8>,
    pub own: bool,
    pub fault: Option<crate::device::DeviceFault>,
}

/// `/<prefix>/identity/reply <firmware> <hex>` when `msg` is the device's
/// identity reply, as its identity block reads it.
pub fn identity_reply(dev: &Device, msg: &[u8]) -> Option<OscMessage> {
    let r = crate::recognition::recognise(dev.identity.as_ref()?, "", None, &[msg.to_vec()]).filter(|r| r.by_reply)?;
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
        return Some(Ok(vec![identity_request(dev).bytes]));
    }
    dev.programs.as_ref()?;
    if addr == full_osc_path(dev, RECALL) {
        let Some(bank) = bank_arg(args.first()) else { return Some(Err(format!("{RECALL} <bank> <number> [channel]"))) };
        let Some(number) = args.get(1).and_then(int) else { return Some(Err("number: the program's number".into())) };
        return Some(recall_paced(dev, &bank, number, channel(2), &mut SystemClock));
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
