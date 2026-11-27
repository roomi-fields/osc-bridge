//! One generic gesture puts the export of a device on a recognised target
//! device. What the target receives, and the way it receives it, are read in
//! its own descriptor (`resources`): the format the resource exports, the path
//! of the file, and the steps of a file written by SysEx. Nothing here names a
//! device.
//!
//! A write on the target is always taken for one that may overwrite a file
//! (the device is not read first): without the person's consent nothing is
//! sent, and what would be written is returned.

use std::collections::HashMap;
use std::time::Duration;

use osd::{Coding, Device, FileItem, FramePart, FrameToken, NumberAs, ReplyToken, SendToken, Step, StepKind};
use serde_json::Value;

use crate::export::{self, Format, Loss};
use crate::frame::{self, Bindings};
use crate::programs::Clock;
use crate::midi_port::Midi;

/// What the gesture did, or, without consent, would do.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    /// The format of the export, by its name.
    pub format: String,
    /// What carries the file (`sysex`).
    pub way: String,
    /// The path of the file on the target.
    pub path: String,
    /// The size of the file, in bytes.
    pub size: usize,
    /// What the format could not carry.
    pub lost: Vec<Loss>,
    /// Whether the file was written.
    pub written: bool,
    /// The messages sent.
    pub messages: usize,
}

/// The `export --on` command: load both descriptors, take the MIDI from `open` (the guarded
/// `midi_port::open_hardware` for the program, the shared fake hardware for a test), and put the
/// export of `source` on `target`. A MIDI system that cannot be opened is an error naming it
/// and the cause.
pub fn install_command(
    source: &std::path::Path,
    target: &std::path::Path,
    confirm_overwrite: bool,
    open: crate::midi_port::Opener,
) -> anyhow::Result<Installed> {
    let source = crate::device::load(source)?;
    let target = crate::device::load(target)?;
    let mut midi = open().map_err(|e| anyhow::anyhow!("{} cannot be reached (MIDI system): {e}", export::device_title(&target)))?;
    install(&source, &target, confirm_overwrite, midi.as_mut(), &mut crate::programs::SystemClock).map_err(anyhow::Error::msg)
}

/// Put the export of `source` on `target`, by the way `target`'s descriptor
/// declares. Without `confirm_overwrite` nothing is sent.
pub fn install(
    source: &Device,
    target: &Device,
    confirm_overwrite: bool,
    midi: &mut dyn Midi,
    clock: &mut dyn Clock,
) -> Result<Installed, String> {
    let name = export::device_title(target);
    let (resource, write) = target
        .resources
        .iter()
        .flatten()
        .filter(|r| r.can_set != osd::CanSet::None)
        .find_map(|r| r.via.as_ref()?.file_write.as_ref().filter(|w| w.steps.is_some()).map(|w| (r, w)))
        .ok_or_else(|| format!("{name} declares no format it receives by a file written: nothing to put on it"))?;
    if write.carried_by != Some(osd::CarriedBy::Sysex) {
        return Err(format!("{name} declares no way it receives {} that is known here (carried_by sysex)", resource.export));
    }
    let way = format!("{:?}", osd::CarriedBy::Sysex).to_lowercase();
    let format: Format = resource.export.parse().map_err(|e: anyhow::Error| format!("{name}: {e}"))?;
    let path = osd::resource_path(target, &resource.resource, source).map_err(|e| format!("{name}: {e}"))?;
    let port = port_of(target, &midi.outputs()).ok_or_else(|| format!("{name} is not plugged: no MIDI output port is its own"))?;
    let out = export::export(source, format).map_err(|e| format!("the export of {} as {} cannot be built: {e}", export::device_title(source), format.name()))?;
    let file = out.value.into_bytes();
    let mut done = Installed { format: format.name().into(), way, path, size: file.len(), lost: out.lost, written: false, messages: 0 };
    if !confirm_overwrite {
        return Ok(done);
    }
    let steps = write.steps.as_deref().unwrap_or_default();
    done.messages = play(steps, &done.path, &file, &port, midi, clock, &name)?;
    done.written = true;
    Ok(done)
}

/// The output port that is the target's: one a port rule of its descriptor names
/// (a rule without direction or of the output direction), or, for a descriptor
/// without rules, a port name that holds its own name.
fn port_of(target: &Device, outputs: &[String]) -> Option<String> {
    let rules = target.identity.as_ref().map(|i| crate::device::kept_list(&i.ports)).unwrap_or_default();
    let own = [target.device.name.clone()];
    outputs.iter().find(|p| if rules.is_empty() {
        crate::recognition::port_matches(&own, p)
    } else {
        crate::recognition::rules_match(rules, None, p)
    }).cloned()
}

/// The facts a message of a step is made of.
struct Facts<'a> {
    path: &'a str,
    size: usize,
    offset: usize,
    chunk: &'a [u8],
    captured: &'a HashMap<String, String>,
}

