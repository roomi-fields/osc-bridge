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
    /// Note-triggered sounds (drum machines, samplers): each entry gives a
    /// named OSC address that fires one MIDI note on the right channel.
    #[serde(default)]
    pub triggers: Option<TriggerTable>,
    /// Catalogue id of the file whose MIDI this one shares (see
    /// [`resolve_midi_of`]); its routes are already merged in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_of: Option<String>,
    /// Where the file's data comes from, with its credit and licence.
    #[serde(default, rename = "_sources")]
    pub sources: Vec<Source>,
    /// With `midi_of`: the sources of the shared file (and of the files it
    /// shares from), where its MIDI interface comes from.
    #[serde(default, rename = "_midi_of_sources", skip_serializing_if = "Vec::is_empty")]
    pub midi_of_sources: Vec<Source>,
}

/// One `_sources` entry. Only the keys an export credits are read.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Source {
    /// Source tier (`vendor-doc`, `pencilresearch`, `hardware-verified`…).
    #[serde(default, rename = "type")]
    pub tier: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub attribution: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub firmware: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TriggerTable {
    pub entries: Vec<TriggerEntry>,
}

/// One note-triggered sound. `/<prefix><osc> [velocity] [note] [channel]`
/// sends a Note On.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TriggerEntry {
    pub osc: String,
    /// Human label of the voice (`Kick`, `TR909 Open hihat`).
    pub sound: String,
    #[serde(default)]
    pub section: Option<String>,
    /// Fixed receive channel 0-15. Absent when the device listens on its
    /// user-set basic channel, in omni, or on the active track
    /// (`channel_mode`); the sender then uses `midi_out.default_channel`.
    #[serde(default)]
    pub channel: Option<u8>,
    /// `"fixed"` (default), `"basic"`, `"omni"` or `"auto"`.
    #[serde(default = "default_channel_mode")]
    pub channel_mode: String,
    /// Lowest and highest note that trigger the sound.
    pub note_range: [u8; 2],
    /// Natural note within the range; absent for slices / active-track rows,
    /// where the caller must pass the note explicitly.
    #[serde(default)]
    pub note: Option<u8>,
    #[serde(default)]
    pub velocity_usage: Option<String>,
    #[serde(default)]
    pub comments: Option<String>,
}

fn default_channel_mode() -> String { "fixed".to_string() }

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
    /// NRPN LSB given by the caller, within this range, when it selects what
    /// the parameter acts on (e.g. a drum note): the call is then
    /// `<osc> <value> <lsb> [<channel>]`. Used with `nrpn_msb`, without `nrpn_lsb`.
    #[serde(default)]
    pub nrpn_lsb_range: Option<[u8; 2]>,
    /// Registered parameter number MSB (CC 101), e.g. 0 for pitch bend sensitivity.
    #[serde(default)]
    pub rpn_msb: Option<u8>,
    /// Registered parameter number LSB (CC 100).
    #[serde(default)]
    pub rpn_lsb: Option<u8>,
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
    /// Device-side expectation for this control — purely informative metadata,
    /// never the signal realization. Describes *what the control expects*
    /// (a continuous range, an on/off switch, a clock…), not how a runtime
    /// drives it (CV / Gate / Trig / Clock — that mapping belongs to the
    /// caller, e.g. BPScript). Recommended vocabulary: "continuous" (default),
    /// "switch", "momentary", "trigger", "clock", "discrete". Left open: any
    /// string is accepted, the runtime owns the interpretation.
    #[serde(default = "default_expects")]
    pub expects: String,
    /// What the values mean, as the source states it (`0: Off; 1-127: On`).
    #[serde(default, rename = "_usage", skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
}

fn default_expects() -> String { "continuous".to_string() }

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
    pub value_type: String, // "u7" | "u14" | "s32"
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
    /// Device-side expectation for this control — see `CcParamEntry::expects`.
    /// Purely informative; the signal realization (CV/Gate/Trig/Clock) belongs
    /// to the caller, not osc-bridge.
    #[serde(default = "default_expects")]
    pub expects: String,
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

