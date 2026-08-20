//! Multi-device orchestration: one `osc-bridge orchestrate --config bridge.toml`
//! drives N devices (hardware MIDI and/or software OSC) over a single OSC
//! socket. Dispatch by `osc_prefix`.
//!
//! Config layout:
//!
//! ```toml
//! [osc]
//! bind = "127.0.0.1:7777"
//! clients = ["127.0.0.1:8888"]
//! # ws_bind = "127.0.0.1:7890"   # optional — browser clients over WebSocket
//!
//! # Hardware (MIDI/SysEx) device.
//! [[devices]]
//! spec = "devices/moog/subsequent-37.json"
//! midi_out_port = 3
//! midi_in_port  = 2
//!
//! # Same model twice — override the prefix to avoid a dispatch collision.
//! [[devices]]
//! spec = "devices/arturia/matrixbrute.json"
//! osc_prefix = "/matrixbrute-1"
//! midi_out_port = 5
//!
//! # Software (OSC-transport) device. No MIDI port. host / port /
//! # reply_port are optional overrides of the driver JSON's transport block.
//! [[devices]]
//! spec = "devices/ableton/live.third-party-osc.fw-12.2.json"
//! host = "192.168.1.40"      # optional — defaults to the JSON's value
//!
//! # Inter-device route: a knob turn on the MiniLab drives an Ableton track.
//! [[routes]]
//! from = "/minilab3/cc/74"
//! to   = "/ableton/track/0/volume"
//! map.from = [0, 127]
//! map.to   = [0, 1]
//! ```

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{bounded, Receiver};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use rosc::{OscMessage, OscPacket};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::sync::OnceLock;

use crate::device::Device;
use crate::runtime::{
    bridge_docs_replies, bridge_status_replies, build_forward_msg, dispatch_osc_to_device,
    dispatch_to_device, handle_midi_in, match_osc_path, parse_osc_tpl, run_osc_reply_loop,
    send_osc_to_target, DeviceDispatch, OscDeviceHandle, Part, SendOsc,
};

/// Compiled form of a RouteRule, pattern parsed once at startup.
struct CompiledRoute {
    from_parts: Vec<Part>,
    to_template: String,
    map: Option<MapRule>,
}

thread_local! {
    static ROUTE_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}
const ROUTE_MAX_DEPTH: u32 = 4;

fn apply_map(arg0: Option<&rosc::OscType>, map: &MapRule) -> Option<rosc::OscType> {
    use rosc::OscType;
    let v = match arg0? {
        OscType::Int(i) => *i as f64,
        OscType::Long(i) => *i as f64,
        OscType::Float(f) => *f as f64,
        OscType::Double(f) => *f,
        _ => return None,
    };
    let [a0, a1] = map.from;
    let [b0, b1] = map.to;
    if (a1 - a0).abs() < f64::EPSILON { return None; }
    let t = ((v - a0) / (a1 - a0)).clamp(0.0, 1.0);
    let out = b0 + t * (b1 - b0);
    Some(OscType::Float(out as f32))
}

