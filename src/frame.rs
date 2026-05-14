//! SysEx frame assembly and matching.

use crate::device::{ArgSpec, Device, FrameToken, ParamEntry, ParamTable};
use anyhow::{anyhow, bail, Result};
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
    /// Raw byte sequence inserted at a placeholder position
    Bytes(Vec<u8>),
}

impl Bindings {
    pub fn new() -> Self { Self::default() }
    pub fn set_u7(&mut self, k: &str, v: u8) { self.0.insert(k.into(), Value::U7(v & 0x7F)); }
    pub fn set_u14(&mut self, k: &str, v: u16) { self.0.insert(k.into(), Value::U14(v & 0x3FFF)); }
    pub fn set_bytes(&mut self, k: &str, v: Vec<u8>) { self.0.insert(k.into(), Value::Bytes(v)); }
    pub fn get(&self, k: &str) -> Option<&Value> { self.0.get(k) }
}

/// Assemble a full SysEx frame from `header + body + footer` where body is
/// produced by substituting bindings into `tokens`.
pub fn build_frame(device: &Device, tokens: &[FrameToken], bindings: &Bindings) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(&device.sysex.header);
    for tok in tokens {
        match tok {
            FrameToken::Literal(b) => out.push(*b),
            FrameToken::Placeholder(s) => emit_placeholder(s, bindings, &mut out)?,
        }
    }
    out.extend_from_slice(&device.sysex.footer);
    Ok(out)
}

fn emit_placeholder(ph: &str, b: &Bindings, out: &mut Vec<u8>) -> Result<()> {
    // Strip the surrounding `{}` — placeholders come in as `"{name}"` literal strings.
    let inner = ph.trim_matches(|c| c == '{' || c == '}');
    // Parse `name[:kind]`
    let mut parts = inner.splitn(2, ':');
    let name = parts.next().unwrap_or("").trim();
    let kind = parts.next().map(|s| s.trim()).unwrap_or("u7");

    let val = b.get(name).ok_or_else(|| anyhow!("unbound placeholder '{name}'"))?;

    match kind {
        "u7" => match val {
            Value::U7(v) => out.push(*v & 0x7F),
            Value::U14(v) => out.push((*v as u8) & 0x7F), // truncate LSB
            Value::Bytes(_) => bail!("expected u7 for '{name}', got bytes"),
        },
        "u14_msb" => match val {
            Value::U14(v) => out.push(((*v >> 7) & 0x7F) as u8),
            Value::U7(v)  => out.push((*v >> 7) & 0x7F), // almost always 0 but handle
            Value::Bytes(_) => bail!("expected u14_msb for '{name}', got bytes"),
        },
        "u14_lsb" => match val {
            Value::U14(v) => out.push((*v & 0x7F) as u8),
            Value::U7(v)  => out.push(*v & 0x7F),
            Value::Bytes(_) => bail!("expected u14_lsb for '{name}', got bytes"),
        },
        "ascii" => match val {
            Value::Bytes(bs) => {
                for byte in bs {
                    if *byte > 0 && *byte < 0x80 { out.push(*byte); }
                }
            }
            _ => bail!("expected ascii bytes for '{name}'"),
        },
        "bytes" => match val {
            Value::Bytes(bs) => out.extend_from_slice(bs),
            _ => bail!("expected bytes for '{name}'"),
        },
        other => bail!("unknown placeholder kind '{other}' for '{name}'"),
    }
    Ok(())
}

/// Convert an incoming JSON-value OSC argument into a `Value` clamped per its spec.
pub fn coerce_arg(spec: &ArgSpec, osc_arg: &rosc::OscType) -> Result<Value> {
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
    match spec.ty.as_str() {
        "u7" => {
            let v = to_i64().ok_or_else(|| anyhow!("arg '{}' expects int/float", spec.name))?;
            let v = v.clamp(0, 127) as u8;
            Ok(Value::U7(v))
        }
        "u14" => {
            let v = to_i64().ok_or_else(|| anyhow!("arg '{}' expects int/float", spec.name))?;
            let v = v.clamp(0, 16383) as u16;
            Ok(Value::U14(v))
        }
        "bool" => {
            let v = to_i64().unwrap_or(0).clamp(0, 1) as u8;
            Ok(Value::U7(v))
        }
        "enum" => {
            let values = spec.values.as_ref()
                .ok_or_else(|| anyhow!("enum arg '{}' missing values map", spec.name))?;
            let key = match osc_arg {
                String(s) => s.clone(),
                Int(i) => i.to_string(),
                _ => bail!("enum arg '{}' expects string or int", spec.name),
            };
            if let Some(v) = values.get(&key).and_then(|v| v.as_i64()) {
                Ok(Value::U7((v & 0x7F) as u8))
            } else if let Ok(n) = key.parse::<i64>() {
                Ok(Value::U7((n.clamp(0, 127)) as u8))
            } else {
                bail!("enum arg '{}' has no value for '{}'", spec.name, key)
            }
        }
        "string" => match osc_arg {
            String(s) => Ok(Value::Bytes(s.as_bytes().to_vec())),
            _ => bail!("arg '{}' expects string", spec.name),
        },
        other => bail!("unsupported arg type '{}' for '{}'", other, spec.name),
    }
}

