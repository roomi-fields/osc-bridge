//! Driver files written in the formats other tools read: OSCQuery (the OSC
//! address space, `crate::oscquery`) and MIDNAM (MIDI names, `crate::midnam`).
//! The same text is given by the CLI (`osc-bridge export`), the MCP tool
//! `export_device` and the hosted server, which shares the catalogue tools.
//! Each document carries the credit and licence of the driver's sources.

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::device::{Device, Source};

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
        .unwrap_or_else(|| format!("{} source", if s.tier.is_empty() { "unnamed" } else { &s.tier }));
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
    let mut out: Vec<String> = dev.sources.iter().map(|s| format!("Source: {}", credit(s))).collect();
    let shared = dev.midi_of.as_deref().unwrap_or("another driver");
    out.extend(dev.midi_of_sources.iter().map(|s| format!("MIDI shared from {shared}: {}", credit(s))));
    if out.is_empty() {
        out.push("Source: not stated".into());
    }
    out
}

/// The same credit as data: the sources, the shared file, its sources.
pub fn sources_json(dev: &Device) -> Value {
    let list = |v: &[Source]| -> Vec<Value> {
        v.iter().map(|s| {
            let mut o = serde_json::to_value(s).unwrap_or(Value::Null);
            if let Some(m) = o.as_object_mut() { m.retain(|_, x| !x.is_null()); }
            o
        }).collect()
    };
    let mut out = json!({ "sources": list(&dev.sources) });
    if let Some(id) = &dev.midi_of {
        out["midi_of"] = json!(id);
        out["midi_of_sources"] = json!(list(&dev.midi_of_sources));
    }
    out
}

/// "Vendor Name (revision)", the revision left out when empty.
pub fn device_title(dev: &Device) -> String {
    let d = &dev.device;
    let mut out = if d.vendor.is_empty() { d.name.clone() } else { format!("{} {}", d.vendor, d.name) };
    if !d.revision.is_empty() {
        out.push_str(&format!(" ({})", d.revision));
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
mod tests {
    use super::*;

    #[test]
    fn a_source_is_credited_with_its_licence() {
        let s: Source = serde_json::from_value(json!({"type": "pencilresearch", "license": "CC-BY-SA-4.0",
            "attribution": "MIDI Guide", "url": "https://midi.guide", "firmware": "unknown"})).unwrap();
        assert_eq!(credit(&s), "MIDI Guide, licence CC-BY-SA-4.0, https://midi.guide");
        let bare: Source = serde_json::from_value(json!({"type": "vendor-doc", "firmware": "2.0"})).unwrap();
        assert_eq!(credit(&bare), "vendor-doc source, firmware 2.0, licence not stated");
    }

    #[test]
    fn every_test_driver_is_written_in_both_formats() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, out) } else if p.extension().is_some_and(|x| x == "json") { out.push(p) }
            }
        }
        let mut files = Vec::new();
        walk(std::path::Path::new("tests/fixtures/devices"), &mut files);
        let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
        let mut midnams = 0;
        for f in &files {
            let Ok(dev) = Device::load(f) else { continue };
            let ns: Value = serde_json::from_str(&export(&dev, Format::OscQuery).unwrap()).unwrap();
            assert_eq!(ns["FULL_PATH"], "/", "{}", f.display());
            if let Ok(xml) = export(&dev, Format::Midnam) {
                roxmltree::Document::parse_with_options(&xml, opts)
                    .unwrap_or_else(|e| panic!("{}: MIDNAM not well-formed: {e}", f.display()));
                midnams += 1;
            }
        }
        assert!(midnams > files.len() / 2, "{midnams} of {}", files.len());
    }

    #[test]
    fn formats_are_read_by_name() {
        assert_eq!("OSCQuery".parse::<Format>().unwrap(), Format::OscQuery);
        assert_eq!("midnam".parse::<Format>().unwrap(), Format::Midnam);
        assert!("ctrlr".parse::<Format>().is_err());
        assert_eq!(label("/filter/env_amount"), "Filter Env Amount");
    }
}
