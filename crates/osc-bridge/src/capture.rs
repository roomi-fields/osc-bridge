//! A capture: what a detection observed on a machine, raw, in a JSON file of a versioned format.
//!
//! It holds the port names as the system gave them (outputs, then inputs), the text of the
//! system's card listing (`/proc/asound/cards` under Linux), the `usbid` text of each card, the
//! clients of the system (`/proc/asound/seq/clients`, raw), the requests sent (bytes, output port) and every message received (input port, bytes, ms since
//! the first request). It holds no pairing, no USB ids read and no identity: the replay
//! (`detect::replay`) derives them again from these raw values, through the derivation the scan
//! uses. Nothing else of the machine is read: no host name, user name or serial number.
//!
//! This module alone reads and writes the format.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The version of the capture format this build reads and writes.
pub const FORMAT_VERSION: u32 = 2;

/// The most cards numbered on a machine whose `usbid` is read.
const CARDS_READ: u32 = 32;

/// The `usbid` text of one card, as the system gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbId {
    pub card: u32,
    pub text: String,
}

/// The system's listing of its sound cards, raw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Asound {
    /// The text of `/proc/asound/cards`.
    pub cards: String,
    /// The `usbid` of each card that has one, by card number.
    pub usbids: Vec<UsbId>,
    /// The text of `/proc/asound/seq/clients`: the clients of the system, kernel or user.
    pub clients: String,
}

impl Asound {
    /// Reads `cards`, `seq/clients` and each `card<N>/usbid` under `root` (`/proc/asound`); no
    /// other file. A missing file gives an empty listing.
    pub fn read(root: &Path) -> Asound {
        let cards = std::fs::read_to_string(root.join("cards")).unwrap_or_default();
        let clients = std::fs::read_to_string(root.join("seq/clients")).unwrap_or_default();
        let usbids = (0..CARDS_READ)
            .filter_map(|n| {
                std::fs::read_to_string(root.join(format!("card{n}/usbid"))).ok().map(|text| UsbId { card: n, text })
            })
            .collect();
        Asound { cards, usbids, clients }
    }

    /// The listing of this machine: Linux reads /proc/asound; no listing on other platforms yet.
    pub fn machine() -> Asound {
        if cfg!(target_os = "linux") { Asound::read(Path::new("/proc/asound")) } else { Asound::default() }
    }
}

/// A request sent on an output port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub port: String,
    pub bytes: Vec<u8>,
}

/// A message received on an input port, `ms` after the first request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Received {
    pub port: String,
    pub bytes: Vec<u8>,
    pub ms: u64,
}

/// What a detection observed, raw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capture {
    format_version: u32,
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
    pub cards: String,
    pub usbids: Vec<UsbId>,
    /// The text of `/proc/asound/seq/clients`, raw; empty where the system gives none.
    pub clients: String,
    /// The USB ids (vendor, product) the system itself read for a port, by port name (CoreMIDI,
    /// WinMM); absent from a capture that holds none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub system_usb: BTreeMap<String, (u16, u16)>,
    pub requests: Vec<Request>,
    pub received: Vec<Received>,
}

impl Capture {
    /// A capture of the ports and the system listing, with no request and no message yet.
    pub fn new(outputs: Vec<String>, inputs: Vec<String>, asound: Asound) -> Capture {
        Capture {
            format_version: FORMAT_VERSION,
            outputs,
            inputs,
            cards: asound.cards,
            usbids: asound.usbids,
            clients: asound.clients,
            system_usb: BTreeMap::new(),
            requests: Vec::new(),
            received: Vec::new(),
        }
    }

    /// Reads a capture; a format version other than `FORMAT_VERSION` is refused, named.
    pub fn parse(text: &str) -> Result<Capture> {
        let raw: serde_json::Value = serde_json::from_str(text).map_err(|e| anyhow!("not a capture: {e}"))?;
        match raw.get("format_version").and_then(serde_json::Value::as_u64) {
            Some(v) if v == u64::from(FORMAT_VERSION) => {}
            Some(1) => return Err(anyhow!("capture format version 1 is no longer read: this build reads version {FORMAT_VERSION}; capture again")),
            Some(v) => return Err(anyhow!("capture format version {v} is unknown: this build reads version {FORMAT_VERSION}")),
            None => return Err(anyhow!("not a capture: no format_version field")),
        }
        serde_json::from_value(raw).map_err(|e| anyhow!("not a capture: {e}"))
    }

    /// Reads the capture file at `path`.
    pub fn read(path: &Path) -> Result<Capture> {
        let text = std::fs::read_to_string(path).map_err(|e| anyhow!("{}: {e}", path.display()))?;
        Capture::parse(&text).map_err(|e| anyhow!("{}: {e}", path.display()))
    }

    /// The capture as JSON text.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Writes the capture file at `path`.
    pub fn write(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.to_json() + "\n").map_err(|e| anyhow!("{}: {e}", path.display()))
    }
}