fn play(steps: &[Step], path: &str, file: &[u8], port: &str, midi: &mut dyn Midi, clock: &mut dyn Clock, name: &str) -> Result<usize, String> {
    let mut sent = 0usize;
    let mut captured: HashMap<String, String> = HashMap::new();
    for kind in [StepKind::Open, StepKind::Write, StepKind::Close] {
        let step = steps.iter().find(|s| s.step == kind).ok_or_else(|| format!("{name}: no {} step", step_name(kind)))?;
        let chunks: Vec<&[u8]> = if kind == StepKind::Write {
            let n = step.chunk_bytes.unwrap_or(1).max(1) as usize;
            file.chunks(n).collect()
        } else {
            vec![&[][..]]
        };
        for (i, chunk) in chunks.iter().enumerate() {
            let offset = i * step.chunk_bytes.unwrap_or(0).max(0) as usize;
            let facts = Facts { path, size: file.len(), offset, chunk, captured: &captured };
            let message = build(step, &facts).map_err(|e| stopped(kind, name, sent, e))?;
            midi.send(port, &[message], &format!("write {path} on {name}")).map_err(|e| stopped(kind, name, sent, e))?;
            sent += 1;
            if let Some(reply) = &step.reply {
                let wait = Duration::from_millis(step.reply_timeout_ms.unwrap_or(0).max(0) as u64);
                let got = midi.received(wait);
                let mut found = None;
                for (from, m) in &got {
                    if from != port {
                        continue;
                    }
                    if let Some(c) = recognise(reply, m).map_err(|e| stopped(kind, name, sent, e))? {
                        found = Some(c);
                        break;
                    }
                }
                match found {
                    Some(c) => captured.extend(c),
                    None => return Err(stopped(kind, name, sent, format!("no reply within {} ms", wait.as_millis()))),
                }
            }
            if let Some(ms) = step.delay_ms.filter(|ms| *ms > 0) {
                clock.sleep(Duration::from_millis(ms as u64));
            }
        }
    }
    Ok(sent)
}

/// The error of a send that fails on the way: the step reached and the
/// messages already sent.
fn stopped(kind: StepKind, name: &str, sent: usize, why: String) -> String {
    format!("{} step of the file write on {name}: {why} ({sent} message(s) already sent, no retry, nothing undone)", step_name(kind))
}

/// The word a descriptor gives a step: `open`, `write`, `close`.
fn step_name(kind: StepKind) -> String {
    format!("{kind:?}").to_lowercase()
}

fn build(step: &Step, facts: &Facts) -> Result<Vec<u8>, String> {
    let none = Bindings::new();
    let mut out = Vec::new();
    for tok in &step.send {
        match tok {
            SendToken::Frame(t) => frame::append_token(t, &none, &mut out).map_err(|e| e.to_string())?,
            SendToken::Text(s) => out.extend_from_slice(s.as_bytes()),
            SendToken::File { item, as_ } => match item {
                FileItem::Path => out.extend_from_slice(facts.path.as_bytes()),
                FileItem::Chunk => {
                    let coding = step.coding.ok_or("a chunk is sent by a step without a coding")?;
                    out.extend(code(facts.chunk, coding));
                }
                FileItem::Size => number(facts.size, *as_, &mut out)?,
                FileItem::Offset => number(facts.offset, *as_, &mut out)?,
                FileItem::ChunkSize => number(facts.chunk.len(), *as_, &mut out)?,
            },
            SendToken::Captured { name, .. } => {
                let v = facts.captured.get(name).ok_or_else(|| format!("'{name}' was not captured from the reply of the open step"))?;
                out.extend_from_slice(v.as_bytes());
            }
        }
    }
    Ok(out)
}

/// A number as ASCII digits; refused written any other way.
fn number(n: usize, as_: Option<NumberAs>, out: &mut Vec<u8>) -> Result<(), String> {
    match as_ {
        Some(NumberAs::Decimal) => {
            out.extend_from_slice(n.to_string().as_bytes());
            Ok(())
        }
        None => Err("a number is written `as` decimal".into()),
    }
}

/// `chunk` in 7 bits: for each group of up to 7 bytes, a byte of their high
/// bits (the first byte's at bit 6), then the 7 low bits of each. The two
/// codings of OSD give the same bytes.
fn code(chunk: &[u8], coding: Coding) -> Vec<u8> {
    let (Coding::Mcoded7 | Coding::MsbFirst7in8) = coding;
    let mut out = Vec::with_capacity(chunk.len() + chunk.len() / 7 + 1);
    for group in chunk.chunks(7) {
        let high = group.iter().enumerate().fold(0u8, |acc, (i, b)| acc | ((b >> 7) << (6 - i)));
        out.push(high);
        out.extend(group.iter().map(|b| b & 0x7F));
    }
    out
}

/// What `message` captures when it fits `reply` (a prefix of the message, its
/// closing F7 left out); None when it does not fit.
fn recognise(reply: &[ReplyToken], message: &[u8]) -> Result<Option<HashMap<String, String>>, String> {
    let body = message.strip_suffix(&[0xF7]).unwrap_or(message);
    let mut at = 0usize;
    let mut got = HashMap::new();
    for tok in reply {
        match tok {
            ReplyToken::Frame(FrameToken::Byte(b)) => {
                if body.get(at) != Some(b) {
                    return Ok(None);
                }
                at += 1;
            }
            ReplyToken::Frame(FrameToken::Part(FramePart::Any | FramePart::Capture(_))) => {
                if at >= body.len() {
                    return Ok(None);
                }
                at += 1;
            }
            ReplyToken::Frame(_) => return Err("a reply element this gesture does not recognise".into()),
            ReplyToken::CaptureJson(key) => {
                let Ok(json) = serde_json::from_slice::<Value>(&body[at.min(body.len())..]) else { return Ok(None) };
                let text = match json.get(key) {
                    Some(Value::Number(n)) => n.to_string(),
                    Some(Value::String(s)) => s.clone(),
                    _ => return Ok(None),
                };
                got.insert(key.clone(), text);
                at = body.len();
            }
        }
    }
    Ok(Some(got))
}
