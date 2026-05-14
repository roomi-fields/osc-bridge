//! Device JSON schema — Rust types that mirror `docs/DEVICE_JSON_SCHEMA.md`.
//!
//! Tolerant by design: unknown fields are ignored so the schema can evolve
//! without breaking older device files.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Device {
    pub device: DeviceMeta,
    #[serde(default)]
    pub sysex: Sysex,
    #[serde(default)]
    pub commands: Vec<Command>,
    #[serde(default)]
    pub params: Option<ParamTable>,
    #[serde(default)]
    pub cc_params: Option<CcParamTable>,
    #[serde(default)]
    pub midi_out: Option<MidiOut>,
    #[serde(default)]
    pub midi_in: MidiInMap,
    #[serde(default)]
    pub replies: Vec<ReplyPattern>,
    /// One-shot OSC messages emitted to the device's OSC target at startup —
    /// typically used to subscribe to remote state changes (AbletonOSC
    /// `/live/song/start_listen/tempo`, SuperCollider notify, etc.). Ignored
    /// for hardware (MIDI) devices.
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    /// Markdown companion loaded from `<stem>.md` next to the device JSON.
    /// Populated by `Device::load`, never serialized. Served to clients via
    /// the `/bridge/docs` endpoint so LLM-driven integrators (Kanopi, agents)
    /// can read the device's integration guide at runtime.
    #[serde(default, skip)]
    pub docs: Option<String>,
    /// Scripted OSC endpoints with no declarative frame. The sole purpose of
    /// an entry is to run its Lua `script` against the incoming OSC message;
    /// scripts can register dynamic routes (`ob.register_cc_route`), set
    /// `ctx.payload` to emit raw SysEx, and read the full string arg via
    /// `ctx.args_str[1]`. This is the hook that makes reconfigurable
    /// controllers (Electra One preset upload) workable.
    #[serde(default)]
    pub custom_commands: Vec<CustomCommand>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CustomCommand {
    pub osc: String,
    pub script: String,
    /// "default" (10 ms / 1 MiB) or "preset_ingest" (500 ms / 4 MiB). Defaults
    /// to "default" — authors must opt into the larger budget explicitly.
    #[serde(default)]
    pub profile: Option<String>,
    /// If true (default), the bytes set by `ctx.payload` are wrapped with the
    /// device's sysex.header / sysex.footer before enqueue. Set to false to
    /// emit raw bytes already including F0…F7.
    #[serde(default = "default_true")]
    pub wrap_sysex: bool,
}

fn default_true() -> bool { true }

