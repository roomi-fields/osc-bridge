//! Driver files written in the formats other tools read (OSCQuery, MIDNAM, the
//! Deluge definition, and every other format of `osd-formats`), all written by
//! `osd-formats`.
//! The same text is given by the CLI (`osc-bridge export`), the MCP tool
//! `export_device` and the hosted server, which shares the catalogue tools.
//! Each document carries the credit and licence of the driver's sources.

use anyhow::{bail, Result};

use osd::Device;
pub use osd_formats::{Loss, Translated};

/// A format a driver is written in: one of `osd-formats`, which gives its
/// name, media type and extension. osc-bridge holds no case per format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format(pub osd_formats::Format);

/// The names `Format` is read from, as the CLI and the MCP tool take them:
/// those of `osd-formats`.
pub fn format_names() -> Vec<&'static str> {
    osd_formats::Format::ALL.iter().map(|f| f.name()).collect()
}

impl std::str::FromStr for Format {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match osd_formats::Format::from_name(&s.to_ascii_lowercase()) {
            Some(f) => Ok(Format(f)),
            None => bail!("unknown format '{s}' (one of: {})", format_names().join(", ")),
        }
    }
}

impl Format {
    /// Its name.
    pub fn name(self) -> &'static str {
        self.0.name()
    }

    /// Media type of the written document.
    pub fn media_type(self) -> &'static str {
        self.0.media_type()
    }

    /// Usual file extension.
    pub fn extension(self) -> &'static str {
        self.0.extension()
    }
}

/// One loaded driver written in `format` (OSCQuery as compact JSON: a
/// namespace runs to thousands of nodes), with what the format could not
/// carry. Fails for MIDNAM when the device speaks OSC only.
pub fn export(dev: &Device, format: Format) -> Result<Translated<String>> {
    if format.0 == osd_formats::Format::Midnam && dev.device.transport.as_ref().is_some_and(|t| t.kind == osd::TransportKind::Osc) {
        bail!("{} speaks OSC, not MIDI: it has no MIDI names", device_title(dev));
    }
    // The credit of the MIDI taken from other drivers stands beside the driver's own.
    let mut device = dev.clone();
    if let Some(shared) = &dev.midi_of_sources {
        device.sources.get_or_insert_with(Vec::new).extend(shared.iter().cloned());
    }
    let resolved = osd::Resolved { device, files: Vec::new(), capabilities: Vec::new(), inherited: Vec::new() };
    let mut out = osd_formats::export(&resolved, format.0)?;
    if format.0 == osd_formats::Format::OscQuery {
        let mut namespace = serde_json::from_str::<serde_json::Value>(&out.value)?;
        crate::bridge_routes::add_to(&mut namespace, &[dev]);
        out.value = serde_json::to_string(&namespace)?;
    }
    Ok(out)
}

/// "Vendor Name", the vendor left out when the driver gives none.
pub fn device_title(dev: &Device) -> String {
    let d = &dev.device;
    let vendor = d.vendor.as_deref().unwrap_or_default();
    if vendor.is_empty() { d.name.clone() } else { format!("{vendor} {}", d.name) }
}

#[cfg(test)]
mod tests;
