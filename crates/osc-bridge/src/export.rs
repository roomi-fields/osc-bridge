//! Driver files written in the formats other tools read: OSCQuery (the OSC
//! address space) and MIDNAM (MIDI names), both written by `osd-formats`.
//! The same text is given by the CLI (`osc-bridge export`), the MCP tool
//! `export_device` and the hosted server, which shares the catalogue tools.
//! Each document carries the credit and licence of the driver's sources.

use anyhow::{bail, Result};

use osd::Device;
pub use osd_formats::{Loss, Translated};

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
/// namespace runs to thousands of nodes), with what the format could not
/// carry. Fails for MIDNAM when the device speaks OSC only.
pub fn export(dev: &Device, format: Format) -> Result<Translated<String>> {
    if format == Format::Midnam && dev.device.transport.as_ref().is_some_and(|t| t.kind == osd::TransportKind::Osc) {
        bail!("{} speaks OSC, not MIDI: it has no MIDI names", device_title(dev));
    }
    let mut device = dev.clone();
    let shared = shared_sources(dev);
    if !shared.is_empty() {
        device.sources.get_or_insert_with(Vec::new).extend(shared);
    }
    let resolved = osd::Resolved { device, files: Vec::new(), capabilities: Vec::new(), inherited: Vec::new() };
    let target = match format {
        Format::OscQuery => osd_formats::Format::OscQuery,
        Format::Midnam => osd_formats::Format::Midnam,
    };
    let mut out = osd_formats::export(&resolved, target)?;
    if format == Format::OscQuery {
        let mut namespace = serde_json::from_str::<serde_json::Value>(&out.value)?;
        crate::bridge_routes::add_to(&mut namespace, &[dev]);
        out.value = serde_json::to_string(&namespace)?;
    }
    Ok(out)
}

/// The sources of the file whose MIDI a driver shares: `osd` keeps them, once
/// resolved, in the driver's `_midi_of_sources` remark. They are credited
/// beside the driver's own sources.
fn shared_sources(dev: &Device) -> Vec<osd::Source> {
    let Some(list) = dev.remarks.get("_midi_of_sources") else { return Vec::new() };
    let text = serde_json::json!({
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

#[cfg(test)]
mod tests;
