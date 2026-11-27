//! SysEx frame assembly and matching.

use anyhow::{anyhow, bail, Result};
use osd::{Algorithm, Device, Encoding, FramePart, FrameToken, IdentityPart};
use std::collections::HashMap;

/// A set of named values (int or bytes) bound during a command build.
#[derive(Debug, Clone, Default)]
pub struct Bindings(pub HashMap<String, Value>);

#[derive(Debug, Clone)]
pub enum Value {
    /// 7-bit value (0..=127)
    U7(u8),
    /// 14-bit value (0..=16383)
    U14(u16),
    /// Signed 32-bit value, sent as five 7-bit groups (Korg M3 and later)
    S32(i32),
    /// Raw byte sequence inserted at a placeholder position
    Bytes(Vec<u8>),
}

impl Bindings {
    pub fn new() -> Self { Self::default() }
    pub fn set_u7(&mut self, k: &str, v: u8) { self.0.insert(k.into(), Value::U7(v & 0x7F)); }
    pub fn set_u14(&mut self, k: &str, v: u16) { self.0.insert(k.into(), Value::U14(v & 0x3FFF)); }
    pub fn set_s32(&mut self, k: &str, v: i32) { self.0.insert(k.into(), Value::S32(v)); }
    pub fn set_bytes(&mut self, k: &str, v: Vec<u8>) { self.0.insert(k.into(), Value::Bytes(v)); }
    pub fn get(&self, k: &str) -> Option<&Value> { self.0.get(k) }
}

/// A checksum element of a frame: its algorithm, the offset it starts at
/// (F0 = 0) and, optionally, how many bytes it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checksum {
    pub algorithm: Algorithm,
    pub from: usize,
    pub length: Option<usize>,
}

impl Checksum {
    /// The checksum a frame element stands for, if it is one.
    pub fn of_token(tok: &FrameToken) -> Option<Checksum> {
        match tok {
            FrameToken::Part(FramePart::Checksum { algorithm, from, length }) => {
                Some(Checksum { algorithm: *algorithm, from: *from as usize, length: length.map(|l| l as usize) })
            }
            _ => None,
        }
    }

    /// How the checksum reads in a message: `checksum twos7 from 5 length 3`.
    pub fn describe(&self) -> String {
        let name = algorithm_name(self.algorithm);
        match self.length {
            Some(l) => format!("checksum {name} from {} length {l}", self.from),
            None => format!("checksum {name} from {}", self.from),
        }
    }

    /// The checksum byte of `written`, the message so far (nothing after the
    /// checksum, never the closing F7).
    pub fn compute(&self, written: &[u8]) -> Result<u8> {
        let end = match self.length {
            Some(l) => self.from.checked_add(l).filter(|e| *e <= written.len())
                .ok_or_else(|| anyhow!("{}: {} bytes from offset {} run past the {} bytes written", self.describe(), l, self.from, written.len()))?,
            None => written.len(),
        };
        let covered = written.get(self.from..end)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| anyhow!("{}: offset {} is past the {} bytes written", self.describe(), self.from, written.len()))?;
        Ok(checksum_byte(self.algorithm, covered))
    }
}

/// Assemble a full SysEx frame: the device's header, then the body produced by
/// substituting bindings into `tokens`, closed by `close_sysex`.
pub fn build_frame(device: &Device, tokens: &[FrameToken], bindings: &Bindings) -> Result<Vec<u8>> {
    let (mut out, slots) = build_frame_deferred(device, tokens, bindings)?;
    finish_checksums(&mut out, &slots)?;
    Ok(out)
}

/// The device's SysEx header, empty when it declares none.
pub(crate) fn sysex_header(device: &Device) -> &[u8] {
    device.sysex.as_ref().and_then(|s| s.header.as_deref()).unwrap_or_default()
}

