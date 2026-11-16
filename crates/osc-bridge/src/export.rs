//! Driver files written in the formats other tools read: OSCQuery (the OSC
//! address space, `crate::oscquery`) and MIDNAM (MIDI names, `crate::midnam`).
//! The same text is given by the CLI (`osc-bridge export`), the MCP tool
//! `export_device` and the hosted server, which shares the catalogue tools.
//! Each document carries the credit and licence of the driver's sources.

use anyhow::{bail, Result};
use serde_json::{json, Value};

use osd::{Device, Source};

/// The formats a driver is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    OscQuery,
    Midnam,
}

/// The names `Format` is read from, as the CLI and the MCP tool take them.
pub const FORMATS: [&str; 2] = ["oscquery", "midnam"];

impl std::str::FromStr for Format {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "oscquery" => Ok(Format::OscQuery),
            "midnam" => Ok(Format::Midnam),
            other => bail!("unknown format '{other}' (one of: {})", FORMATS.join(", ")),
        }
    }
}

impl Format {
    /// Media type of the written document.
    pub fn media_type(self) -> &'static str {
        match self {
            Format::OscQuery => "application/json",
            Format::Midnam => "application/xml",
        }
    }

    /// Usual file extension.
    pub fn extension(self) -> &'static str {
        match self {
            Format::OscQuery => "json",
            Format::Midnam => "midnam",
        }
    }
}

/// One loaded driver written in `format` (OSCQuery as compact JSON: a
/// namespace runs to thousands of nodes). Fails for MIDNAM when the device
/// speaks OSC only.
pub fn export(dev: &Device, format: Format) -> Result<String> {
    match format {
        Format::OscQuery => Ok(serde_json::to_string(&crate::oscquery::namespace(std::slice::from_ref(dev)))?),
        Format::Midnam => crate::midnam::midnam(dev),
    }
}

/// One line of credit: who (attribution, else author, else the source
/// tier), the firmware, the licence and where the data was read.
pub fn credit(s: &Source) -> String {
    let who = s.attribution.clone().or_else(|| s.author.clone())
        .unwrap_or_else(|| format!("{} source", if s.kind.is_empty() { "unnamed" } else { &s.kind }));
    let mut out = who;
    if let Some(fw) = s.firmware.as_deref().filter(|f| *f != "unknown") {
        out.push_str(&format!(", firmware {fw}"));
    }
    out.push_str(&format!(", licence {}", s.license.as_deref().unwrap_or("not stated")));
    if let Some(url) = &s.url {
        out.push_str(&format!(", {url}"));
    }
    out
}

/// Credit lines for everything a driver holds: its own sources, then those
/// of the file whose MIDI it shares (`midi_of`).
pub fn credits(dev: &Device) -> Vec<String> {
    let mut out: Vec<String> = own_sources(dev).iter().map(|s| format!("Source: {}", credit(s))).collect();
    let shared = dev.midi_of.as_deref().unwrap_or("another driver");
    out.extend(shared_sources(dev).iter().map(|s| format!("MIDI shared from {shared}: {}", credit(s))));
    if out.is_empty() {
        out.push("Source: not stated".into());
    }
    out
}

/// The same credit as data: the sources, the shared file, its sources.
pub fn sources_json(dev: &Device) -> Value {
    let list = |v: &[Source]| -> Vec<Value> {
        v.iter().map(|s| {
            let mut o = serde_json::Map::new();
            o.insert("type".into(), json!(s.kind));
            for (k, v) in [("url", &s.url), ("attribution", &s.attribution), ("author", &s.author), ("license", &s.license), ("firmware", &s.firmware)] {
                if let Some(v) = v { o.insert(k.into(), json!(v)); }
            }
            Value::Object(o)
        }).collect()
    };
    let mut out = json!({ "sources": list(own_sources(dev)) });
    if let Some(id) = &dev.midi_of {
        out["midi_of"] = json!(id);
        out["midi_of_sources"] = json!(list(&shared_sources(dev)));
    }
    out
}

/// The sources a driver states itself.
fn own_sources(dev: &Device) -> &[Source] {
    dev.sources.as_deref().unwrap_or_default()
}

/// The sources of the file whose MIDI a driver shares: `osd` keeps them, once
/// resolved, in the driver's `_midi_of_sources` remark.
pub(crate) fn shared_sources(dev: &Device) -> Vec<Source> {
    let Some(list) = dev.remarks.get("_midi_of_sources") else { return Vec::new() };
    let text = json!({
        "$schema": "https://w3id.org/osd/0.5/device.schema.json",
        "device": { "name": "shared", "osc_prefix": "/shared" },
        "_sources": list,
    });
    match osd::read(&text.to_string(), osd::Kind::Device) {
        Ok(osd::Descriptor::Device(d)) => d.sources.unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// "Vendor Name (revision)", the revision left out when empty.
pub fn device_title(dev: &Device) -> String {
    let d = &dev.device;
    let vendor = d.vendor.as_deref().unwrap_or_default();
    let mut out = if vendor.is_empty() { d.name.clone() } else { format!("{vendor} {}", d.name) };
    if let Some(rev) = d.revision.as_deref().filter(|r| !r.is_empty()) {
        out.push_str(&format!(" ({rev})"));
    }
    out
}

/// A route address read as words: `/filter/env_amount` → "Filter Env Amount".
pub fn label(osc: &str) -> String {
    osc.split(['/', '_', ' ']).filter(|w| !w.is_empty()).map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
    }).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