fn interp_to_template(tpl: &str, caps: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = match rest[open..].find('}') {
            Some(c) => c,
            None => break,
        };
        let name = &rest[open + 1..open + close];
        if let Some(v) = caps.get(name) {
            out.push_str(v);
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// Loopback placeholder for the `from` SocketAddr of bridge-internal events
/// (route forwards, MIDI-in emissions) that don't originate from a real client.
fn internal_from() -> SocketAddr {
    "0.0.0.0:0".parse().unwrap()
}

/// Dispatch one OSC message into whichever device owns its prefix, branching
/// on the device's transport (MIDI/SysEx vs OSC).
fn dispatch_into_owner(
    msg: OscMessage,
    devices: &[LoadedDevice],
    prefix_map: &HashMap<String, usize>,
    send_osc: &SendOsc,
    from: SocketAddr,
) -> bool {
    let owner = prefix_map.iter()
        .filter(|(p, _)| msg.addr.starts_with(p.as_str()))
        .max_by_key(|(p, _)| p.len())
        .map(|(_, i)| *i);
    match owner {
        Some(idx) => {
            match &devices[idx] {
                LoadedDevice::Midi(m) => dispatch_to_device(&m.dispatch, msg, send_osc, from),
                LoadedDevice::Osc(h) => dispatch_osc_to_device(h, msg, send_osc, from),
            }
            true
        }
        None => false,
    }
}

fn apply_routes(
    msg: &OscMessage,
    routes: &[CompiledRoute],
    devices: &[LoadedDevice],
    prefix_map: &HashMap<String, usize>,
    send_osc: &SendOsc,
) {
    let depth = ROUTE_DEPTH.with(|c| c.get());
    if depth >= ROUTE_MAX_DEPTH { return; }
    for r in routes {
        let Some(caps) = match_osc_path(&r.from_parts, &msg.addr) else { continue };
        let new_addr = interp_to_template(&r.to_template, &caps);
        let new_args = if let Some(m) = &r.map {
            if let Some(mapped) = apply_map(msg.args.first(), m) {
                let mut a = msg.args.clone();
                if !a.is_empty() { a[0] = mapped; }
                a
            } else { msg.args.clone() }
        } else { msg.args.clone() };
        let new_msg = OscMessage { addr: new_addr.clone(), args: new_args };
        eprintln!("  [route] {} -> {}", msg.addr, new_addr);
        ROUTE_DEPTH.with(|c| c.set(depth + 1));
        let hit = dispatch_into_owner(new_msg, devices, prefix_map, send_osc, internal_from());
        ROUTE_DEPTH.with(|c| c.set(depth));
        if !hit {
            eprintln!("  [route] no device owns {new_addr}");
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct OrchestratorConfig {
    pub osc: OscConfig,
    pub devices: Vec<DeviceConfig>,
    #[serde(default)]
    pub routes: Vec<RouteRule>,
}

/// Declarative inter-device route. When the orchestrator emits or sees an OSC
/// message at `from` (or matching the `from` pattern with `{captures}`), it
/// builds a new message at `to` (template with the same `{captures}`),
/// optionally remapping the first numeric argument via a linear `map`, and
/// dispatches it into the owning device of the new address.
///
/// Example use case: `/minilab3/cc/74 <val>` → `/ableton/track/0/volume <val/127>`.
#[derive(Debug, Deserialize, Clone)]
pub struct RouteRule {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub map: Option<MapRule>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MapRule {
    pub from: [f64; 2],
    pub to: [f64; 2],
}

#[derive(Debug, Deserialize)]
pub struct OscConfig {
    pub bind: String,
    #[serde(default)]
    pub clients: Vec<String>,
    /// Optional WebSocket listen address (e.g. "127.0.0.1:7890") for browser
    /// clients. Binary WS frames carry raw OSC packets; each connected client
    /// acts as an extra entry in `clients` and dispatches through the same
    /// prefix-routing as UDP.
    #[serde(default)]
    pub ws_bind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeviceConfig {
    pub spec: PathBuf,
    /// MIDI OUT port index. Required for hardware (MIDI/SysEx) devices,
    /// ignored for software devices (`device.transport.kind = "osc"`).
    #[serde(default)]
    pub midi_out_port: Option<usize>,
    /// MIDI IN port index. Optional; ignored for software devices.
    #[serde(default)]
    pub midi_in_port: Option<usize>,
    /// Override `device.osc_prefix` from the JSON — required if two instances
    /// of the same device type are in the same orchestrator (same JSON ⇒ same
    /// prefix ⇒ dispatch collision).
    #[serde(default)]
    pub osc_prefix: Option<String>,
    /// OSC-transport override: replaces `device.transport.host` from the
    /// driver JSON. Lets one driver JSON target a remote instance (or a
    /// non-default port) without editing the file. Ignored for MIDI devices.
    #[serde(default)]
    pub host: Option<String>,
    /// OSC-transport override: replaces `device.transport.port`.
    #[serde(default)]
    pub port: Option<u16>,
    /// OSC-transport override: replaces `device.transport.reply_port`.
    #[serde(default)]
    pub reply_port: Option<u16>,
}

impl OrchestratorConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let p = path.as_ref();
        let txt = fs::read_to_string(p)
            .with_context(|| format!("reading {}", p.display()))?;
        let cfg: Self = toml::from_str(&txt)
            .with_context(|| format!("parsing {}", p.display()))?;
        Ok(cfg)
    }
}

/// A hardware (MIDI/SysEx) device loaded in the orchestrator.
struct MidiLoaded {
    dispatch: DeviceDispatch,
    _midi_out_name: String,
}

/// A device loaded in the orchestrator — either a hardware MIDI/SysEx synth
/// or a software OSC target (DAW, live coding environment).
enum LoadedDevice {
    Midi(MidiLoaded),
    Osc(OscDeviceHandle),
}

impl LoadedDevice {
    fn device(&self) -> &Device {
        match self {
            LoadedDevice::Midi(m) => &m.dispatch.device,
            LoadedDevice::Osc(h) => &h.device,
        }
    }
    fn osc_prefix(&self) -> &str { &self.device().device.osc_prefix }
}

/// Deferred MIDI-IN attachment — collected during the load pass, wired up
/// after the route-aware `send_osc` exists so MIDI knob turns flow through
/// the inter-device routing layer.
struct PendingMidiIn {
    device_idx: usize,
    in_port: usize,
    port_label: String,
}

/// Deferred OSC reply-listener — same reason: needs the route-aware send_osc.
struct PendingOscReply {
    device_idx: usize,
    reply_port: u16,
}

pub struct Orchestrator;

impl Orchestrator {
    pub fn run(config_path: &Path) -> Result<()> {
        let cfg = OrchestratorConfig::load(config_path)?;

        // Parse OSC clients
        let client_addrs: Vec<SocketAddr> = cfg.osc.clients.iter()
            .map(|s| s.parse::<SocketAddr>()
                .with_context(|| format!("parse osc.clients entry {s}")))
            .collect::<Result<Vec<_>>>()?;

        // raw_send: broadcast to OSC clients only (no routing) — UDP and WS.
        let ws_clients = cfg.osc.ws_bind.as_ref().map(|_| crate::ws::WsClients::new());
        let osc_sock = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
        let clients = client_addrs.clone();
        let sock_cb = osc_sock.clone();
        let ws_cb = ws_clients.clone();
        let raw_send: SendOsc = Arc::new(move |msg| {
            let ws_active = ws_cb.as_ref().is_some_and(|w| w.has_clients());
            if clients.is_empty() && !ws_active { return; }
            if let Ok(bytes) = rosc::encoder::encode(&OscPacket::Message(msg)) {
                for tgt in &clients { let _ = sock_cb.send_to(&bytes, tgt); }
                if let Some(w) = &ws_cb { w.broadcast(&bytes); }
            }
        });

        // --- Load pass: build LoadedDevice per config entry ---
        let mut devices: Vec<LoadedDevice> = Vec::new();
        let mut pending_midi_in: Vec<PendingMidiIn> = Vec::new();
        let mut pending_osc_reply: Vec<PendingOscReply> = Vec::new();

        for dc in &cfg.devices {
            let mut dev = Device::load(&dc.spec)?;
            if let Some(pfx) = &dc.osc_prefix {
                if !pfx.starts_with('/') {
                    bail!("osc_prefix override must start with '/': {pfx}");
                }
                dev.device.osc_prefix = pfx.clone();
            }
            let is_osc = dev.device.transport.as_ref()
                .map(|t| t.kind.as_str()) == Some("osc");

            if is_osc {
                // Software (OSC-transport) device. Apply optional host /
                // port / reply_port overrides from bridge.toml before build.
                if let Some(t) = dev.device.transport.as_mut() {
                    if let Some(h) = &dc.host { t.host = Some(h.clone()); }
                    if let Some(p) = dc.port { t.port = Some(p); }
                    if let Some(rp) = dc.reply_port { t.reply_port = Some(rp); }
                }
                let transport_reply_port = dev.device.transport.as_ref()
                    .and_then(|t| t.reply_port);
                let handle = OscDeviceHandle::build(dev)
                    .with_context(|| format!("loading OSC device {}", dc.spec.display()))?;
                eprintln!("  + {} → OSC {}", handle.device.device.osc_prefix, handle.target_addr);
                let idx = devices.len();
                if let Some(rp) = transport_reply_port {
                    pending_osc_reply.push(PendingOscReply { device_idx: idx, reply_port: rp });
                }
                devices.push(LoadedDevice::Osc(handle));
            } else {
                // Hardware (MIDI/SysEx) device — needs a MIDI OUT port.
                let midi_out_port = dc.midi_out_port.ok_or_else(|| anyhow!(
                    "{}: hardware device requires midi_out_port in bridge.toml",
                    dev.device.name))?;
                let output = MidiOutput::new("osc-bridge-out")?;
                let ports = output.ports();
                let port = ports.get(midi_out_port)
                    .ok_or_else(|| anyhow!("{}: no MIDI out at idx {}", dev.device.name, midi_out_port))?
                    .clone();
                let name = output.port_name(&port).unwrap_or_else(|_| "?".into());
                let out_conn = output.connect(&port, "osc-bridge-out-conn")
                    .map_err(|e| anyhow!("midi out for {}: {e}", dev.device.name))?;
                let (tx, rx) = bounded::<Vec<u8>>(1024);
                let rate = dev.device.rate_limit_hz.unwrap_or(0);
                std::thread::spawn(move || drain_midi_out(out_conn, rx, rate));

                eprintln!("  + {} → out[{midi_out_port}] {name}", dev.device.osc_prefix);
                let idx = devices.len();
                if let Some(in_idx) = dc.midi_in_port {
                    pending_midi_in.push(PendingMidiIn {
                        device_idx: idx,
                        in_port: in_idx,
                        port_label: dev.device.osc_prefix.clone(),
                    });
                }
                let dispatch = DeviceDispatch::build(dev, tx);
                devices.push(LoadedDevice::Midi(MidiLoaded { dispatch, _midi_out_name: name }));
            }
        }

        // Build prefix → device index map
        let mut prefix_map: HashMap<String, usize> = HashMap::new();
        for (i, d) in devices.iter().enumerate() {
            if prefix_map.insert(d.osc_prefix().to_string(), i).is_some() {
                bail!("duplicate osc_prefix {} — override it in bridge.toml", d.osc_prefix());
            }
        }

        // Compile inter-device routes
        let compiled_routes: Vec<CompiledRoute> = cfg.routes.iter().map(|r| CompiledRoute {
            from_parts: parse_osc_tpl(&r.from),
            to_template: r.to.clone(),
            map: r.map.clone(),
        }).collect();
        if !compiled_routes.is_empty() {
            eprintln!("Routes: {} declared", compiled_routes.len());
            for r in &cfg.routes {
                eprintln!("  {} -> {}{}", r.from, r.to,
                    r.map.as_ref().map(|m| format!(" [map {:?} -> {:?}]", m.from, m.to)).unwrap_or_default());
            }
        }

        // Share device state + routing tables across the main loop, the
        // MIDI-IN callbacks, and the OSC reply listeners.
        let devices = Arc::new(devices);
        let prefix_map = Arc::new(prefix_map);
        let routes = Arc::new(compiled_routes);

        // Route-aware send_osc: broadcasts to clients, then re-injects matching
        // messages through the routing layer. Self-referential (replies from a
        // routed dispatch cascade through routing too) via a OnceLock holder.
        let routed_holder: Arc<OnceLock<SendOsc>> = Arc::new(OnceLock::new());
        let routed_send: SendOsc = {
            let raw = raw_send.clone();
            let devices = devices.clone();
            let prefix_map = prefix_map.clone();
            let routes = routes.clone();
            let holder = routed_holder.clone();
            Arc::new(move |msg: OscMessage| {
                raw(msg.clone());
                if let Some(self_send) = holder.get() {
                    apply_routes(&msg, &routes, &devices, &prefix_map, self_send);
                }
            })
        };
        let _ = routed_holder.set(routed_send.clone());

        // WebSocket transport — browser clients dispatch through the same
        // prefix-routing (and inter-device routes) as the UDP loop below.
        if let (Some(bind), Some(wsc)) = (&cfg.osc.ws_bind, &ws_clients) {
            let dispatch_cb: crate::ws::WsDispatch = {
                let devices = devices.clone();
                let prefix_map = prefix_map.clone();
                let routes = routes.clone();
                let send = routed_send.clone();
                Arc::new(move |pkt, from| {
                    orch_dispatch(pkt, &devices, &prefix_map, &routes, &send, from);
                })
            };
            let local = crate::ws::serve(bind, wsc.clone(), dispatch_cb)?;
            eprintln!("WS IN    {local}");
        }

        // --- Deferred wiring: MIDI-IN connections (kept alive for the process
        // lifetime in `keepalive`) ---
        let mut keepalive: Vec<MidiInputConnection<()>> = Vec::new();
        for p in &pending_midi_in {
            let mut input = MidiInput::new("osc-bridge-in")?;
            input.ignore(Ignore::None);
            let in_ports = input.ports();
            let in_port = in_ports.get(p.in_port)
                .ok_or_else(|| anyhow!("{}: no MIDI in at idx {}", p.port_label, p.in_port))?
                .clone();
            // Reuse the device's own dispatch state (Arc<Device> + scripts +
            // dynamic routes) so MIDI-IN decoding matches the single-device path.
            let (dev_cb, routes_cb, scripts_cb) = match &devices[p.device_idx] {
                LoadedDevice::Midi(m) => (
                    m.dispatch.device.clone(),
                    m.dispatch.routes.clone(),
                    m.dispatch.scripts.clone(),
                ),
                LoadedDevice::Osc(_) => unreachable!("pending_midi_in only built for MIDI devices"),
            };
            let send_cb = routed_send.clone();
            let conn = input.connect(
                &in_port,
                &format!("osc-bridge-in-{}", p.port_label),
                move |_, msg, _| {
                    handle_midi_in(msg, &dev_cb, &send_cb, &routes_cb, &scripts_cb);
                },
                (),
            ).map_err(|e| anyhow!("midi in for {}: {e}", p.port_label))?;
            keepalive.push(conn);
        }

        // --- Deferred wiring: OSC reply listeners for software devices ---
        for p in &pending_osc_reply {
            let bind = format!("0.0.0.0:{}", p.reply_port);
            let reply_sock = UdpSocket::bind(&bind)
                .with_context(|| format!("bind OSC reply socket {bind}"))?;
            let dev_cb = devices[p.device_idx].device().clone();
            let dev_cb = Arc::new(dev_cb);
            let send_cb = routed_send.clone();
            eprintln!("  OSC REPLY {bind} ({})", dev_cb.device.osc_prefix);
            std::thread::spawn(move || run_osc_reply_loop(reply_sock, dev_cb, send_cb));
        }

        // --- Fire startup subscriptions for software devices ---
        let empty: HashMap<String, rosc::OscType> = HashMap::new();
        for d in devices.iter() {
            if let LoadedDevice::Osc(h) = d {
                for sub in &h.device.subscriptions {
                    if sub.on != "startup" { continue; }
                    if let Some(out_msg) = build_forward_msg(&sub.forward, &empty) {
                        send_osc_to_target(&h.target_sock, h.target_addr, &out_msg);
                        eprintln!("  OSC SUB  {} -> {}", out_msg.addr, h.target_addr);
                    }
                }
            }
        }

        // Inbound OSC socket
        let sock = UdpSocket::bind(&cfg.osc.bind)
            .with_context(|| format!("bind {}", cfg.osc.bind))?;
        eprintln!("Orchestrator ready. {} device(s) on {}", devices.len(), cfg.osc.bind);
        for c in &client_addrs { eprintln!("OSC OUT  {c}"); }

        let mut buf = [0u8; 65536];
        loop {
            let (n, from) = match sock.recv_from(&mut buf) {
                Ok(p) => p,
                Err(e) => { eprintln!("recv: {e}"); continue; }
            };
            match rosc::decoder::decode_udp(&buf[..n]) {
                Ok((_, pkt)) => orch_dispatch(pkt, &devices, &prefix_map, &routes, &routed_send, from),
                Err(e) => eprintln!("OSC decode err: {e}"),
            }
        }
    }
}

fn orch_dispatch(
    pkt: OscPacket,
    devices: &[LoadedDevice],
    prefix_map: &HashMap<String, usize>,
    routes: &[CompiledRoute],
    send_osc: &SendOsc,
    from: SocketAddr,
) {
    match pkt {
        OscPacket::Message(m) => orch_handle(m, devices, prefix_map, routes, send_osc, from),
        OscPacket::Bundle(b) => for p in b.content { orch_dispatch(p, devices, prefix_map, routes, send_osc, from); }
    }
}

fn orch_handle(
    msg: OscMessage,
    devices: &[LoadedDevice],
    prefix_map: &HashMap<String, usize>,
    routes: &[CompiledRoute],
    send_osc: &SendOsc,
    from: SocketAddr,
) {
    // Global: /bridge/status → reply with every device
    if msg.addr == "/bridge/status" {
        let devs: Vec<Device> = devices.iter().map(|d| d.device().clone()).collect();
        for reply in bridge_status_replies(&devs) {
            send_osc(reply);
        }
        return;
    }

    // Global: /bridge/docs → one reply per device that has a companion .md
    // loaded. Mirrors the single-device `run` behaviour.
    if msg.addr == "/bridge/docs" {
        let devs: Vec<Device> = devices.iter().map(|d| d.device().clone()).collect();
        for reply in bridge_docs_replies(&devs) {
            send_osc(reply);
        }
        return;
    }

    // Dispatch into the owning device (MIDI or OSC transport).
    if !dispatch_into_owner(msg.clone(), devices, prefix_map, send_osc, from) {
        // No device owns this address — it may still be a route source.
    }

    // Inter-device routing: always apply, regardless of whether a device owns
    // the source address (lets you route from any client-emitted path too).
    apply_routes(&msg, routes, devices, prefix_map, send_osc);
}

fn drain_midi_out(mut conn: MidiOutputConnection, rx: Receiver<Vec<u8>>, rate_hz: u32) {
    let interval = if rate_hz > 0 {
        Duration::from_micros(1_000_000 / rate_hz as u64)
    } else { Duration::ZERO };
    let mut last = Instant::now() - interval;
    while let Ok(frm) = rx.recv() {
        if !interval.is_zero() {
            let e = last.elapsed();
            if e < interval { std::thread::sleep(interval - e); }
        }
        if let Err(e) = conn.send(&frm) { eprintln!("midi send: {e}"); }
        last = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::OscType;

    #[test]
    fn interp_substitutes_captures() {
        let mut caps = HashMap::new();
        caps.insert("n".to_string(), "3".to_string());
        assert_eq!(
            interp_to_template("/ableton/track/{n}/volume", &caps),
            "/ableton/track/3/volume"
        );
    }

    #[test]
    fn interp_unknown_placeholder_becomes_empty() {
        let caps = HashMap::new();
        assert_eq!(interp_to_template("/x/{missing}/y", &caps), "/x//y");
    }

    #[test]
    fn interp_no_placeholder_passes_through() {
        let caps = HashMap::new();
        assert_eq!(interp_to_template("/static/path", &caps), "/static/path");
    }

    #[test]
    fn apply_map_linear_endpoints_and_midpoint() {
        let m = MapRule { from: [0.0, 127.0], to: [0.0, 1.0] };
        match apply_map(Some(&OscType::Int(127)), &m).unwrap() {
            OscType::Float(f) => assert!((f - 1.0).abs() < 0.001),
            o => panic!("expected float, got {o:?}"),
        }
        match apply_map(Some(&OscType::Int(0)), &m).unwrap() {
            OscType::Float(f) => assert!(f.abs() < 0.001),
            o => panic!("expected float, got {o:?}"),
        }
        match apply_map(Some(&OscType::Int(64)), &m).unwrap() {
            OscType::Float(f) => assert!((f - 0.504).abs() < 0.01),
            o => panic!("expected float, got {o:?}"),
        }
    }

    #[test]
    fn apply_map_clamps_out_of_range_input() {
        let m = MapRule { from: [0.0, 100.0], to: [0.0, 1.0] };
        match apply_map(Some(&OscType::Int(250)), &m).unwrap() {
            OscType::Float(f) => assert!((f - 1.0).abs() < 0.001),
            o => panic!("expected float, got {o:?}"),
        }
        match apply_map(Some(&OscType::Int(-50)), &m).unwrap() {
            OscType::Float(f) => assert!(f.abs() < 0.001),
            o => panic!("expected float, got {o:?}"),
        }
    }

    #[test]
    fn apply_map_inverted_output_range() {
        // map.to can be descending — e.g. CC → an inverted parameter.
        let m = MapRule { from: [0.0, 127.0], to: [1.0, 0.0] };
        match apply_map(Some(&OscType::Int(0)), &m).unwrap() {
            OscType::Float(f) => assert!((f - 1.0).abs() < 0.001),
            o => panic!("expected float, got {o:?}"),
        }
        match apply_map(Some(&OscType::Int(127)), &m).unwrap() {
            OscType::Float(f) => assert!(f.abs() < 0.001),
            o => panic!("expected float, got {o:?}"),
        }
    }

    #[test]
    fn apply_map_rejects_non_numeric_and_missing() {
        let m = MapRule { from: [0.0, 1.0], to: [0.0, 1.0] };
        assert!(apply_map(Some(&OscType::String("x".into())), &m).is_none());
        assert!(apply_map(None, &m).is_none());
    }

    #[test]
    fn apply_map_zero_input_range_is_rejected() {
        let m = MapRule { from: [5.0, 5.0], to: [0.0, 1.0] };
        assert!(apply_map(Some(&OscType::Int(5)), &m).is_none());
    }
}
