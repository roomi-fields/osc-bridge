//! osc-bridge — declarative OSC ↔ MIDI/SysEx bridge.
//!
//! Devices are described by JSON documents under `devices/<vendor>/<name>.json`.
//! The engine loads a device spec, exposes its OSC surface, and translates
//! incoming OSC to outgoing MIDI (and vice-versa for replies / live MIDI in).
//!
//! See `docs/DEVICE_JSON_SCHEMA.md` for the schema reference.

pub mod device;
pub mod frame;
pub mod runtime;
pub mod midi_codec;
pub mod scripting;
pub mod orchestrator;
pub mod routing;
pub mod mcp;
pub mod ws;

pub use device::Device;
pub use runtime::{Runtime, RuntimeOptions};