/// Close a message the device's header opens with F0: F7 is added unless it
/// already ends with it.
fn close_sysex(device: &Device, message: &mut Vec<u8>) {
    if sysex_header(device).first() == Some(&0xF0) && message.last() != Some(&0xF7) {
        message.push(0xF7);
    }
}

/// `body` after the device's SysEx header, the message closed.
pub fn wrap_sysex(device: &Device, body: &[u8]) -> Vec<u8> {
    let mut buf = [sysex_header(device), body].concat();
    close_sysex(device, &mut buf);
    buf
}

/// Where a command's frame holds a checksum, left for after the command's script.
#[derive(Debug, Clone)]
pub struct ChecksumSlot {
    pub pos: usize,
    pub checksum: Checksum,
}

/// Like `build_frame`, but each checksum is checked as computable then written
/// as 0x00, and its place returned: the checksums are computed by
/// `finish_checksums` once the command's script has run.
pub fn build_frame_deferred(device: &Device, tokens: &[FrameToken], bindings: &Bindings) -> Result<(Vec<u8>, Vec<ChecksumSlot>)> {
    let mut out = Vec::with_capacity(32);
    let mut slots = Vec::new();
    out.extend_from_slice(sysex_header(device));
    for tok in tokens {
        match Checksum::of_token(tok) {
            Some(c) => {
                c.compute(&out)?;
                slots.push(ChecksumSlot { pos: out.len(), checksum: c });
                out.push(0);
            }
            None => emit_token(tok, bindings, &mut out)?,
        }
    }
    close_sysex(device, &mut out);
    Ok((out, slots))
}

/// Write each slot's checksum, left to right, over the final bytes of `frame`
/// (a checksum covering an earlier slot sees its final value).
pub fn finish_checksums(frame: &mut [u8], slots: &[ChecksumSlot]) -> Result<()> {
    for s in slots {
        let b = s.checksum.compute(&frame[..s.pos])?;
        frame[s.pos] = b;
    }
    Ok(())
}

/// A whole message from `tokens` (F0 to F7 written in them, nothing added);
/// its checksums are computed by `Checksum`.
pub fn build_message(tokens: &[FrameToken], bindings: &Bindings) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(tokens.len());
    for tok in tokens {
        match Checksum::of_token(tok) {
            Some(c) => {
                let b = c.compute(&out)?;
                out.push(b);
            }
            None => emit_token(tok, bindings, &mut out)?,
        }
    }
    Ok(out)
}

/// The word that names an algorithm in a driver.
fn algorithm_name(a: Algorithm) -> &'static str {
    match a {
        Algorithm::Twos7 => "twos7",
        Algorithm::Sum7 => "sum7",
        Algorithm::Xor7 => "xor7",
    }
}

/// The one computation of a checksum byte, 7 bits (Electra One definitions):
/// `twos7` `(128 - (sum mod 128)) & 0x7F`, `sum7` `sum & 0x7F`, `xor7` the XOR of the bytes `& 0x7F`.
pub fn checksum_byte(algorithm: Algorithm, bytes: &[u8]) -> u8 {
    let sum = bytes.iter().map(|b| *b as u32).sum::<u32>();
    match algorithm {
        Algorithm::Twos7 => ((128 - sum % 128) & 0x7F) as u8,
        Algorithm::Sum7 => (sum & 0x7F) as u8,
        Algorithm::Xor7 => bytes.iter().fold(0u8, |a, b| a ^ b) & 0x7F,
    }
}

