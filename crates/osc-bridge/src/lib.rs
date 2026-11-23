//! osc-bridge — declarative OSC ↔ MIDI/SysEx bridge.
//!
//! Devices are described by JSON driver files (`<vendor>/<name>.json`): the
//! catalogue of them lives on a server, which hands them out one
//! at a time, through the link module.
//! The engine loads a device spec, exposes its OSC surface, and translates
//! incoming OSC to outgoing MIDI (and vice-versa for replies / live MIDI in).
//!
//! The format of a device spec is the Open Synth Descriptors (OSD), read
//! through the `osd` crate.

// The MCP tool schemas are one large `json!` literal.
#![recursion_limit = "512"]

pub mod device;
pub mod frame;
pub mod runtime;
pub mod midi_codec;
pub mod scripting;
pub mod orchestrator;
pub mod routing;
pub mod mcp;
pub mod ws;
pub mod detect;
pub mod recognition;
pub mod capture;
pub mod librarian;
pub mod midi_port;
pub mod synthub;
pub mod export;
pub mod install;
pub mod oscquery;
pub mod bridge_routes;
pub mod programs;

pub use osd::Device;
pub use runtime::{Runtime, RuntimeOptions};