/// A device that uses plain MIDI CC (no SysEx) for parameter addressing.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CcParamTable {
    /// Default MIDI channel (0-15) for outgoing CC messages.
    #[serde(default)]
    pub channel: u8,
    pub entries: Vec<CcParamEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CcParamEntry {
    pub osc: String,
    /// Optional Lua expression applied to the incoming OSC value before clamp.
    /// Receives `value` in scope and must `return` a number.
    #[serde(default)]
    pub transform: Option<String>,
    /// Optional Lua expression applied to the value before emitting OSC on
    /// the reverse direction (device → host, not yet wired for CC).
    #[serde(default)]
    pub transform_reverse: Option<String>,
    /// MSB CC number (optional — entry may be NRPN-only).
    #[serde(default)]
    pub cc: Option<u8>,
    /// Optional LSB CC number for 14-bit CC pairs.
    #[serde(default)]
    pub cc_lsb: Option<u8>,
    /// NRPN parameter number MSB (CC 99).
    #[serde(default)]
    pub nrpn_msb: Option<u8>,
    /// NRPN parameter number LSB (CC 98).
    #[serde(default)]
    pub nrpn_lsb: Option<u8>,
    #[serde(default = "default_range")]
    pub range: [i64; 2],
    /// Purely informational: "0-based" or "centered".
    #[serde(default)]
    pub orientation: String,
    /// Optional per-entry channel override.
    #[serde(default)]
    pub channel: Option<u8>,
    /// Free-form tag so JSON authors can group related CCs (e.g. engine name).
    #[serde(default)]
    pub section: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeviceMeta {
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub revision: String,
    pub osc_prefix: String,
    #[serde(default)]
    pub manufacturer_id: Vec<u8>,
    #[serde(default)]
    pub device_id: Vec<u8>,
    #[serde(default)]
    pub rate_limit_hz: Option<u32>,
    /// "hardware" (default — MIDI/SysEx synth) or "software" (DAW or live
    /// coding environment that speaks OSC). Controls how the runtime opens
    /// I/O and which CLI flags are required.
    #[serde(default)]
    pub kind: Option<String>,
    /// Out-of-band transport configuration. When present with kind="osc",
    /// the runtime skips midir entirely and routes via UDP/OSC instead.
    /// Absent / kind="midi" preserves the legacy MIDI flow unchanged.
    #[serde(default)]
    pub transport: Option<Transport>,
}

/// Outbound transport for the driver. Default (absent) is implicit MIDI.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Transport {
    /// "midi" (default if `transport` is absent) or "osc".
    pub kind: String,
    /// OSC target host (e.g. "127.0.0.1"). Required for kind="osc".
    #[serde(default)]
    pub host: Option<String>,
    /// OSC target port the bridge sends to (e.g. 11000 for AbletonOSC).
    #[serde(default)]
    pub port: Option<u16>,
    /// UDP port the bridge listens on for replies from the OSC target
    /// (e.g. 11001 for AbletonOSC). Required when the driver declares any
    /// `subscriptions` or OSC-mode `replies`.
    #[serde(default)]
    pub reply_port: Option<u16>,
    /// Optional per-transport throttle. Off by default for OSC (UDP local
    /// has µs-latency); declare only if the receiver is observed to drop
    /// messages under bursts.
    #[serde(default)]
    pub rate_limit_hz: Option<u32>,
    /// Passthrough mode: when set, ANY OSC message under `<osc_prefix>/...` is
    /// forwarded as `<passthrough_prefix>/...` to the target, byte-for-byte,
    /// without command dispatch. Reverse for replies. Used for environments
    /// with user-defined OSC surfaces (Sonic Pi, SuperCollider, Pure Data,
    /// TouchDesigner). Empty string `""` means strip the prefix entirely.
    /// Mutually exclusive with `commands[]` style dispatch.
    #[serde(default)]
    pub passthrough_prefix: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Sysex {
    #[serde(default)]
    pub header: Vec<u8>,
    #[serde(default)]
    pub footer: Vec<u8>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Command {
    pub osc: String,
    #[serde(default)]
    pub args: Vec<ArgSpec>,
    #[serde(default)]
    pub pre: Vec<PreBind>,
    /// MIDI/SysEx framing — used when device.transport is absent or kind="midi".
    /// Tolerantly defaulted so OSC-only drivers can omit it.
    #[serde(default)]
    pub frame: Vec<FrameToken>,
    /// OSC forwarding — used when device.transport.kind="osc". The bridge
    /// builds an OSC message at this path with the listed args (literals or
    /// `{name}` placeholders resolved from `args`/`pre` bindings) and sends
    /// it to `transport.host:transport.port`.
    #[serde(default)]
    pub forward: Option<ForwardSpec>,
    /// Optional Lua script run after the frame is built but before enqueue.
    /// Receives a ScriptContext (with `payload` = assembled body bytes) and
    /// must `return ctx` (possibly with a mutated `payload` / `checksum`).
    #[serde(default)]
    pub script: Option<String>,
}

/// OSC message template emitted to an OSC target (DAW, live-coding env).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ForwardSpec {
    pub path: String,
    #[serde(default)]
    pub args: Vec<ForwardArg>,
}

/// A typed OSC argument or a `{name}` placeholder string.
///
/// Untagged: serde picks the variant by literal JSON shape — `42` → Int,
/// `1.5` → Float, `true` → Bool, `"foo"` → Str (literal or placeholder,
/// disambiguated at substitution time by checking if the value matches
/// `{name}`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ForwardArg {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

/// One-shot OSC message emitted at a lifecycle event (currently only
/// `on: "startup"`). Used to subscribe to remote state on the OSC target.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Subscription {
    #[serde(default = "default_on_startup")]
    pub on: String,
    pub forward: ForwardSpec,
}