/// The checksums no frame could compute, one line each (nothing refuses a
/// driver). `base` is the number of bytes written before the first token (the
/// header). A `from` at or past the token's smallest position is a problem
/// always; a length past it only when every earlier token has a fixed size,
/// else the build decides.
pub fn checksum_problems(tokens: &[FrameToken], base: usize) -> Vec<String> {
    let mut problems = Vec::new();
    let (mut min, mut fixed) = (base, true);
    for tok in tokens {
        if let Some(c) = Checksum::of_token(tok) {
            let what = c.describe();
            if c.from >= min {
                problems.push(format!("{what}: offset {} is not before the checksum, written at offset {min}", c.from));
            } else if let Some(l) = c.length.filter(|l| fixed && c.from.saturating_add(*l) > min) {
                problems.push(format!("{what}: {l} bytes from offset {} run past the {min} bytes written", c.from));
            }
            min += 1;
            continue;
        }
        match tok {
            FrameToken::Part(FramePart::Identity { part, index: None }) => {
                fixed = false;
                if *part == IdentityPart::Family { min += 1; }
            }
            FrameToken::Part(FramePart::Arg { encoding: Some(Encoding::Ascii | Encoding::Bytes), .. }) => fixed = false,
            FrameToken::Part(FramePart::Arg { encoding: Some(Encoding::S32), .. }) => min += 5,
            _ => min += 1,
        }
    }
    problems
}

/// `tokens` with the identity elements (`{"identity": "family"}`, `member`,
/// with their `index`) written out as the model's bytes.
pub fn expand_identity(identity: &osd::Identity, tokens: &[FrameToken]) -> Vec<FrameToken> {
    let mut out = Vec::with_capacity(tokens.len());
    for t in tokens {
        let FrameToken::Part(FramePart::Identity { part, index }) = t else { out.push(t.clone()); continue };
        let bytes = match part {
            IdentityPart::Family => crate::device::kept_list(&identity.family),
            IdentityPart::Member => crate::device::kept_list(&identity.member),
        };
        match index {
            Some(i) => out.extend(bytes.get(*i as usize).map(|b| FrameToken::Byte(*b))),
            None => out.extend(bytes.iter().map(|b| FrameToken::Byte(*b))),
        }
    }
    out
}

/// Append the bytes of one frame element to `out`, a checksum computed over
/// what `out` already holds: for a message put together piece by piece.
pub fn append_token(tok: &FrameToken, bindings: &Bindings, out: &mut Vec<u8>) -> Result<()> {
    match Checksum::of_token(tok) {
        Some(c) => {
            let b = c.compute(out)?;
            out.push(b);
            Ok(())
        }
        None => emit_token(tok, bindings, out),
    }
}

/// Write the bytes of one frame element other than a checksum.
fn emit_token(tok: &FrameToken, b: &Bindings, out: &mut Vec<u8>) -> Result<()> {
    match tok {
        FrameToken::Byte(v) => out.push(*v),
        FrameToken::Placeholder(s) => bail!("'{s}' is an OSD 0.4 placeholder: convert the driver to 0.5 (osd convert)"),
        FrameToken::Part(FramePart::DeviceId) => emit_arg("dev", None, None, b, out)?,
        FrameToken::Part(FramePart::Arg { name, encoding, add }) => emit_arg(name, *encoding, *add, b, out)?,
        FrameToken::Part(FramePart::Identity { part, .. }) => {
            bail!("the {part:?} bytes of the model are not expanded in this frame")
        }
        FrameToken::Part(FramePart::Capture(n)) => bail!("a capture ({n}) is for a message recognised, not one sent"),
        FrameToken::Part(FramePart::Any) => bail!("`any` is for a message recognised, not one sent"),
        FrameToken::Part(FramePart::Checksum { .. }) => unreachable!("checksums are written by the callers"),
    }
    Ok(())
}