/// The `device` keys that say which firmware a file is, not how it speaks
/// MIDI: a file with `midi_of` keeps these and takes the rest from its target.
pub const IDENTITY: [&str; 5] = ["name", "vendor", "revision", "osc_prefix", "kind"];

/// Route lists a `midi_of` file may add to its target's, by OSC address.
pub const OWN_LISTS: [&str; 2] = ["commands", "custom_commands"];
/// Route tables (`entries`) a `midi_of` file may add to its target's.
pub const OWN_TABLES: [&str; 3] = ["params", "cc_params", "triggers"];

/// A driver file as its firmware runs it. A file naming another with
/// `midi_of` (its catalogue id, `m-vave/fm-1.vendor.fw-15`: a firmware that
/// shares that MIDI interface) keeps its own identity — the `_` keys
/// (sources, limitations) and the `device` keys of [`IDENTITY`] — and takes
/// everything else from the target, read from the same catalogue. The target
/// is a driver of the same device (same maker folder, same name); it may
/// itself name another with `midi_of` (JJOS Light shares OS3's MIDI, which
/// adds three CCs to the stock MPC1000's), resolved first, up to
/// [`MIDI_OF_DEPTH`] links and never in a circle. The file may add routes of its own
/// ([`OWN_LISTS`], the `entries` of [`OWN_TABLES`]; a norns script's
/// parameters on the norns): appended to the target's, its own winning on
/// the same OSC address. Transport, SysEx framing, MIDI in and out and the
/// tables' framing stay the target's.
pub fn resolve_midi_of(path: &Path, doc: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    resolve_chain(path, doc, &mut Vec::new())
}

/// The most `midi_of` links followed from one file.
pub const MIDI_OF_DEPTH: usize = 8;