/// Produce bindings for a parameter SET from a ParamEntry + value.
pub fn bindings_for_param_set(pt: &ParamTable, entry: &ParamEntry, val: &rosc::OscType) -> Result<Bindings> {
    let mut b = Bindings::new();
    b.set_u7("pr", entry.pr);
    b.set_u7("p",  entry.p);
    b.set_u7("c",  entry.c);
    b.set_u7("r",  entry.r);
    let val_int = match val {
        rosc::OscType::Int(i) => *i as i64,
        rosc::OscType::Long(i) => *i,
        rosc::OscType::Float(f) => *f as i64,
        rosc::OscType::Double(f) => *f as i64,
        rosc::OscType::Bool(x) => if *x { 1 } else { 0 },
        _ => bail!("param value must be numeric"),
    };
    let clamped = val_int.clamp(entry.range[0], entry.range[1]);
    match pt.value_type.as_str() {
        "u7" => b.set_u7("val", clamped as u8),
        "u14" => b.set_u14("val", clamped as u16),
        other => bail!("unsupported value_type '{}'", other),
    }
    Ok(b)
}

pub fn bindings_for_param_get(entry: &ParamEntry) -> Bindings {
    let mut b = Bindings::new();
    b.set_u7("pr", entry.pr);
    b.set_u7("p",  entry.p);
    b.set_u7("c",  entry.c);
    b.set_u7("r",  entry.r);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Device, DeviceMeta, FrameToken, Sysex};

    fn test_device() -> Device {
        Device {
            device: DeviceMeta {
                name: "test".into(), vendor: "t".into(), revision: "1".into(),
                osc_prefix: "/t".into(),
                manufacturer_id: vec![0x00, 0x20, 0x6B],
                device_id: vec![0x7F, 0x42],
                rate_limit_hz: None,
                kind: None,
                transport: None,
            },
            sysex: Sysex {
                header: vec![0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42],
                footer: vec![0xF7],
            },
            commands: vec![], params: None, cc_params: None,
            midi_out: None,
            midi_in: Default::default(), replies: vec![],
            subscriptions: vec![],
            custom_commands: vec![], docs: None,
        }
    }

    #[test]
    fn frame_with_literals_only() {
        let d = test_device();
        let tokens = vec![FrameToken::Literal(0x02), FrameToken::Literal(0x40)];
        let f = build_frame(&d, &tokens, &Bindings::new()).unwrap();
        assert_eq!(f, vec![0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x02, 0x40, 0xF7]);
    }

    #[test]
    fn frame_with_u7_placeholder() {
        let d = test_device();
        let tokens = vec![FrameToken::Literal(0x21), FrameToken::Placeholder("{val}".into())];
        let mut b = Bindings::new(); b.set_u7("val", 100);
        let f = build_frame(&d, &tokens, &b).unwrap();
        assert_eq!(&f[6..=7], &[0x21, 100]);
    }

    #[test]
    fn frame_with_u14_split() {
        let d = test_device();
        let tokens = vec![
            FrameToken::Placeholder("{val:u14_msb}".into()),
            FrameToken::Placeholder("{val:u14_lsb}".into()),
        ];
        let mut b = Bindings::new(); b.set_u14("val", 12345);  // 0x3039
        let f = build_frame(&d, &tokens, &b).unwrap();
        // 12345 = 0b01100000 0111001 -> msb = 0x60, lsb = 0x39
        assert_eq!(&f[6..=7], &[0x60, 0x39]);
    }

    #[test]
    fn ascii_placeholder_filters_nonprintable() {
        let d = test_device();
        let tokens = vec![FrameToken::Placeholder("{text:ascii}".into())];
        let mut b = Bindings::new();
        b.set_bytes("text", b"Hi\x00\x80!".to_vec());
        let f = build_frame(&d, &tokens, &b).unwrap();
        assert_eq!(&f[6..=8], b"Hi!");
    }
}