fn emit_arg(name: &str, encoding: Option<Encoding>, add: Option<u8>, b: &Bindings, out: &mut Vec<u8>) -> Result<()> {
    let val = b.get(name).ok_or_else(|| anyhow!("unbound placeholder '{name}'"))?;
    // `add`: the value plus N on seven bits, a channel in a status nibble (Korg `3g`).
    if let Some(n) = add {
        return match val {
            Value::U7(v) => { out.push(v.wrapping_add(n) & 0x7F); Ok(()) }
            _ => bail!("expected u7 for '{name}'"),
        };
    }
    match encoding {
        None => match val {
            Value::U7(v) => out.push(*v & 0x7F),
            Value::U14(v) => out.push((*v as u8) & 0x7F), // truncate LSB
            Value::S32(_) | Value::Bytes(_) => bail!("expected u7 for '{name}'"),
        },
        Some(Encoding::U14Msb) => match val {
            Value::U14(v) => out.push(((*v >> 7) & 0x7F) as u8),
            Value::U7(v)  => out.push((*v >> 7) & 0x7F), // almost always 0 but handle
            Value::S32(_) | Value::Bytes(_) => bail!("expected u14_msb for '{name}'"),
        },
        Some(Encoding::U14Lsb) => match val {
            Value::U14(v) => out.push((*v & 0x7F) as u8),
            Value::U7(v)  => out.push(*v & 0x7F),
            Value::S32(_) | Value::Bytes(_) => bail!("expected u14_lsb for '{name}'"),
        },
        // Two's complement, bits 28-31 then 21-27, 14-20, 7-13, 0-6.
        Some(Encoding::S32) => match val {
            Value::S32(v) => {
                let u = *v as u32;
                out.extend([28, 21, 14, 7, 0].map(|sh| ((u >> sh) & 0x7F) as u8));
            }
            Value::U7(v) => out.extend([0, 0, 0, 0, *v & 0x7F]),
            Value::U14(v) => out.extend([0, 0, 0, ((*v >> 7) & 0x7F) as u8, (*v & 0x7F) as u8]),
            Value::Bytes(_) => bail!("expected s32 for '{name}', got bytes"),
        },
        Some(Encoding::Ascii) => match val {
            Value::Bytes(bs) => {
                for byte in bs {
                    if *byte > 0 && *byte < 0x80 { out.push(*byte); }
                }
            }
            _ => bail!("expected ascii bytes for '{name}'"),
        },
        Some(Encoding::Bytes) => match val {
            Value::Bytes(bs) => out.extend_from_slice(bs),
            _ => bail!("expected bytes for '{name}'"),
        },
    }
    Ok(())
}

/// Convert an incoming OSC argument into a `Value` clamped per its spec.
pub fn coerce_arg(spec: &osd::Arg, osc_arg: &rosc::OscType) -> Result<Value> {
    use osd::ArgType;
    use rosc::OscType::*;
    let to_i64 = || -> Option<i64> {
        match osc_arg {
            Int(i) => Some(*i as i64),
            Long(i) => Some(*i),
            Float(f) => Some(*f as i64),
            Double(f) => Some(*f as i64),
            Bool(b) => Some(if *b { 1 } else { 0 }),
            _ => None,
        }
    };
    match spec.ty {
        ArgType::U7 => {
            let v = to_i64().ok_or_else(|| anyhow!("arg '{}' expects int/float", spec.name))?;
            let v = v.clamp(0, 127) as u8;
            Ok(Value::U7(v))
        }
        ArgType::U14 => {
            let v = to_i64().ok_or_else(|| anyhow!("arg '{}' expects int/float", spec.name))?;
            let v = v.clamp(0, 16383) as u16;
            Ok(Value::U14(v))
        }
        ArgType::S32 => {
            let v = to_i64().ok_or_else(|| anyhow!("arg '{}' expects int/float", spec.name))?;
            let [lo, hi] = spec.range.unwrap_or([i32::MIN as i64, i32::MAX as i64]);
            Ok(Value::S32(v.clamp(lo, hi) as i32))
        }
        ArgType::Bool => {
            let v = to_i64().unwrap_or(0).clamp(0, 1) as u8;
            Ok(Value::U7(v))
        }
        ArgType::Enum => {
            let values = spec.values.as_ref()
                .ok_or_else(|| anyhow!("enum arg '{}' missing values map", spec.name))?;
            let key = match osc_arg {
                String(s) => s.clone(),
                Int(i) => i.to_string(),
                _ => bail!("enum arg '{}' expects string or int", spec.name),
            };
            if let Some(v) = values.get(&key) {
                Ok(Value::U7((v & 0x7F) as u8))
            } else if let Ok(n) = key.parse::<i64>() {
                Ok(Value::U7((n.clamp(0, 127)) as u8))
            } else {
                bail!("enum arg '{}' has no value for '{}'", spec.name, key)
            }
        }
        ArgType::String => match osc_arg {
            String(s) => Ok(Value::Bytes(s.as_bytes().to_vec())),
            _ => bail!("arg '{}' expects string", spec.name),
        },
        other => bail!("unsupported arg type '{other:?}' for '{}'", spec.name),
    }
}