fn resolve_chain(path: &Path, doc: serde_json::Value, chain: &mut Vec<std::path::PathBuf>) -> anyhow::Result<serde_json::Value> {
    use serde_json::Value;
    let Some(target) = doc.get("midi_of") else { return Ok(doc) };
    let fail = |why: String| anyhow::anyhow!("{}: midi_of {target}: {why}", path.display());
    let id = target.as_str().ok_or_else(|| fail("not a catalogue id".into()))?;
    let parts: Vec<&str> = id.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|c| c.is_empty() || c.starts_with('.')) {
        return Err(fail("not a catalogue id (<maker>/<file name without .json>)".into()));
    }
    let folder = path.parent().ok_or_else(|| fail("no catalogue folder".into()))?;
    if folder.file_name().and_then(|f| f.to_str()) != Some(parts[0]) {
        return Err(fail("names another maker's driver".into()));
    }
    let allowed = |k: &str| k.starts_with('_') || ["device", "midi_of"].contains(&k)
        || OWN_LISTS.contains(&k) || OWN_TABLES.contains(&k);
    if let Some(own) = doc.as_object().and_then(|o| o.keys().find(|k| !allowed(k))) {
        return Err(fail(format!("{own} is the target's (a midi_of file adds routes only)")));
    }
    // The file's own routes: each address once.
    let osc = |e: &Value| e.get("osc").and_then(Value::as_str).map(str::to_string);
    let entries_of = |v: &Value, key: &str| -> Vec<Value> {
        let list = if OWN_LISTS.contains(&key) { v.get(key) } else { v.get(key).and_then(|t| t.get("entries")) };
        list.and_then(Value::as_array).cloned().unwrap_or_default()
    };
    for key in OWN_LISTS.iter().chain(OWN_TABLES.iter()) {
        let mut seen = std::collections::HashSet::new();
        for e in entries_of(&doc, key) {
            let a = osc(&e).ok_or_else(|| fail(format!("a {key} entry has no osc address")))?;
            if !seen.insert(a.clone()) {
                return Err(fail(format!("{a} is twice in its own {key}")));
            }
        }
    }
    let tpath = folder.join(format!("{}.json", parts[1]));
    let txt = std::fs::read_to_string(&tpath).map_err(|e| fail(format!("reading {}: {e}", tpath.display())))?;
    let out: Value = serde_json::from_str(&txt).map_err(|e| fail(format!("parsing {}: {e}", tpath.display())))?;
    chain.push(path.to_path_buf());
    if chain.iter().any(|p| p == &tpath) {
        return Err(fail("the links come back to this file (a circle)".into()));
    }
    if chain.len() > MIDI_OF_DEPTH {
        return Err(fail(format!("more than {MIDI_OF_DEPTH} links")));
    }
    let mut out = resolve_chain(&tpath, out, chain)?;
    let name = |d: &Value| d.pointer("/device/name").and_then(Value::as_str).map(str::to_string);
    if name(&doc).is_some() && name(&doc) != name(&out) {
        return Err(fail("the target is another device".into()));
    }
    // Appended to the target's routes, its own replacing one at the same address.
    let merge = |base: Vec<Value>, own: Vec<Value>| -> Vec<Value> {
        let mine: std::collections::HashSet<_> = own.iter().filter_map(osc).collect();
        base.into_iter().filter(|e| osc(e).map_or(true, |a| !mine.contains(&a))).chain(own).collect()
    };
    let obj = out.as_object_mut().ok_or_else(|| fail("the target is not a driver".into()))?;
    // Credit for the MIDI taken from the target: its sources and those it took from.
    let shared: Vec<Value> = ["_sources", "_midi_of_sources"].iter()
        .filter_map(|k| obj.get(*k).and_then(Value::as_array)).flatten().cloned().collect();
    obj.retain(|k, _| !k.starts_with('_'));
    let mut device = obj.remove("device").unwrap_or_else(|| Value::Object(Default::default()));
    for (k, v) in doc.as_object().into_iter().flatten() {
        if k == "device" {
            for key in IDENTITY {
                match v.get(key) {
                    Some(x) => { device[key] = x.clone(); }
                    None => { device.as_object_mut().map(|d| d.remove(key)); }
                }
            }
        } else if OWN_LISTS.contains(&k.as_str()) {
            let base = obj.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
            obj.insert(k.clone(), Value::Array(merge(base, entries_of(&doc, k))));
        } else if OWN_TABLES.contains(&k.as_str()) {
            match obj.get_mut(k).filter(|t| t.is_object()) {
                Some(table) => {
                    if let Some(extra) = v.as_object().and_then(|t| t.keys().find(|x| *x != "entries")) {
                        return Err(fail(format!("{k}.{extra} is the target's (a midi_of file adds entries only)")));
                    }
                    let base = table.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
                    table["entries"] = Value::Array(merge(base, entries_of(&doc, k)));
                }
                None => { obj.insert(k.clone(), v.clone()); }
            }
        } else {
            obj.insert(k.clone(), v.clone());
        }
    }
    if !shared.is_empty() {
        obj.insert("_midi_of_sources".into(), Value::Array(shared));
    }
    obj.insert("device".into(), device);
    Ok(out)
}

impl Device {
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let resolved = crate::catalogue::resolve(path.as_ref());
        let p = resolved.as_path();
        let txt = std::fs::read_to_string(p)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", p.display()))?;
        let doc: serde_json::Value = serde_json::from_str(&txt)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", p.display()))?;
        let mut dev: Device = serde_json::from_value(resolve_midi_of(p, doc)?)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cc_param_expects_defaults_to_continuous() {
        let e: CcParamEntry = serde_json::from_str(r#"{"osc":"/cutoff","cc":74}"#).unwrap();
        assert_eq!(e.expects, "continuous");
    }

    #[test]
    fn cc_param_expects_is_carried_when_set() {
        let e: CcParamEntry =
            serde_json::from_str(r#"{"osc":"/arp/hold","cc":64,"expects":"switch"}"#).unwrap();
        assert_eq!(e.expects, "switch");
    }

    fn catalogue(files: &[(&str, &str)]) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!("osc-bridge-midi-of-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        for (rel, txt) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, txt).unwrap();
        }
        root
    }

    const STOCK: &str = r#"{"_sources":[{"type":"vendor-doc","firmware":"15"}],"_limitations":"stock",
        "device":{"name":"Box","vendor":"Acme","revision":"V15","osc_prefix":"/box","manufacturer_id":[67]},
        "commands":[{"osc":"/init","frame":[240,67,247]}],"loads_sounds_of":["dx7"]}"#;