fn default_on_startup() -> String { "startup".to_string() }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ArgSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub range: Option<[i64; 2]>,
    #[serde(default)]
    pub values: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PreBind {
    pub bind: String,
    pub expr: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FrameToken {
    Literal(u8),
    Placeholder(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParamTable {
    pub get_frame: Vec<FrameToken>,
    pub set_frame: Vec<FrameToken>,
    pub value_type: String, // "u7" | "u14"
    pub entries: Vec<ParamEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParamEntry {
    pub osc: String,
    /// Optional Lua expression — see CcParamEntry::transform.
    #[serde(default)]
    pub transform: Option<String>,
    #[serde(default)]
    pub transform_reverse: Option<String>,
    #[serde(default)]
    pub pr: u8,
    #[serde(default)]
    pub p: u8,
    #[serde(default)]
    pub c: u8,
    #[serde(default)]
    pub r: u8,
    #[serde(default = "default_range")]
    pub range: [i64; 2],
}

fn default_range() -> [i64; 2] {
    [0, 127]
}

/// Performance MIDI output: when present, activates standard OSC routes
/// (`/note/on`, `/note/off`, `/pitchbend`, `/aftertouch`, `/poly_aftertouch`,
/// `/cc/{num}`, `/program_change`) prefixed by `device.osc_prefix`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MidiOut {
    /// Default MIDI channel (0-15, i.e. "channel 1" .. "channel 16"). Every
    /// performance route accepts an optional trailing OSC arg to override
    /// the channel per call.
    #[serde(default)]
    pub default_channel: u8,
    /// Offset applied to every note number. Covers drum machines with a
    /// non-standard note mapping (e.g. kick at note 24 instead of 36).
    #[serde(default)]
    pub note_offset: i8,
}

impl Default for MidiOut {
    fn default() -> Self {
        Self { default_channel: 0, note_offset: 0 }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MidiInMap {
    #[serde(default)]
    pub note_on: Option<String>,
    #[serde(default)]
    pub note_off: Option<String>,
    #[serde(default)]
    pub cc: Option<String>,
    #[serde(default)]
    pub pitchbend: Option<String>,
    #[serde(default)]
    pub aftertouch: Option<String>,
    #[serde(default)]
    pub poly_aftertouch: Option<String>,
    #[serde(default)]
    pub program_change: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReplyPattern {
    /// MIDI/SysEx reply matching: incoming bytes from the device get matched
    /// against this token list. Used when the parent device speaks MIDI.
    #[serde(default)]
    pub match_frame: Vec<FrameToken>,
    /// OSC reply matching: incoming OSC path from the target. Used when the
    /// parent device speaks OSC. Mutually exclusive with `match_frame`; the
    /// runtime picks based on which one is non-empty.
    #[serde(default)]
    pub match_osc: Option<String>,
    /// Typed OSC argument captures for `match_osc`. Each entry binds an
    /// incoming OSC arg by position to a name reusable in `emit_osc`.
    #[serde(default)]
    pub match_args: Vec<OscArgPattern>,
    pub emit_osc: String,
    /// Optional Lua script that post-processes captured bindings before emit.
    /// Returning nil drops the OSC message.
    #[serde(default)]
    pub script: Option<String>,
}

/// One typed positional OSC arg capture for an OSC-mode reply.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OscArgPattern {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String, // "int" | "float" | "string" | "bool"
}

impl Device {
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let p = path.as_ref();
        let txt = std::fs::read_to_string(p)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", p.display()))?;
        let mut dev: Device = serde_json::from_str(&txt)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", p.display()))?;
        // Optional companion markdown at <stem>.md — integration guide for
        // clients (Kanopi, LLM agents). Silently absent is fine.
        let md_path = p.with_extension("md");
        if md_path.exists() {
            dev.docs = std::fs::read_to_string(&md_path).ok();
        }
        Ok(dev)
    }

    /// Prepend the device `osc_prefix` to a relative OSC path (starting with `/`).
    /// If the path already starts with the prefix it is returned unchanged.
    pub fn full_osc_path(&self, rel: &str) -> String {
        if rel.starts_with(&self.device.osc_prefix) {
            rel.to_string()
        } else if rel.starts_with('/') {
            format!("{}{}", self.device.osc_prefix, rel)
        } else {
            format!("{}/{}", self.device.osc_prefix, rel)
        }
    }
}