/// Produce bindings for a parameter SET: the numbers of the SysEx `way` it is
/// sent by, and the value, bounded to the reference scale of the entry then
/// scaled to the way.
pub fn bindings_for_param_set(pt: &osd::Params, entry: &osd::ParamEntry, way: &osd::Way, val: &rosc::OscType) -> Result<Bindings> {
    let mut b = bindings_for_param_get(pt, way);
    let val_int = match val {
        rosc::OscType::Int(i) => *i as i64,
        rosc::OscType::Long(i) => *i,
        rosc::OscType::Float(f) => *f as i64,
        rosc::OscType::Double(f) => *f as i64,
        rosc::OscType::Bool(x) => if *x { 1 } else { 0 },
        _ => bail!("param value must be numeric"),
    };
    let [lo, hi] = entry.reference_scale();
    let sent = entry.in_way(val_int.clamp(lo, hi), way);
    match pt.value_type {
        Some(osd::ValueType::U7) => b.set_u7("val", sent as u8),
        Some(osd::ValueType::U14) => b.set_u14("val", sent as u16),
        Some(osd::ValueType::S32) => b.set_s32("val", sent as i32),
        None => bail!("the parameters declare no value_type: no SysEx value to set"),
    }
    Ok(b)
}

/// The arguments of the parameter frames that the numbers of a SysEx way fill.
const SYSEX_NUMBERS: [&str; 4] = ["pr", "p", "c", "r"];

/// The values of `pr`, `p`, `c` and `r` for a SysEx way: when it has as many
/// numbers as the frames name of those arguments, they fill the named ones in
/// that order; else they fill the four in order. The absent ones are 0.
pub(crate) fn sysex_numbers(pt: &osd::Params, way: &osd::Way) -> [u8; 4] {
    let numbers = way.ctrl_index.as_deref().unwrap_or_default();
    let named = |name: &&str| [&pt.get_frame, &pt.set_frame].into_iter().flatten().flatten()
        .any(|t| matches!(t, FrameToken::Part(FramePart::Arg { name: n, .. }) if n == name));
    let mut filled: Vec<usize> = (0..SYSEX_NUMBERS.len()).filter(|i| named(&SYSEX_NUMBERS[*i])).collect();
    if filled.len() != numbers.len() {
        filled = (0..SYSEX_NUMBERS.len()).collect();
    }
    let mut out = [0u8; 4];
    for (i, number) in filled.into_iter().zip(numbers) {
        out[i] = (*number & 0x7F) as u8;
    }
    out
}

/// The numbers of a SysEx way as the parameter frames take them; a number alone
/// also fills `id`.
pub fn bindings_for_param_get(pt: &osd::Params, way: &osd::Way) -> Bindings {
    let mut b = Bindings::new();
    for (name, number) in SYSEX_NUMBERS.iter().zip(sysex_numbers(pt, way)) {
        b.set_u7(name, number);
    }
    if let Some([id]) = way.ctrl_index.as_deref() {
        match *id {
            0..=0x7F => b.set_u7("id", *id as u8),
            0x80..=0x3FFF => b.set_u14("id", *id as u16),
            // A number past 32 bits has no coding in a frame: `id` stays unbound.
            _ => if let Ok(id) = i32::try_from(*id) { b.set_s32("id", id) },
        }
    }
    b
}

#[cfg(test)]
mod tests;