    #[test]
    fn a_firmware_sharing_another_files_midi_keeps_its_own_identity() {
        let root = catalogue(&[("acme/box.vendor.fw-15.json", STOCK),
            ("acme/box.vendor.fw-fork-1.0.json", r#"{"_sources":[{"type":"vendor-doc","firmware":"fork-1.0"}],
                "device":{"name":"Box","vendor":"Acme","revision":"Fork 1.0","osc_prefix":"/box_fork"},
                "midi_of":"acme/box.vendor.fw-15"}"#),
            ("acme/box.vendor.fw-fork-1.0.md", "# Fork")]);
        let dev = Device::load(root.join("acme/box.vendor.fw-fork-1.0.json")).unwrap();
        assert_eq!((dev.device.revision.as_str(), dev.device.osc_prefix.as_str()), ("Fork 1.0", "/box_fork"));
        assert_eq!(dev.device.manufacturer_id, vec![67], "the MIDI identity comes with the shared interface");
        assert_eq!(dev.commands.len(), 1);
        assert_eq!(dev.docs.as_deref(), Some("# Fork"), "its own README");
        let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("acme/box.vendor.fw-fork-1.0.json")).unwrap()).unwrap();
        let doc = resolve_midi_of(&root.join("acme/box.vendor.fw-fork-1.0.json"), raw).unwrap();
        assert_eq!(doc["_sources"][0]["firmware"], "fork-1.0");
        assert!(doc.get("_limitations").is_none(), "the target's limitations are its own");
        assert_eq!(doc["_midi_of_sources"][0]["firmware"], "15", "the shared MIDI is credited to its sources");
        assert_eq!((dev.sources[0].firmware.as_deref(), dev.midi_of_sources[0].firmware.as_deref()),
                   (Some("fork-1.0"), Some("15")));
        assert_eq!(doc["loads_sounds_of"][0], "dx7");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_file_sharing_another_files_midi_adds_routes_of_its_own() {
        let base = r#"{"device":{"name":"norns","vendor":"monome","osc_prefix":"/norns","transport":{"kind":"osc","host":"127.0.0.1","port":10111}},
            "commands":[{"osc":"/remote/key","frame":[]},{"osc":"/param/{id}","frame":[]}],
            "cc_params":{"channel":3,"entries":[{"osc":"/vol","cc":7}]}}"#;
        let root = catalogue(&[("monome/norns.json", base),
            ("monome/norns.script-awake.json", r#"{"_sources":[{"type":"vendor-doc"}],
                "device":{"name":"norns","vendor":"monome","revision":"awake","osc_prefix":"/norns_awake"},"midi_of":"monome/norns",
                "commands":[{"osc":"/param/{id}","frame":[1]},{"osc":"/param/step_div","frame":[2]}],
                "cc_params":{"entries":[{"osc":"/cutoff","cc":74}]},
                "triggers":{"entries":[{"osc":"/kick","sound":"Kick","note_range":[36,36],"note":36}]}}"#)]);
        let dev = Device::load(root.join("monome/norns.script-awake.json")).unwrap();
        let cmds: Vec<_> = dev.commands.iter().map(|c| c.osc.as_str()).collect();
        assert_eq!(cmds, ["/remote/key", "/param/{id}", "/param/step_div"], "the target's, then its own");
        assert_eq!(dev.commands[1].frame.len(), 1, "its own wins on the same address");
        let cc = dev.cc_params.as_ref().unwrap();
        assert_eq!((cc.channel, cc.entries.len()), (3, 2), "the table's framing stays the target's");
        assert_eq!(dev.triggers.as_ref().unwrap().entries.len(), 1, "a table the target lacks is the file's");
        assert_eq!(dev.device.osc_prefix, "/norns_awake");
        assert!(dev.device.transport.is_some(), "the transport is the target's");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_broken_midi_of_is_refused() {
        let link = |target: &str, extra: &str| format!(
            r#"{{"device":{{"name":"Box","vendor":"Acme","osc_prefix":"/b"}},"midi_of":"{target}"{extra}}}"#);
        let other = STOCK.replace(r#""name":"Box""#, r#""name":"Crate""#);
        let root = catalogue(&[("acme/box.vendor.fw-15.json", STOCK), ("acme/crate.json", &other),
            ("other/box.json", STOCK),
            ("acme/a.json", &link("acme/box.vendor.fw-15", "")),
            ("acme/chain.json", &link("acme/a", r#","commands":[{"osc":"/more","frame":[1]}]"#)),
            ("acme/circle1.json", &link("acme/circle2", "")),
            ("acme/circle2.json", &link("acme/circle1", "")),
            ("acme/self.json", &link("acme/self", "")),
            ("acme/missing.json", &link("acme/none", "")),
            ("acme/maker.json", &link("other/box", "")),
            ("acme/device.json", &link("acme/crate", "")),
            ("acme/body.json", &link("acme/box.vendor.fw-15", r#","sysex":{}"#)),
            ("acme/twice.json", &link("acme/box.vendor.fw-15", r#","commands":[{"osc":"/x"},{"osc":"/x"}]"#)),
            ("acme/framing.json", &link("acme/script", r#","cc_params":{"channel":2,"entries":[]}"#)),
            ("acme/script.json", &STOCK.replace(r#""commands""#, r#""cc_params":{"channel":0,"entries":[]},"commands""#)),
            ("acme/up.json", &link("../box", ""))]);
        assert!(Device::load(root.join("acme/a.json")).is_ok());
        let chained = Device::load(root.join("acme/chain.json")).unwrap();
        assert_eq!(chained.commands.iter().map(|c| c.osc.as_str()).collect::<Vec<_>>(), ["/init", "/more"],
            "a link to a link: the first target's routes, then its own");
        for (file, why) in [("circle1", "a circle"), ("self", "a circle"), ("missing", "reading"), ("maker", "another maker"),
                            ("device", "another device"), ("body", "is the target's"), ("up", "not a catalogue id"),
                            ("twice", "twice in its own commands"), ("framing", "cc_params.channel is the target's")] {
            let err = Device::load(root.join(format!("acme/{file}.json"))).unwrap_err().to_string();
            assert!(err.contains(why), "{file}: {err}");
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_chain_of_links_stops_past_its_depth() {
        let link = |n: usize| format!(r#"{{"device":{{"name":"Box","vendor":"Acme","osc_prefix":"/b{n}"}},"midi_of":"acme/l{}"}}"#, n - 1);
        let mut files = vec![("acme/l0.json".to_string(), STOCK.to_string())];
        files.extend((1..=MIDI_OF_DEPTH + 1).map(|n| (format!("acme/l{n}.json"), link(n))));
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let root = catalogue(&refs);
        assert!(Device::load(root.join(format!("acme/l{MIDI_OF_DEPTH}.json"))).is_ok());
        let err = Device::load(root.join(format!("acme/l{}.json", MIDI_OF_DEPTH + 1))).unwrap_err().to_string();
        assert!(err.contains("more than"), "{err}");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn sysex_param_expects_defaults_to_continuous() {
        let e: ParamEntry = serde_json::from_str(r#"{"osc":"/p","pr":1,"p":2}"#).unwrap();
        assert_eq!(e.expects, "continuous");
    }
}
