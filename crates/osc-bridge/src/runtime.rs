//! Runtime wiring: OSC server ↔ MIDI ports ↔ rate limiter ↔ device spec.

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use rosc::{OscMessage, OscPacket, OscType};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::device::{address_text, attached_docs, full_osc_path};
use crate::frame::{self, Bindings, Value};
use osd::{Address, CcEntry, Command, CustomCommand, Device, ParamEntry, Scalar};
use crate::midi_codec;
use crate::routing::{self, DynamicRoutes};
use crate::scripting::{EmitBlock, EmitOsc, EmitOscArg, ScriptContext, ScriptEngine};

pub struct RuntimeOptions {
    pub device: Device,
    pub midi_out_port_idx: usize,
    pub midi_in_port_idx: Option<usize>,
    pub osc_bind: String,
    /// Zero, one, or more outbound OSC destinations. Every MIDI-in event,
    /// SysEx reply, and `/bridge/status` response is broadcast to all of them.
    pub osc_clients: Vec<SocketAddr>,
    /// Optional WebSocket listen address (e.g. `127.0.0.1:7890`). When set,
    /// browser clients can connect; each one behaves like a dynamic
    /// `--osc-client` (binary WS frames = raw OSC packets, both directions).
    pub ws_bind: Option<String>,
    /// Extra browser origins allowed on the WebSocket (`*` for any). Local
    /// pages and non-browser clients are always accepted.
    pub ws_allow_origins: Vec<String>,
    /// Optional HTTP listen address (e.g. `127.0.0.1:7891`) answering
    /// OSCQuery requests with the device's OSC namespace (`crate::oscquery`).
    pub oscquery_bind: Option<String>,
}

pub struct Runtime;

/// A bridge whose sockets are bound: the client-facing OSC socket (and the
/// reply, WebSocket and OSCQuery listeners) are open, so a client may send as
/// soon as `Runtime::bind` returns; datagrams queue until `serve` reads them.
pub struct BoundRuntime {
    osc_addr: SocketAddr,
    serve: Box<dyn FnOnce() -> Result<()> + Send>,
}

impl BoundRuntime {
    /// Resolved address of the client-facing OSC socket (the real port when
    /// `osc_bind` asked for port 0).
    pub fn osc_addr(&self) -> SocketAddr {
        self.osc_addr
    }

    /// The receive loop: dispatches each incoming packet. Returns only on error.
    pub fn serve(self) -> Result<()> {
        (self.serve)()
    }
}

impl Runtime {
    /// Bind then serve: the blocking entry point of the application.
    pub fn run(opts: RuntimeOptions) -> Result<()> {
        Self::bind(opts)?.serve()
    }

    /// Open every socket the bridge needs and return it ready to `serve`.
    pub fn bind(opts: RuntimeOptions) -> Result<BoundRuntime> {
        // Software (OSC-out) drivers bypass midir entirely. Branch early so
        // the rest of run() stays unchanged for hardware (MIDI) drivers.
        if opts.device.device.transport.as_ref().map(|t| t.kind)
            == Some(osd::TransportKind::Osc)
        {
            return bind_osc(opts);
        }

        let dev = Arc::new(opts.device);

        // MIDI OUT
        let output = MidiOutput::new("osc-bridge-out")?;
        let out_ports = output.ports();
        let out_port = out_ports.get(opts.midi_out_port_idx)
            .ok_or_else(|| anyhow!("no MIDI output at index {}", opts.midi_out_port_idx))?
            .clone();
        let out_name = output.port_name(&out_port).unwrap_or_else(|_| "?".into());
        let out_conn = output.connect(&out_port, "osc-bridge-out-conn")
            .map_err(|e| anyhow!("midi out: {e}"))?;

        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = bounded(1024);
        let rate = dev.device.rate_limit_hz.unwrap_or(0).max(0) as u32;
        std::thread::spawn(move || drain_midi_out(out_conn, rx, rate));

        // Outbound OSC — broadcast to every configured client (UDP and WS).
        let ws_clients = opts.ws_bind.as_ref().map(|_| crate::ws::WsClients::new());
        let osc_sock = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
        let clients = opts.osc_clients.clone();
        let sock_cb = osc_sock.clone();
        let ws_cb = ws_clients.clone();
        let send_osc: Arc<dyn Fn(OscMessage) + Send + Sync> = Arc::new(move |msg| {
            let ws_active = ws_cb.as_ref().is_some_and(|w| w.has_clients());
            if clients.is_empty() && !ws_active { return; }
            if let Ok(bytes) = rosc::encoder::encode(&OscPacket::Message(msg)) {
                for tgt in &clients {
                    let _ = sock_cb.send_to(&bytes, tgt);
                }
                if let Some(w) = &ws_cb { w.broadcast(&bytes); }
            }
        });

        // Dynamic routes — shared between the Lua engine (scripts register
        // routes after preset ingest) and the dispatch paths (MIDI-in / OSC-in
        // consult them ahead of the static spec).
        let routes = DynamicRoutes::handle();

        // Lua engine — created early so the MIDI-in callback can invoke
        // reply.script (e.g. Electra's /page/switched updating current_page).
        let script_engine: ScriptCell = Arc::new(OnceLock::new());
        if device_uses_scripting(&dev) {
            match ScriptEngine::new_with_routes_for_device(routes.clone(), &dev) {
                Ok(e) => {
                    let _ = script_engine.set(Mutex::new(e));
                    eprintln!("Lua scripting: enabled (device uses transform/script fields)");
                }
                Err(e) => eprintln!("Lua scripting: init failed — scripts will be ignored: {e}"),
            }
        }

        // MIDI IN
        let _in_conn = if let Some(in_idx) = opts.midi_in_port_idx {
            let mut input = MidiInput::new("osc-bridge-in")?;
            input.ignore(Ignore::None);
            let in_ports = input.ports();
            let in_port = in_ports.get(in_idx)
                .ok_or_else(|| anyhow!("no MIDI input at index {in_idx}"))?
                .clone();
            let in_name = input.port_name(&in_port).unwrap_or_else(|_| "?".into());
            eprintln!("MIDI IN  [{in_idx}] {in_name}");
            let send_osc_cb = send_osc.clone();
            let dev_cb = dev.clone();
            let routes_cb = routes.clone();
            let scripts_cb = script_engine.clone();
            let conn: MidiInputConnection<()> = input.connect(
                &in_port,
                "osc-bridge-in-conn",
                move |_stamp, msg, _| handle_midi_in(msg, &dev_cb, &send_osc_cb, &routes_cb, &scripts_cb),
                (),
            ).map_err(|e| anyhow!("midi in: {e}"))?;
            Some(conn)
        } else { None };

        eprintln!("Device:  {} ({})", dev.device.name, dev.device.revision.as_deref().unwrap_or_default());
        eprintln!("MIDI OUT [{}] {out_name}", opts.midi_out_port_idx);
        eprintln!("OSC IN   {}", opts.osc_bind);
        for c in &opts.osc_clients { eprintln!("OSC OUT  {c}"); }

        // Build dispatcher maps (Arc so WS connection threads share them).
        let state = Arc::new(DeviceDispatch {
            device: dev.clone(),
            cmds: build_command_map(&dev),
            params: build_param_map(&dev),
            cc_params: build_cc_param_map(&dev),
            routes: routes.clone(),
            scripts: script_engine.clone(),
            tx: tx.clone(),
        });

        // WebSocket transport — every connected browser client goes through
        // the exact same dispatch as the UDP loop below.
        let mut ws_local = None;
        if let (Some(bind), Some(wsc)) = (&opts.ws_bind, &ws_clients) {
            let dispatch_cb: crate::ws::WsDispatch = {
                let state = state.clone();
                let send_osc = send_osc.clone();
                Arc::new(move |pkt, from| dispatch(pkt, &state, &send_osc, from))
            };
            let local = crate::ws::serve(bind, wsc.clone(), dispatch_cb, opts.ws_allow_origins.clone())?;
            eprintln!("WS IN    {local}");
            ws_local = Some(local);
        }

        let sock = UdpSocket::bind(&opts.osc_bind)
            .with_context(|| format!("bind {}", opts.osc_bind))?;
        let osc_addr = sock.local_addr()?;
        serve_oscquery(&opts.oscquery_bind, std::slice::from_ref(dev.as_ref()), &sock, ws_local)?;
        let osc_clients = opts.osc_clients;

        let serve = move || -> Result<()> {
            // Keep the MIDI input connection alive for the life of the loop.
            let _in_conn = _in_conn;
            eprintln!("Ready. Ctrl-C to stop.");
            let mut buf = [0u8; 65536];
            loop {
                let (n, from) = match sock.recv_from(&mut buf) {
                    Ok(p) => p,
                    Err(e) => { eprintln!("recv: {e}"); continue; }
                };
                match rosc::decoder::decode_udp(&buf[..n]) {
                    Ok((_, pkt)) => {
                        answer_query(&sock, &pkt, from, std::slice::from_ref(dev.as_ref()), &osc_clients);
                        dispatch(pkt, &state, &send_osc, from)
                    }
                    Err(e) => eprintln!("OSC decode err from {from}: {e}"),
                }
            }
        };
        Ok(BoundRuntime { osc_addr, serve: Box::new(serve) })
    }
}

pub type ScriptCell = Arc<OnceLock<Mutex<ScriptEngine>>>;

/// Start the OSCQuery listener when `bind` is set, describing `devices` as
/// reached on `sock` (UDP) and the WebSocket at `ws`.
pub fn serve_oscquery(bind: &Option<String>, devices: &[Device], sock: &UdpSocket, ws: Option<SocketAddr>) -> Result<()> {
    if let Some(bind) = bind {
        let local = crate::oscquery::serve(bind, devices, sock.local_addr()?, ws)?;
        eprintln!("OSCQuery http://{local}/");
    }
    Ok(())
}

/// Build the `/bridge/status/device` replies for every device currently
/// served by this runtime. In single-device mode returns one entry;
/// the orchestrator (phase 4) will feed it the full device list.
pub fn bridge_status_replies(devices: &[Device]) -> Vec<OscMessage> {
    devices.iter().map(|d| {
        let slug = d.device.osc_prefix.trim_start_matches('/').to_string();
        OscMessage {
            addr: "/bridge/status/device".into(),
            args: vec![
                OscType::String(slug),
                OscType::String("ok".into()),
            ],
        }
    }).collect()
}

/// Build `/bridge/docs/device <slug> <markdown>` replies for every device
/// that has a companion `.md` (each device with its note). Devices without
/// docs are silently skipped — the client can fall back to introspection
/// endpoints.
pub fn bridge_docs_replies(devices: &[(Device, Option<String>)]) -> Vec<OscMessage> {
    devices.iter().filter_map(|(d, docs)| docs_reply(d, docs.as_deref())).collect()
}

/// The `/bridge/docs/device` reply of one device and its note.
fn docs_reply(d: &Device, docs: Option<&str>) -> Option<OscMessage> {
    let docs = docs?;
    let slug = d.device.osc_prefix.trim_start_matches('/').to_string();
    Some(OscMessage {
        addr: "/bridge/docs/device".into(),
        args: vec![OscType::String(slug), OscType::String(docs.to_string())],
    })
}

/// The `/bridge/docs` replies for devices loaded with their notes.
fn loaded_docs_replies(devices: &[Device]) -> Vec<OscMessage> {
    devices.iter().filter_map(|d| docs_reply(d, attached_docs(d).as_deref())).collect()
}

/// Replies to a `/bridge/status` or `/bridge/docs` query, None for any other
/// address.
pub fn bridge_query_replies(addr: &str, devices: &[Device]) -> Option<Vec<OscMessage>> {
    match addr {
        "/bridge/status" => Some(bridge_status_replies(devices)),
        "/bridge/docs" => Some(loaded_docs_replies(devices)),
        _ => None,
    }
}

/// Send the answer to a `/bridge/*` query straight back to whoever asked —
/// a one-off caller such as the MCP `get_status` tool is not an
/// `--osc-client`. Configured clients already get it through the broadcast.
pub fn answer_query(sock: &UdpSocket, pkt: &OscPacket, from: SocketAddr, devices: &[Device], clients: &[SocketAddr]) {
    let OscPacket::Message(m) = pkt else { return };
    if clients.contains(&from) { return; }
    let Some(replies) = bridge_query_replies(&m.addr, devices) else { return };
    for r in replies {
        if let Ok(bytes) = rosc::encoder::encode(&OscPacket::Message(r)) {
            let _ = sock.send_to(&bytes, from);
        }
    }
}

/// Test helper — given a Device and an OscMessage, return the MIDI bytes that
/// the performance-MIDI route would emit (or None if `midi_out` is not enabled
/// or the path isn't a performance route). Used by `tests/midi_out.rs`.
pub fn midi_out_for_msg(dev: &Device, msg: &OscMessage) -> Option<Vec<u8>> {
    let mo = dev.midi_out.as_ref()?;
    try_midi_out(msg, mo, &dev.device.osc_prefix)
}

/// Named note trigger: `/<prefix><trigger.osc> [velocity] [note] [channel]`
/// becomes a Note On. None when `msg.addr` is not a trigger of `dev`.
/// Velocity defaults to 100; the note defaults to the trigger's natural note
/// and must stay inside its range; the channel defaults to the trigger's
/// fixed channel, else `midi_out.default_channel`.
pub fn trigger_note_on(dev: &Device, msg: &OscMessage) -> Option<Result<Vec<u8>, String>> {
    let table = dev.triggers.as_ref()?;
    let t = table.entries.iter().find(|t| full_osc_path(dev, &t.osc) == msg.addr)?;
    let int_at = |i: usize| -> Option<i64> {
        match msg.args.get(i)? {
            OscType::Int(v) => Some(*v as i64),
            OscType::Long(v) => Some(*v),
            OscType::Float(v) => Some(*v as i64),
            OscType::Double(v) => Some(*v as i64),
            _ => None,
        }
    };
    let velocity = int_at(0).unwrap_or(100).clamp(1, 127) as u8;
    let [lo, hi] = t.note_range;
    let note = match int_at(1).or(t.note.map(i64::from)) {
        Some(n) if (lo..=hi).contains(&n) => n as u8,
        Some(n) => return Some(Err(format!("note {n} outside {lo}..={hi} for {}", t.sound))),
        None => return Some(Err(format!("{} plays notes {lo}..={hi}: pass the note as 2nd arg", t.sound))),
    };
    let default_ch = dev.midi_out.as_ref().and_then(|m| m.default_channel).unwrap_or(0);
    let channel = int_at(2).map(|c| c as u8).or(t.channel).unwrap_or(default_ch) & 0x0F;
    Some(Ok(vec![0x90 | channel, note & 0x7F, velocity]))
}

/// Dynamic-route CC in: given raw MIDI bytes, return the semantic OSC message
/// if a route is registered. None means "no dynamic route — fall through to
/// the declarative pipeline".
pub fn try_dynamic_cc_in(routes: &Arc<Mutex<DynamicRoutes>>, msg: &[u8]) -> Option<OscMessage> {
    if msg.len() != 3 { return None; }
    if msg[0] & 0xF0 != 0xB0 { return None; }
    let channel = msg[0] & 0x0F;
    let cc = msg[1] & 0x7F;
    let raw = msg[2] & 0x7F;
    let g = routes.lock().ok()?;
    let route = g.lookup_cc(channel, cc)?;
    let norm = routing::cc_in_to_norm(raw, route.min, route.max);
    Some(OscMessage {
        addr: route.osc_addr.clone(),
        args: vec![OscType::Float(norm)],
    })
}

/// Dynamic-route OSC in: given an OSC message on a semantic addr, assemble
/// the corresponding MIDI CC bytes. None means "no dynamic route for this
/// addr — fall through to declarative command/param matching".
pub fn try_dynamic_osc_out(routes: &Arc<Mutex<DynamicRoutes>>, msg: &OscMessage) -> Option<Vec<u8>> {
    let g = routes.lock().ok()?;
    let route = g.lookup_osc(&msg.addr)?;
    let arg = msg.args.first()?;
    let norm = match arg {
        OscType::Float(f) => *f,
        OscType::Double(f) => *f as f32,
        OscType::Int(i) => (*i as f32) / 127.0,
        OscType::Long(i) => (*i as f32) / 127.0,
        _ => return None,
    };
    let u7 = routing::norm_to_u7(norm);
    let status = 0xB0 | (route.channel & 0x0F);
    Some(vec![status, route.cc & 0x7F, u7])
}

/// Handle the standard performance-MIDI OSC routes enabled by `midi_out`.
/// Returns the assembled MIDI bytes, or None if `msg.addr` is not a
/// performance route for this device.
fn try_midi_out(msg: &OscMessage, mo: &osd::MidiOut, prefix: &str) -> Option<Vec<u8>> {
    let rel = msg.addr.strip_prefix(prefix)?;
    let args = &msg.args;

    // Fetch integer arg at index i (various numeric OscType coercions).
    let int_at = |i: usize| -> Option<i64> {
        match args.get(i)? {
            OscType::Int(v)    => Some(*v as i64),
            OscType::Long(v)   => Some(*v),
            OscType::Float(v)  => Some(*v as i64),
            OscType::Double(v) => Some(*v as i64),
            OscType::Bool(v)   => Some(if *v { 1 } else { 0 }),
            _ => None,
        }
    };
    let default_channel = mo.default_channel.unwrap_or(0);
    let note_offset = mo.note_offset.unwrap_or(0);
    let channel = |idx: usize| -> u8 {
        int_at(idx).map(|v| (v & 0x0F) as u8).unwrap_or(default_channel & 0x0F)
    };
    let apply_note = |n: i64| -> u8 {
        ((n + note_offset).clamp(0, 127)) as u8
    };
    let u7 = |v: i64| -> u8 { v.clamp(0, 127) as u8 };

    match rel {
        "/note/on" => {
            let note = int_at(0)?;
            let vel  = int_at(1)?;
            let ch   = channel(2);
            Some(vec![0x90 | ch, apply_note(note), u7(vel)])
        }
        "/note/off" => {
            let note = int_at(0)?;
            let vel  = int_at(1).unwrap_or(0);
            let ch   = channel(2);
            Some(vec![0x80 | ch, apply_note(note), u7(vel)])
        }
        "/pitchbend" => {
            // value_u14: 0..16383, centre 8192
            let v = int_at(0)?.clamp(0, 16383) as u16;
            let ch = channel(1);
            Some(vec![0xE0 | ch, (v & 0x7F) as u8, ((v >> 7) & 0x7F) as u8])
        }
        "/aftertouch" => {
            let v  = int_at(0)?;
            let ch = channel(1);
            Some(vec![0xD0 | ch, u7(v)])
        }
        "/poly_aftertouch" => {
            let note = int_at(0)?;
            let v    = int_at(1)?;
            let ch   = channel(2);
            Some(vec![0xA0 | ch, apply_note(note), u7(v)])
        }
        "/program_change" => {
            let p  = int_at(0)?;
            let ch = channel(1);
            Some(vec![0xC0 | ch, u7(p)])
        }
        path if path.starts_with("/cc/") => {
            let cc_num = path[4..].parse::<u8>().ok()?;
            let v  = int_at(0)?;
            let ch = channel(1);
            Some(vec![0xB0 | ch, cc_num & 0x7F, u7(v)])
        }
        _ => None,
    }
}


/// Whether the driver holds Lua: what `osd` says the reader needs.
fn device_uses_scripting(dev: &Device) -> bool {
    !dev.derived_capabilities().is_empty()
}

/// Electra One's own annotation of a command (a free `_` key of the command):
/// the parameter a preset's Lua reads through `parameterMap.get`.
const ELECTRA_KEY: &str = "_electra";

/// Hand the engine the value a command just sent (its last numeric OSC
/// argument), for `parameterMap.get`.
fn record_sent_value(cell: &ScriptCell, cmd: &Command, args: &[OscType]) {
    if !cmd.remarks.contains_key(ELECTRA_KEY) { return; }
    let Some(m) = cell.get() else { return };
    let v = args.iter().rev().find_map(|a| match a {
        OscType::Int(i) => Some(*i as i64),
        OscType::Long(i) => Some(*i),
        OscType::Float(f) => Some(*f as i64),
        OscType::Double(f) => Some(*f as i64),
        _ => None,
    });
    if let (Some(v), Ok(e)) = (v, m.lock()) { e.record_value(&address_text(&cmd.osc), v); }
}

/// Hand the engine the value a device reply carries (its last numeric OSC
/// argument) when the reply's address is that of a command annotated
/// `_electra`, for `parameterMap.get`.
fn record_received_value(cell: &ScriptCell, dev: &Device, m: &OscMessage) {
    let Some(rel) = m.addr.strip_prefix(&dev.device.osc_prefix) else { return };
    let Some(cmd) = dev.commands.iter().flatten().find(|c| c.remarks.contains_key(ELECTRA_KEY) && address_text(&c.osc) == rel) else { return };
    record_sent_value(cell, cmd, &m.args);
}

/// Run a command's `script` field (if any) against the assembled frame.
/// Returns the (possibly mutated) frame, or None to drop the message.
fn maybe_run_cmd_script(cell: &ScriptCell, cmd: &Command, frame: Vec<u8>, device: &str) -> Option<Vec<u8>> {
    let Some(code) = &cmd.script else { return Some(frame); };
    let osc = address_text(&cmd.osc);
    let Some(m) = cell.get() else {
        eprintln!("  command {osc} script ignored (engine unavailable)");
        return Some(frame);
    };
    let ctx = ScriptContext {
        payload: frame,
        direction: "osc_to_midi".into(),
        device: device.into(),
        command: osc.clone(),
        ..Default::default()
    };
    match m.lock() {
        Ok(g) => match g.run_script(code, ctx) {
            Ok(Some(out)) => Some(out.payload),
            Ok(None) => {
                eprintln!("  command {osc} script dropped message");
                None
            }
            Err(e) => {
                eprintln!("  command {osc} script err: {e} — passing through raw frame");
                None
            }
        },
        Err(_) => None,
    }
}

/// Dispatch a `custom_commands` entry. Build a ScriptContext populated with
/// numeric and string OSC args, run the script under the requested profile,
/// and — if the script filled `ctx.payload` — enqueue those bytes as SysEx
/// (wrapped with header/footer when `wrap_sysex` is true).
fn run_custom_command(
    cc: &CustomCommand,
    msg: &OscMessage,
    dev: &Arc<Device>,
    cell: &ScriptCell,
    tx: &Sender<Vec<u8>>,
    send_osc: &SendOsc,
    from: SocketAddr,
) {
    let Some(m) = cell.get() else {
        eprintln!("  custom_command {} skipped (engine unavailable)", cc.osc);
        return;
    };

    let mut args_i: Vec<i64> = Vec::new();
    let mut args_s: Vec<String> = Vec::new();
    for a in &msg.args {
        match a {
            OscType::Int(i) => args_i.push(*i as i64),
            OscType::Long(i) => args_i.push(*i),
            OscType::Float(f) => args_i.push(*f as i64),
            OscType::Double(f) => args_i.push(*f as i64),
            OscType::Bool(b) => args_i.push(if *b { 1 } else { 0 }),
            OscType::String(s) => args_s.push(s.clone()),
            _ => {}
        }
    }

    let profile = match cc.profile {
        Some(osd::ScriptProfile::PresetIngest) => crate::scripting::Profile::PresetIngest,
        _ => crate::scripting::Profile::Default,
    };

    let ctx = ScriptContext {
        args: args_i,
        args_str: args_s,
        direction: "osc_to_midi".into(),
        device: dev.device.name.clone(),
        command: cc.osc.clone(),
        ..Default::default()
    };

    let (result, emits, osc_emits) = match m.lock() {
        Ok(g) => {
            let r = g.run_script_with(profile, &cc.script, ctx);
            let e = g.drain_emits();
            let o = g.drain_osc_emits();
            (r, e, o)
        }
        Err(_) => { eprintln!("  custom_command {} lock poisoned", cc.osc); return; }
    };

    // Enqueue any SysEx blocks the script requested via ob.emit_sysex, in order.
    enqueue_emits(tx, &emits, dev, &msg.addr, from);
    // Emit any OSC messages the script requested via ob.emit_osc (introspection).
    send_script_osc(send_osc, &osc_emits);

    match result {
        Ok(Some(out)) if !out.payload.is_empty() => {
            let bytes = if cc.wrap_sysex.unwrap_or(true) {
                frame::wrap_sysex(dev, &out.payload)
            } else {
                out.payload
            };
            enqueue(tx, bytes, &msg.addr, from);
        }
        Ok(_) => {
            // Script returned no payload — pure side-effects (registered
            // routes, logged, etc.). Nothing to send.
        }
        Err(e) => eprintln!("  custom_command {} script err: {e}", cc.osc),
    }
}

/// Run a reply.script with the matched bindings exposed as `ctx.bindings`.
/// Returns true if the OSC emission should proceed, false to drop it.
fn run_reply_script(
    rp: &osd::Reply,
    bindings: &Bindings,
    dev: &Arc<Device>,
    cell: &ScriptCell,
) -> bool {
    let Some(code) = &rp.script else { return true; };
    let emit = address_text(&rp.emit_osc);
    let Some(m) = cell.get() else {
        eprintln!("  reply.script {emit} skipped (engine unavailable)");
        return true;
    };
    let mut binds: std::collections::HashMap<String, i64> = Default::default();
    for (k, v) in &bindings.0 {
        let iv = match v {
            Value::U7(n) => *n as i64,
            Value::U14(n) => *n as i64,
            Value::S32(n) => *n as i64,
            Value::Bytes(_) => continue,
        };
        binds.insert(k.clone(), iv);
    }
    let ctx = ScriptContext {
        bindings: binds,
        direction: "midi_to_osc".into(),
        device: dev.device.name.clone(),
        command: emit.clone(),
        ..Default::default()
    };
    let (outcome, emits) = match m.lock() {
        Ok(g) => {
            let r = g.run_script(code, ctx);
            let e = g.drain_emits();
            (r, e)
        }
        Err(_) => return true,
    };

    // Reply scripts rarely emit SysEx but the capability is symmetric; drain
    // anyway so a reply can trigger a follow-up command on the device.
    // We don't have tx/from here — plumb them if a real need arises; for now
    // dropping with a warning is acceptable (reply scripts in practice only
    // update state via ob.set_current_page / ob.state).
    if !emits.is_empty() {
        eprintln!("  reply.script {emit} emitted {} SysEx block(s) but emits \
                   from reply scripts are not yet plumbed to MIDI out",
                  emits.len());
    }

    match outcome {
        Ok(None) => false, // script dropped the event
        Ok(Some(_)) => true,
        Err(e) => {
            eprintln!("  reply.script {emit} err: {e} — emitting OSC anyway");
            true
        }
    }
}

/// Fan out script-emitted OSC messages through the runtime's send_osc.
fn send_script_osc(send_osc: &SendOsc, emits: &[EmitOsc]) {
    for em in emits {
        let args = em.args.iter().map(|a| match a {
            EmitOscArg::Int(i) => OscType::Long(*i),
            EmitOscArg::Float(f) => OscType::Float(*f as f32),
            EmitOscArg::Str(s) => OscType::String(s.clone()),
            EmitOscArg::Bool(b) => OscType::Bool(*b),
        }).collect();
        send_osc(OscMessage { addr: em.addr.clone(), args });
    }
}

/// Wrap (if requested) and enqueue every script-emitted SysEx block.
fn enqueue_emits(
    tx: &Sender<Vec<u8>>,
    emits: &[EmitBlock],
    dev: &Arc<Device>,
    addr: &str,
    from: SocketAddr,
) {
    for block in emits {
        let bytes = if block.wrap {
            frame::wrap_sysex(dev, &block.bytes)
        } else {
            block.bytes.clone()
        };
        enqueue(tx, bytes, addr, from);
    }
}

fn apply_transform(cell: &ScriptCell, expr: Option<&String>, value: i64, addr: &str) -> i64 {
    let Some(code) = expr else { return value; };
    let Some(m) = cell.get() else {
        eprintln!("  [{addr}] transform ignored (engine unavailable)");
        return value;
    };
    match m.lock().ok().and_then(|g| g.eval_transform(code, value).ok()) {
        Some(v) => v,
        None => {
            eprintln!("  [{addr}] transform failed, using raw value");
            value
        }
    }
}

fn drain_midi_out(mut conn: MidiOutputConnection, rx: Receiver<Vec<u8>>, rate_hz: u32) {
    let interval = if rate_hz > 0 {
        Duration::from_micros(1_000_000 / rate_hz as u64)
    } else {
        Duration::ZERO
    };
    let mut last = Instant::now() - interval;
    while let Ok(frm) = rx.recv() {
        if !interval.is_zero() {
            let elapsed = last.elapsed();
            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
        }
        if let Err(e) = conn.send(&frm) { eprintln!("midi send: {e}"); }
        last = Instant::now();
    }
}

pub fn handle_midi_in(
    msg: &[u8],
    dev: &Arc<Device>,
    send: &Arc<dyn Fn(OscMessage) + Send + Sync>,
    routes: &Arc<Mutex<DynamicRoutes>>,
    scripts: &ScriptCell,
) {
    // Dynamic-route fast path: reconfigurable controllers (Electra).
    if let Some(osc_msg) = try_dynamic_cc_in(routes, msg) {
        send(osc_msg);
        return;
    }

    if msg.first() == Some(&0xF0) {
        // The device's identity reply, the same address whatever its maker.
        if let Some(m) = crate::programs::identity_reply(dev, msg) {
            send(m);
            return;
        }
        // Attempt to match one of the reply patterns
        for rp in dev.replies.iter().flatten() {
            if let Some(bindings) = match_reply(msg, dev, rp.match_frame.as_deref().unwrap_or_default()) {
                // Optional reply.script: lets drivers update state (e.g.
                // ob.set_current_page) in response to device-initiated events.
                // Returning nil from the script drops the OSC emission.
                if rp.script.is_some() && !run_reply_script(rp, &bindings, dev, scripts) {
                    return;
                }
                let m = emit_reply_osc(&rp.emit_osc, rp.emit_args.as_deref(), &dev.device.osc_prefix, &bindings);
                record_received_value(scripts, dev, &m);
                send(m);
                return;
            }
        }
        // Fallback: emit raw hex for power users
        let hex = msg.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
        send(OscMessage {
            addr: format!("{}/sysex/raw", dev.device.osc_prefix),
            args: vec![OscType::String(hex)],
        });
        return;
    }
    if let Some(m) = midi_codec::midi_to_osc(msg, dev.midi_in.as_ref(), &dev.device.osc_prefix) {
        send(m);
    }
}

/// Try to match `msg` against `pattern = header + pattern_body + footer`.
/// Returns captured bindings if matched.
fn match_reply(msg: &[u8], dev: &Device, pattern: &[osd::FrameToken]) -> Option<Bindings> {
    let sysex = dev.sysex.as_ref();
    let h = sysex.and_then(|s| s.header.as_deref()).unwrap_or_default();
    let f = sysex.and_then(|s| s.footer.as_deref()).unwrap_or_default();
    if msg.len() < h.len() + f.len() || !msg.starts_with(h) || !msg.ends_with(f) {
        return None;
    }
    let body = &msg[h.len()..msg.len() - f.len()];
    if body.len() != pattern.len() { return None; }
    let mut b = Bindings::new();
    for (pat, byte) in pattern.iter().zip(body.iter()) {
        match pat {
            osd::FrameToken::Byte(l) => {
                if l != byte { return None; }
            }
            osd::FrameToken::Part(osd::FramePart::Capture(name)) => b.set_u7(name, *byte),
            osd::FrameToken::Part(osd::FramePart::Any) => {}
            // A checksum is not verified on what the device sends: any byte stands for it.
            osd::FrameToken::Part(osd::FramePart::Checksum { .. }) => {}
            // 0.4 names the byte in a `{name}` string.
            osd::FrameToken::Placeholder(ph) => {
                let inner = ph.trim_matches(|c| c == '{' || c == '}');
                b.set_u7(inner.split(':').next().unwrap_or("").trim(), *byte);
            }
            // Any other element names no byte to match.
            _ => return None,
        }
    }
    Some(b)
}

fn emit_reply_osc(tpl: &Address, emit_args: Option<&[Scalar]>, prefix: &str, bindings: &Bindings) -> OscMessage {
    let (tpl, emit_args) = reply_parts(tpl, emit_args);
    let addr_tpl = address_text(&tpl);
    let addr = if addr_tpl.starts_with(prefix) || addr_tpl.starts_with('/') {
        addr_tpl
    } else {
        format!("{prefix}/{addr_tpl}")
    };
    let addr = if addr.starts_with(prefix) { addr } else { format!("{prefix}{addr}") };
    let mut args = Vec::new();
    for a in &emit_args {
        let (Scalar::Capture(key) | Scalar::Arg(key)) = a else { continue };
        if let Some(Value::U7(v)) = bindings.get(key) {
            args.push(OscType::Int(*v as i32));
        } else if let Some(Value::U14(v)) = bindings.get(key) {
            args.push(OscType::Int(*v as i32));
        }
    }
    OscMessage { addr, args }
}

// ---- Command dispatch ----

pub struct CommandHandler {
    pub cmd: Command,
    pub pattern_parts: Vec<Part>,
}

pub enum Part { Literal(String), Capture(String) }

pub fn parse_osc_tpl(tpl: &str) -> Vec<Part> {
    let mut out = Vec::new();
    let mut rest = tpl;
    while let Some(open) = rest.find('{') {
        if open > 0 { out.push(Part::Literal(rest[..open].to_string())); }
        if let Some(close) = rest[open..].find('}') {
            let name = rest[open + 1..open + close].to_string();
            out.push(Part::Capture(name));
            rest = &rest[open + close + 1..];
        } else {
            out.push(Part::Literal(rest[open..].to_string()));
            break;
        }
    }
    if !rest.is_empty() { out.push(Part::Literal(rest.to_string())); }
    out
}

pub fn match_osc_path(parts: &[Part], addr: &str) -> Option<HashMap<String, String>> {
    let mut captures = HashMap::new();
    let mut idx = 0;
    let bytes = addr.as_bytes();
    for (i, p) in parts.iter().enumerate() {
        match p {
            Part::Literal(s) => {
                let lb = s.as_bytes();
                if idx + lb.len() > bytes.len() { return None; }
                if &bytes[idx..idx + lb.len()] != lb { return None; }
                idx += lb.len();
            }
            Part::Capture(name) => {
                // Capture up to the next literal (or end of string)
                let next_literal = parts.get(i + 1).and_then(|p| {
                    if let Part::Literal(s) = p { Some(s.as_str()) } else { None }
                });
                let end = if let Some(lit) = next_literal {
                    let needle = lit.as_bytes();
                    let search = &bytes[idx..];
                    let pos = search.windows(needle.len()).position(|w| w == needle);
                    pos.map(|p| idx + p).unwrap_or(bytes.len())
                } else {
                    bytes.len()
                };
                let captured = std::str::from_utf8(&bytes[idx..end]).ok()?.to_string();
                captures.insert(name.clone(), captured);
                idx = end;
            }
        }
    }
    if idx == bytes.len() { Some(captures) } else { None }
}

/// The first handler matching an address wins, so the ones with fewer
/// captures come first: a script's `/param/cutoff` before the norns'
/// `/param/{id}`. Ties keep the driver's order.
pub fn build_command_map(dev: &Device) -> Vec<CommandHandler> {
    let mut cmds: Vec<CommandHandler> = dev.commands.iter().flatten().map(|c| {
        let full = full_osc_path(dev, &address_text(&c.osc));
        let pattern_parts = parse_osc_tpl(&full);
        CommandHandler { cmd: c.clone(), pattern_parts }
    }).collect();
    cmds.sort_by_key(|h| h.pattern_parts.iter().filter(|p| matches!(p, Part::Capture(_))).count());
    cmds
}

pub fn build_param_map(dev: &Device) -> HashMap<String, ParamEntry> {
    let mut m = HashMap::new();
    if let Some(pt) = &dev.params {
        for e in &pt.entries {
            m.insert(full_osc_path(dev, &e.osc), e.clone());
        }
    }
    m
}

pub fn build_cc_param_map(dev: &Device) -> HashMap<String, (CcEntry, u8)> {
    let mut m = HashMap::new();
    if let Some(cct) = &dev.cc_params {
        let default_ch = cct.channel.unwrap_or(0);
        for e in &cct.entries {
            let ch = e.channel.unwrap_or(default_ch);
            m.insert(full_osc_path(dev, &e.osc), (e.clone(), ch));
        }
    }
    m
}

pub type SendOsc = Arc<dyn Fn(OscMessage) + Send + Sync>;

/// Per-device dispatch state shared between the single-device runtime and
/// the orchestrator (multi-device). Lets the orchestrator reuse the full
/// dispatch pipeline (commands, params, cc_params, custom_commands) instead
/// of the previous perf-MIDI-only subset.
pub struct DeviceDispatch {
    pub device: Arc<Device>,
    pub cmds: Vec<CommandHandler>,
    pub params: HashMap<String, ParamEntry>,
    pub cc_params: HashMap<String, (CcEntry, u8)>,
    pub routes: Arc<Mutex<DynamicRoutes>>,
    pub scripts: ScriptCell,
    pub tx: Sender<Vec<u8>>,
}

impl DeviceDispatch {
    pub fn build(device: Device, tx: Sender<Vec<u8>>) -> Self {
        let dev = Arc::new(device);
        let cmds = build_command_map(&dev);
        let params = build_param_map(&dev);
        let cc_params = build_cc_param_map(&dev);
        let routes = DynamicRoutes::handle();
        let scripts: ScriptCell = Arc::new(OnceLock::new());
        if device_uses_scripting(&dev) {
            if let Ok(e) = ScriptEngine::new_with_routes_for_device(routes.clone(), &dev) {
                let _ = scripts.set(Mutex::new(e));
            }
        }
        Self { device: dev, cmds, params, cc_params, routes, scripts, tx }
    }
}

/// The MIDI messages `msg` would send to `device`, without opening a port:
/// the full dispatch pipeline (commands, params, CC/NRPN, triggers, scripts)
/// writing into a queue that is read back. Used by the relay, where the
/// hardware is on another machine.
pub fn encode(device: Device, msg: OscMessage) -> Vec<Vec<u8>> {
    let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = bounded(1024);
    let state = DeviceDispatch::build(device, tx);
    let discard: SendOsc = Arc::new(|_| {});
    dispatch_to_device(&state, msg, &discard, SocketAddr::from(([0, 0, 0, 0], 0)));
    rx.try_iter().collect()
}

/// Public dispatch entry point used by the orchestrator. For MIDI/SysEx
/// devices: runs the full command / param / cc_param / midi_out pipeline
/// just like the single-device runtime.
pub fn dispatch_to_device(
    state: &DeviceDispatch,
    msg: OscMessage,
    send_osc: &SendOsc,
    from: SocketAddr,
) {
    handle_message(msg, state, send_osc, from);
}

/// Per-device dispatch state for an OSC-transport (software) device — the
/// orchestrator equivalent of what the single-device OSC runtime keeps. Lets the
/// orchestrator host hardware (MIDI) and software (OSC) devices side by side.
pub struct OscDeviceHandle {
    pub device: Arc<Device>,
    pub cmds: Vec<CommandHandler>,
    pub target_sock: Arc<UdpSocket>,
    pub target_addr: SocketAddr,
}

impl OscDeviceHandle {
    /// Build the handle: resolve the OSC target from `device.transport`,
    /// open the send socket, pre-compute the command map. Does NOT open the
    /// reply listener or fire subscriptions — the caller owns those so it
    /// can pass its own (route-aware) `send_osc`.
    pub fn build(device: Device) -> Result<Self> {
        let dev = Arc::new(device);
        let transport = dev.device.transport.as_ref()
            .ok_or_else(|| anyhow!("OSC device requires device.transport block"))?;
        let host = transport.host.as_deref().unwrap_or("127.0.0.1");
        let port = transport.port
            .ok_or_else(|| anyhow!("OSC device requires device.transport.port"))?;
        let target_addr: SocketAddr = format!("{host}:{port}").parse()
            .with_context(|| format!("invalid OSC target {host}:{port}"))?;
        let target_sock = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
        let cmds = build_command_map(&dev);
        Ok(Self { device: dev, cmds, target_sock, target_addr })
    }
}

/// Public dispatch entry point for OSC-transport devices in the orchestrator.
pub fn dispatch_osc_to_device(
    handle: &OscDeviceHandle,
    msg: OscMessage,
    send_osc: &SendOsc,
    from: SocketAddr,
) {
    handle_osc_message(msg, &handle.device, &handle.cmds,
                       &handle.target_sock, handle.target_addr, send_osc, from);
}

fn dispatch(pkt: OscPacket, state: &DeviceDispatch, send_osc: &SendOsc, from: SocketAddr) {
    match pkt {
        OscPacket::Message(m) => handle_message(m, state, send_osc, from),
        OscPacket::Bundle(b) => {
            for p in b.content { dispatch(p, state, send_osc, from); }
        }
    }
}

fn handle_message(msg: OscMessage, state: &DeviceDispatch, send_osc: &SendOsc, from: SocketAddr) {
    let DeviceDispatch { device: dev, cmds, params, cc_params, routes, scripts, tx } = state;
    // /bridge/status — RPC-style health check; emit one reply per device.
    if msg.addr == "/bridge/status" {
        for reply in bridge_status_replies(std::slice::from_ref(dev.as_ref())) {
            send_osc(reply);
        }
        return;
    }

    // /bridge/docs — integration guide fetch. Returns one reply per device
    // that has a companion .md loaded. The full markdown text is carried
    // in a single string arg — clients (Kanopi, LLM agents) can parse it.
    if msg.addr == "/bridge/docs" {
        for reply in loaded_docs_replies(std::slice::from_ref(dev.as_ref())) {
            send_osc(reply);
        }
        return;
    }

    // Scripted custom_commands (preset upload, dynamic-route init,
    // introspection). Matched on exact full OSC path.
    for cc in dev.custom_commands.iter().flatten() {
        let full = full_osc_path(dev, &cc.osc);
        if msg.addr == full {
            run_custom_command(cc, &msg, dev, scripts, tx, send_osc, from);
            return;
        }
    }

    // Dynamic routes (reconfigurable controllers): semantic OSC addr → CC.
    if let Some(bytes) = try_dynamic_osc_out(routes, &msg) {
        enqueue(tx, bytes, &msg.addr, from);
        return;
    }

    // The same addresses on every device: identity, program recall and dump.
    match crate::programs::uniform_osc(dev, &msg.addr, &msg.args) {
        Some(Ok(frames)) => { for f in frames { enqueue(tx, f, &msg.addr, from); } return; }
        Some(Err(e)) => { eprintln!("  {}: {e}", msg.addr); return; }
        None => {}
    }

    // Performance MIDI out (midi_out section enables these routes).
    if let Some(mo) = &dev.midi_out {
        if let Some(bytes) = try_midi_out(&msg, mo, &dev.device.osc_prefix) {
            enqueue(tx, bytes, &msg.addr, from);
            return;
        }
    }

    // Named note triggers (drum machines, samplers).
    match trigger_note_on(dev, &msg) {
        Some(Ok(bytes)) => { enqueue(tx, bytes, &msg.addr, from); return; }
        Some(Err(e)) => { eprintln!("  trigger {}: {e}", msg.addr); return; }
        None => {}
    }

    // Try CC-param match (pure MIDI, no SysEx)
    if let Some((entry, default_ch)) = cc_params.get(&msg.addr) {
        if let Some(v) = msg.args.first() {
            let val_i = match v {
                OscType::Int(i) => *i as i64,
                OscType::Long(i) => *i,
                OscType::Float(f) => *f as i64,
                OscType::Double(f) => *f as i64,
                OscType::Bool(x) => if *x { 1 } else { 0 },
                _ => { eprintln!("  cc arg not numeric"); return; }
            };
            let val_i = apply_transform(scripts, entry.transform.as_ref(), val_i, &msg.addr);
            let [lo, hi] = entry.range.unwrap_or([0, 127]);
            let clamped = val_i.clamp(lo, hi);
            match cc_messages(entry, *default_ch, clamped, &msg.args) {
                Ok(frames) => for f in frames { enqueue(tx, f, &msg.addr, from); },
                Err(e) => eprintln!("  {}: {e}", msg.addr),
            }
            return;
        }
    }

    // Try direct param match
    if let Some(entry) = params.get(&msg.addr) {
        if let Some(v) = msg.args.first() {
            if let Some(pt) = &dev.params {
                // Apply transform (if any) by extracting numeric → transform → re-wrap.
                let v_transformed = if entry.transform.is_some() {
                    let as_i = match v {
                        OscType::Int(i) => *i as i64,
                        OscType::Long(i) => *i,
                        OscType::Float(f) => *f as i64,
                        OscType::Double(f) => *f as i64,
                        OscType::Bool(x) => if *x { 1 } else { 0 },
                        _ => { eprintln!("  param arg not numeric"); return; }
                    };
                    let t = apply_transform(scripts, entry.transform.as_ref(), as_i, &msg.addr);
                    OscType::Long(t)
                } else {
                    v.clone()
                };
                match frame::bindings_for_param_set(pt, entry, &v_transformed) {
                    Ok(b) => {
                        match frame::build_frame(dev, &pt.set_frame, &b) {
                            Ok(frm) => { enqueue(tx, frm, &msg.addr, from); return; }
                            Err(e) => eprintln!("build_frame err: {}: {}: {e}", dev.device.name, msg.addr),
                        }
                    }
                    Err(e) => eprintln!("coerce err: {e}"),
                }
            }
        }
        return;
    }

    // Try command match
    for h in cmds {
        if let Some(caps) = match_osc_path(&h.pattern_parts, &msg.addr) {
            match dispatch_command(&h.cmd, &caps, &msg.args, dev) {
                Ok((frm, slots)) => {
                    record_sent_value(scripts, &h.cmd, &msg.args);
                    let built_len = frm.len();
                    if let Some(post) = maybe_run_cmd_script(scripts, &h.cmd, frm, &dev.device.name) {
                        match finish_cmd_checksums(dev, &h.cmd, built_len, post, &slots) {
                            Ok(post) => enqueue(tx, post, &msg.addr, from),
                            Err(e) => eprintln!("command '{}' err: {e}", address_text(&h.cmd.osc)),
                        }
                    }
                }
                Err(e) => eprintln!("command '{}' err: {e}", address_text(&h.cmd.osc)),
            }
            return;
        }
    }

    // Special: raw sysex passthrough
    let raw = full_osc_path(dev, "/raw/syx");
    if msg.addr == raw {
        if let Some(OscType::String(s)) = msg.args.first() {
            let bytes: Vec<u8> = s.split_ascii_whitespace()
                .filter_map(|t| u8::from_str_radix(t.trim_start_matches("0x"), 16).ok())
                .collect();
            if bytes.first() == Some(&0xF0) && bytes.last() == Some(&0xF7) {
                enqueue(tx, bytes, &msg.addr, from);
                return;
            }
        }
    }

    // Param-value GET convenience: `/xxx/param/get` with 4 u7 args
    let get_addr = full_osc_path(dev, "/param/get");
    if msg.addr == get_addr && msg.args.len() >= 4 {
        if let Some(pt) = &dev.params {
            let as_u7 = |t: &OscType| match t {
                OscType::Int(i) => Some((*i as i64).clamp(0, 127) as u8),
                _ => None,
            };
            if let (Some(pr), Some(p), Some(c), Some(r)) =
                (as_u7(&msg.args[0]), as_u7(&msg.args[1]), as_u7(&msg.args[2]), as_u7(&msg.args[3])) {
                let mut b = Bindings::new();
                b.set_u7("pr", pr); b.set_u7("p", p); b.set_u7("c", c); b.set_u7("r", r);
                match frame::build_frame(dev, &pt.get_frame, &b) {
                    Ok(frm) => { enqueue(tx, frm, &msg.addr, from); return; }
                    Err(e) => eprintln!("build_frame err: {}: {}: {e}", dev.device.name, msg.addr),
                }
            }
        }
    }

    eprintln!("  unknown OSC: {}", msg.addr);
}

fn dispatch_command(cmd: &Command, caps: &HashMap<String, String>, args: &[OscType], dev: &Device) -> Result<(Vec<u8>, Vec<frame::ChecksumSlot>)> {
    let mut b = Bindings::new();

    // Bind captured path parts as u7 (if they parse) or as strings
    for (k, v) in caps {
        if let Ok(n) = v.parse::<i64>() {
            b.set_u7(k, n.clamp(0, 127) as u8);
        } else {
            b.set_bytes(k, v.as_bytes().to_vec());
        }
    }

    // Bind declared args — maintain a separate counter for OSC args because
    // some arg names are pre-bound from URL path captures.
    let mut osc_idx = 0usize;
    for spec in cmd.args.iter().flatten() {
        if b.get(&spec.name).is_some() { continue; } // already bound by URL capture
        match args.get(osc_idx) {
            Some(a) => {
                let v = frame::coerce_arg(spec, a)?;
                b.0.insert(spec.name.clone(), v);
                osc_idx += 1;
            }
            None => {
                // Apply default if present
                if let Some(default) = &spec.default {
                    let default_osc = match default {
                        serde_json::Value::String(s) => OscType::String(s.clone()),
                        serde_json::Value::Number(n) => OscType::Int(n.as_i64().unwrap_or(0) as i32),
                        serde_json::Value::Bool(b) => OscType::Bool(*b),
                        _ => OscType::Int(0),
                    };
                    let v = frame::coerce_arg(spec, &default_osc)?;
                    b.0.insert(spec.name.clone(), v);
                } else {
                    anyhow::bail!("missing arg {} for {}", spec.name, address_text(&cmd.osc));
                }
            }
        }
    }

    // Evaluate pre-bindings (simple expression engine)
    for pb in cmd.pre.iter().flatten() {
        let v = eval_pre(&pb.expr, &b)?;
        b.set_u7(&pb.bind, v);
    }

    let origin = |e: anyhow::Error| anyhow::anyhow!("{}: {}: {e}", dev.device.name, full_osc_path(dev, &address_text(&cmd.osc)));
    let frame_tokens = cmd.frame.as_deref().unwrap_or_default();
    if cmd.script.is_some() {
        return frame::build_frame_deferred(dev, frame_tokens, &b).map_err(origin);
    }
    frame::build_frame(dev, frame_tokens, &b).map(|f| (f, Vec::new())).map_err(origin)
}

/// Compute the checksums a command's frame left for after its script, over the
/// script's final bytes; a script that changed the frame's length is refused.
fn finish_cmd_checksums(dev: &Device, cmd: &Command, built_len: usize, mut frame: Vec<u8>, slots: &[frame::ChecksumSlot]) -> Result<Vec<u8>> {
    if slots.is_empty() { return Ok(frame); }
    let origin = || format!("{}: {}", dev.device.name, full_osc_path(dev, &address_text(&cmd.osc)));
    if frame.len() != built_len {
        anyhow::bail!("{}: script changed the frame from {built_len} to {} bytes; a command with a checksum keeps its length", origin(), frame.len());
    }
    frame::finish_checksums(&mut frame, slots).map_err(|e| anyhow::anyhow!("{}: {e}", origin()))?;
    Ok(frame)
}

/// The value of a `pre` expression: a tree of OSD 0.5 (`add`, `sub` and
/// `mul` bounded to 0..127, comparisons 0 or 1, `if` on a condition not 0), or
/// a 0.4 expression string.
fn eval_pre(expr: &osd::Expr, b: &Bindings) -> Result<u8> {
    use osd::{BinOp, Expr};
    Ok(match expr {
        Expr::Text(s) => eval_expr(s, b)?,
        Expr::Literal(n) => *n,
        Expr::Arg(name) => match b.get(name) {
            Some(Value::U7(v)) => *v,
            Some(Value::U14(v)) => (*v & 0x7F) as u8,
            _ => anyhow::bail!("unknown identifier in expr: '{name}'"),
        },
        Expr::Binary(op, l, r) => {
            let (l, r) = (eval_pre(l, b)? as i64, eval_pre(r, b)? as i64);
            match op {
                BinOp::Add => (l + r).clamp(0, 127) as u8,
                BinOp::Sub => (l - r).clamp(0, 127) as u8,
                BinOp::Mul => (l * r).clamp(0, 127) as u8,
                BinOp::Gt => (l > r) as u8,
                BinOp::Lt => (l < r) as u8,
                BinOp::Eq => (l == r) as u8,
                BinOp::Ne => (l != r) as u8,
            }
        }
        Expr::If(c, t, e) => if eval_pre(c, b)? != 0 { eval_pre(t, b)? } else { eval_pre(e, b)? },
    })
}

/// Extremely limited expression evaluator supporting `name + N`, `name - N`,
/// `name * N`, `name > N ? X : Y` — enough for our declared pre-binds.
fn eval_expr(expr: &str, b: &Bindings) -> Result<u8> {
    let expr = expr.trim();
    // ternary: `cond ? a : b`
    if let Some(q) = expr.find('?') {
        if let Some(c) = expr.find(':') {
            let cond = expr[..q].trim();
            let t = expr[q + 1..c].trim();
            let f = expr[c + 1..].trim();
            let cv = eval_expr(cond, b)?;
            return if cv != 0 { eval_expr(t, b) } else { eval_expr(f, b) };
        }
    }
    // compare: `X > Y`, `X == Y` -> 0 or 1
    for op in [" > ", " < ", " == ", " != "] {
        if let Some(idx) = expr.find(op) {
            let l = eval_expr(expr[..idx].trim(), b)? as i64;
            let r = eval_expr(expr[idx + op.len()..].trim(), b)? as i64;
            let v = match op.trim() {
                ">" => (l > r) as u8,
                "<" => (l < r) as u8,
                "==" => (l == r) as u8,
                "!=" => (l != r) as u8,
                _ => 0,
            };
            return Ok(v);
        }
    }
    // additive: a+b / a-b
    for (op, add) in [("+", true), ("-", false)] {
        if let Some(idx) = expr.find(op) {
            if idx > 0 {  // skip unary minus / plus
                let l = eval_expr(expr[..idx].trim(), b)? as i64;
                let r = eval_expr(expr[idx + 1..].trim(), b)? as i64;
                let v = if add { l + r } else { l - r };
                return Ok(v.clamp(0, 127) as u8);
            }
        }
    }
    // multiplicative
    if let Some(idx) = expr.find('*') {
        let l = eval_expr(expr[..idx].trim(), b)? as i64;
        let r = eval_expr(expr[idx + 1..].trim(), b)? as i64;
        return Ok((l * r).clamp(0, 127) as u8);
    }
    // numeric literal?
    if let Ok(n) = expr.parse::<i64>() {
        return Ok(n.clamp(0, 127) as u8);
    }
    // otherwise a binding name
    match b.get(expr) {
        Some(Value::U7(v)) => Ok(*v),
        Some(Value::U14(v)) => Ok((*v & 0x7F) as u8),
        _ => anyhow::bail!("unknown identifier in expr: '{expr}'"),
    }
}

/// The Control Change messages for one cc_params entry set to `value`
/// (already transformed and clamped): a CC (+ its LSB CC for 14-bit), or a
/// parameter number (NRPN 99/98 or RPN 101/100) then data entry 6 (+38 for
/// 14-bit). `args` is the OSC call: value, then the NRPN LSB for an entry
/// with `nrpn_lsb_range`, then an optional channel.
fn cc_messages(entry: &CcEntry, default_ch: u8, value: i64, args: &[OscType]) -> Result<Vec<Vec<u8>>, String> {
    let int_arg = |i: usize| args.get(i).and_then(|a| match a {
        OscType::Int(i) => Some(*i as i64),
        OscType::Long(i) => Some(*i),
        _ => None,
    });
    let lsb_from_call = entry.nrpn_lsb_range.filter(|_| entry.nrpn_msb.is_some() && entry.nrpn_lsb.is_none());
    let ch = int_arg(if lsb_from_call.is_some() { 2 } else { 1 }).map(|i| (i & 0x0F) as u8).unwrap_or(default_ch);
    let status = 0xB0 | (ch & 0x0F);
    let is_14bit = entry.range.unwrap_or([0, 127])[1] > 127;
    let msb = if is_14bit { ((value >> 7) & 0x7F) as u8 } else { (value as u8) & 0x7F };
    let lsb = (value as u8) & 0x7F;
    let number = match (entry.nrpn_msb, entry.nrpn_lsb, lsb_from_call, entry.rpn_msb, entry.rpn_lsb) {
        (Some(m), Some(l), _, _, _) => Some((99, m, 98, l)),
        (Some(m), None, Some([lo, hi]), _, _) => {
            let l = int_arg(1).ok_or(format!("needs its NRPN LSB ({lo}-{hi}) after the value"))?;
            Some((99, m, 98, l.clamp(lo, hi) as u8))
        }
        (_, _, _, Some(m), Some(l)) => Some((101, m, 100, l)),
        _ => None,
    };
    let mut out = Vec::new();
    if let Some((cm, m, cl, l)) = number {
        out.push(vec![status, cm, m & 0x7F]);
        out.push(vec![status, cl, l & 0x7F]);
        out.push(vec![status, 6, msb]);
        if is_14bit { out.push(vec![status, 38, lsb]); }
    } else if let Some(cc) = entry.cc {
        out.push(vec![status, cc & 0x7F, msb]);
        if let (true, Some(lsb_cc)) = (is_14bit, entry.cc_lsb) { out.push(vec![status, lsb_cc & 0x7F, lsb]); }
    } else {
        return Err("no cc, nrpn or rpn address".into());
    }
    Ok(out)
}

fn enqueue(tx: &Sender<Vec<u8>>, frm: Vec<u8>, addr: &str, from: SocketAddr) {
    let len = frm.len();
    if let Err(e) = tx.try_send(frm) {
        eprintln!("  [{from}] queue full for {addr}: {e}");
    } else {
        eprintln!("  [{from}] {addr} -> {len} bytes SysEx");
    }
}

// ---- OSC transport (software drivers: DAW, live coding) ----

/// Drives drivers declared with `device.transport.kind = "osc"`. Skips midir
/// entirely, opens UDP sockets toward the OSC target and (optionally) a reply
/// listener, fires startup `subscriptions`, then loops on the client-facing
/// OSC bind dispatching each incoming command through `Command.forward`.
fn bind_osc(opts: RuntimeOptions) -> Result<BoundRuntime> {
    let dev = Arc::new(opts.device);
    let transport = dev.device.transport.as_ref()
        .ok_or_else(|| anyhow!("OSC driver requires `device.transport` block"))?;
    let host = transport.host.as_deref().unwrap_or("127.0.0.1");
    let port = transport.port
        .ok_or_else(|| anyhow!("OSC driver requires `device.transport.port`"))?;
    let target_addr: SocketAddr = format!("{host}:{port}").parse()
        .with_context(|| format!("invalid OSC target {host}:{port}"))?;

    // Bridge → OSC target (commands and subscriptions).
    let target_sock = Arc::new(UdpSocket::bind("0.0.0.0:0")?);

    // Bridge → OSC clients (replies/events fanned out over UDP and WS).
    let ws_clients = opts.ws_bind.as_ref().map(|_| crate::ws::WsClients::new());
    let osc_sock = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
    let clients = opts.osc_clients.clone();
    let sock_cb = osc_sock.clone();
    let ws_cb = ws_clients.clone();
    let send_osc: SendOsc = Arc::new(move |msg| {
        let ws_active = ws_cb.as_ref().is_some_and(|w| w.has_clients());
        if clients.is_empty() && !ws_active { return; }
        if let Ok(bytes) = rosc::encoder::encode(&OscPacket::Message(msg)) {
            for tgt in &clients { let _ = sock_cb.send_to(&bytes, tgt); }
            if let Some(w) = &ws_cb { w.broadcast(&bytes); }
        }
    });

    // Reply listener (OSC target → bridge → clients), if configured.
    if let Some(reply_port) = transport.reply_port {
        let bind = format!("0.0.0.0:{reply_port}");
        let reply_sock = UdpSocket::bind(&bind)
            .with_context(|| format!("bind OSC reply socket {bind}"))?;
        let dev_cb = dev.clone();
        let send_osc_cb = send_osc.clone();
        eprintln!("OSC REPLY {bind}");
        std::thread::spawn(move || run_osc_reply_loop(reply_sock, dev_cb, send_osc_cb));
    } else if dev.replies.iter().flatten().any(|r| r.match_osc.is_some()) {
        eprintln!("WARN: device declares OSC replies but no transport.reply_port — they will never fire");
    }

    // One-shot startup subscriptions (e.g. /live/song/start_listen/tempo).
    let empty_bindings: HashMap<String, OscType> = HashMap::new();
    for sub in dev.subscriptions.iter().flatten() {
        if sub.on.is_some_and(|on| on != osd::SubscriptionOn::Startup) { continue; }
        if let Some(out_msg) = build_forward_msg(&sub.forward, &empty_bindings) {
            send_osc_to_target(&target_sock, target_addr, &out_msg);
            eprintln!("OSC SUB  {} -> {target_addr}", out_msg.addr);
        }
    }

    eprintln!("Device:  {} ({})", dev.device.name, dev.device.revision.as_deref().unwrap_or_default());
    eprintln!("OSC TGT  {target_addr}");
    eprintln!("OSC IN   {}", opts.osc_bind);
    for c in &opts.osc_clients { eprintln!("OSC CLI  {c}"); }

    let cmds = Arc::new(build_command_map(&dev));

    // WebSocket transport — same dispatch as the UDP loop below.
    let mut ws_local = None;
    if let (Some(bind), Some(wsc)) = (&opts.ws_bind, &ws_clients) {
        let dispatch_cb: crate::ws::WsDispatch = {
            let dev = dev.clone();
            let cmds = cmds.clone();
            let target_sock = target_sock.clone();
            let send_osc = send_osc.clone();
            Arc::new(move |pkt, from| {
                dispatch_osc(pkt, &dev, &cmds, &target_sock, target_addr, &send_osc, from);
            })
        };
        let local = crate::ws::serve(bind, wsc.clone(), dispatch_cb, opts.ws_allow_origins.clone())?;
        eprintln!("WS IN    {local}");
        ws_local = Some(local);
    }

    let sock = UdpSocket::bind(&opts.osc_bind)
        .with_context(|| format!("bind {}", opts.osc_bind))?;
    let osc_addr = sock.local_addr()?;
    serve_oscquery(&opts.oscquery_bind, std::slice::from_ref(dev.as_ref()), &sock, ws_local)?;
    let osc_clients = opts.osc_clients;

    let serve = move || -> Result<()> {
        eprintln!("Ready. Ctrl-C to stop.");
        let mut buf = [0u8; 65536];
        loop {
            let (n, from) = match sock.recv_from(&mut buf) {
                Ok(p) => p,
                Err(e) => { eprintln!("recv: {e}"); continue; }
            };
            match rosc::decoder::decode_udp(&buf[..n]) {
                Ok((_, pkt)) => {
                    answer_query(&sock, &pkt, from, std::slice::from_ref(dev.as_ref()), &osc_clients);
                    dispatch_osc(pkt, &dev, &cmds, &target_sock, target_addr, &send_osc, from)
                }
                Err(e) => eprintln!("OSC decode err from {from}: {e}"),
            }
        }
    };
    Ok(BoundRuntime { osc_addr, serve: Box::new(serve) })
}

fn dispatch_osc(
    pkt: OscPacket,
    dev: &Arc<Device>,
    cmds: &[CommandHandler],
    target_sock: &Arc<UdpSocket>,
    target: SocketAddr,
    send_osc: &SendOsc,
    from: SocketAddr,
) {
    match pkt {
        OscPacket::Message(m) => handle_osc_message(m, dev, cmds, target_sock, target, send_osc, from),
        OscPacket::Bundle(b) => {
            for p in b.content {
                dispatch_osc(p, dev, cmds, target_sock, target, send_osc, from);
            }
        }
    }
}

pub fn handle_osc_message(
    msg: OscMessage,
    dev: &Arc<Device>,
    cmds: &[CommandHandler],
    target_sock: &Arc<UdpSocket>,
    target: SocketAddr,
    send_osc: &SendOsc,
    from: SocketAddr,
) {
    // /bridge/status, /bridge/docs — same surface as MIDI flow.
    if msg.addr == "/bridge/status" {
        for r in bridge_status_replies(std::slice::from_ref(dev.as_ref())) { send_osc(r); }
        return;
    }
    if msg.addr == "/bridge/docs" {
        for r in loaded_docs_replies(std::slice::from_ref(dev.as_ref())) { send_osc(r); }
        return;
    }

    // Passthrough: forward any /<osc_prefix>/... as /<passthrough_prefix>/...
    // verbatim. No command dispatch.
    if let Some(pt) = dev.device.transport.as_ref().and_then(|t| t.passthrough_prefix.as_deref()) {
        if let Some(rest) = msg.addr.strip_prefix(dev.device.osc_prefix.as_str()) {
            let new_addr = if pt.is_empty() {
                // Strip the prefix entirely.
                rest.to_string()
            } else {
                format!("{pt}{rest}")
            };
            let out = OscMessage { addr: new_addr, args: msg.args.clone() };
            send_osc_to_target(target_sock, target, &out);
            eprintln!("  [{from}] passthrough {} -> {}", msg.addr, out.addr);
        } else {
            eprintln!("  [{from}] passthrough miss (prefix not matched) for {}", msg.addr);
        }
        return;
    }

    for handler in cmds {
        if let Some(caps) = match_osc_path(&handler.pattern_parts, &msg.addr) {
            let cmd = &handler.cmd;
            let Some(forward) = &cmd.forward else {
                eprintln!("  [{from}] {} matched but no `forward` block", msg.addr);
                return;
            };
            let bindings = build_forward_bindings(cmd, &caps, &msg.args);
            if let Some(out_msg) = build_forward_msg(forward, &bindings) {
                send_osc_to_target(target_sock, target, &out_msg);
                eprintln!("  [{from}] {} -> {} {:?}", msg.addr, out_msg.addr, out_msg.args);
            } else {
                eprintln!("  [{from}] {} -> forward build failed (missing binding?)", msg.addr);
            }
            return;
        }
    }
    eprintln!("  [{from}] no command for {}", msg.addr);
}

/// Compose the bindings that interpolate into a `ForwardSpec`. Path captures
/// (e.g. `{n}` in `/track/{n}/volume`) are typed using the command's `args`
/// table; remaining positional OSC args from the incoming message bind by
/// order to the next un-captured `args` entry.
fn build_forward_bindings(
    cmd: &Command,
    path_caps: &HashMap<String, String>,
    incoming_args: &[OscType],
) -> HashMap<String, OscType> {
    let mut out: HashMap<String, OscType> = HashMap::new();
    let arg_ty: HashMap<&str, osd::ArgType> = cmd.args.iter().flatten()
        .map(|a| (a.name.as_str(), a.ty))
        .collect();
    for (k, v) in path_caps {
        let ty = arg_ty.get(k.as_str()).copied().unwrap_or(osd::ArgType::String);
        if let Some(osc) = coerce_str_to_osc(v, ty) {
            out.insert(k.clone(), osc);
        }
    }
    let mut osc_iter = incoming_args.iter();
    for spec in cmd.args.iter().flatten() {
        if path_caps.contains_key(&spec.name) { continue; }
        match osc_iter.next() {
            Some(v) => { out.insert(spec.name.clone(), coerce_osc_to_type(v, spec.ty)); }
            None => break,
        }
    }
    out
}

/// Build an outgoing OscMessage from a ForwardSpec template. Literal int /
/// float / bool args carry through; string args either literal-out, or strip
/// the surrounding `{}` and substitute from bindings.
pub fn build_forward_msg(
    forward: &osd::Forward,
    bindings: &HashMap<String, OscType>,
) -> Option<OscMessage> {
    let addr = interp_address(&forward.path, bindings)?;
    let mut args: Vec<OscType> = Vec::new();
    for fa in forward.args.iter().flatten() {
        match fa {
            Scalar::Number(n) => match n.as_i64() {
                Some(i) => args.push(OscType::Int(i as i32)),
                None => args.push(OscType::Float(n.as_f64().unwrap_or(0.0) as f32)),
            },
            Scalar::Bool(b) => args.push(OscType::Bool(*b)),
            // 0.4 writes an argument taken from the message as `{name}`; in 0.5 a string is a literal.
            Scalar::String(s) => match strip_placeholder(s) {
                Some(name) => args.push(bindings.get(name)?.clone()),
                None => args.push(OscType::String(s.clone())),
            },
            Scalar::Arg(name) | Scalar::Capture(name) => args.push(bindings.get(name)?.clone()),
        }
    }
    Some(OscMessage { addr, args })
}

/// `name` of a text written `{name}`: how 0.4 takes an argument.
fn strip_placeholder(s: &str) -> Option<&str> {
    s.strip_prefix('{').and_then(|r| r.strip_suffix('}'))
}

/// A 0.4 address template with each `{name}` replaced by its binding; None
/// when one is not bound.
fn interp_placeholders(tpl: &str, bindings: &HashMap<String, OscType>) -> Option<String> {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}')?;
        let name = &rest[open + 1..open + close];
        out.push_str(&osc_to_string(bindings.get(name)?));
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// What a reply is reported at and with. In 0.4 the address and its arguments
/// are one text (`/cc/{num} {value}`); in 0.5 the address is its own value and
/// the arguments are `emit_args`.
fn reply_parts(tpl: &Address, emit_args: Option<&[Scalar]>) -> (Address, Vec<Scalar>) {
    match tpl {
        Address::Text(t) if t.contains(' ') => {
            let (addr, rest) = t.split_once(' ').unwrap_or((t, ""));
            let args = rest.split_whitespace().map(|tok| match strip_placeholder(tok) {
                Some(name) => Scalar::Arg(name.to_string()),
                None => match tok.parse::<serde_json::Number>() {
                    Ok(n) => Scalar::Number(n),
                    Err(_) => Scalar::String(tok.to_string()),
                },
            }).collect();
            (Address::Text(addr.to_string()), args)
        }
        _ => (tpl.clone(), emit_args.unwrap_or_default().to_vec()),
    }
}

/// The address with each capture replaced by its binding; None when one is
/// not bound.
fn interp_address(a: &Address, bindings: &HashMap<String, OscType>) -> Option<String> {
    match a {
        Address::Text(t) => interp_placeholders(t, bindings),
        Address::Segments(segments) => {
            let mut out = String::new();
            for s in segments {
                match s {
                    osd::Segment::Text(t) => out.push_str(t),
                    osd::Segment::Capture(name) => out.push_str(&osc_to_string(bindings.get(name)?)),
                }
            }
            Some(out)
        }
    }
}

fn osc_to_string(v: &OscType) -> String {
    match v {
        OscType::Int(i) => i.to_string(),
        OscType::Long(i) => i.to_string(),
        OscType::Float(f) => f.to_string(),
        OscType::Double(d) => d.to_string(),
        OscType::Bool(b) => b.to_string(),
        OscType::String(s) => s.clone(),
        _ => String::new(),
    }
}

fn coerce_str_to_osc(s: &str, ty: osd::ArgType) -> Option<OscType> {
    use osd::ArgType;
    match ty {
        ArgType::Int | ArgType::U7 | ArgType::U14 => s.parse::<i32>().ok().map(OscType::Int),
        ArgType::Float => s.parse::<f32>().ok().map(OscType::Float),
        ArgType::Bool => match s {
            "0" | "false" => Some(OscType::Bool(false)),
            "1" | "true" => Some(OscType::Bool(true)),
            _ => None,
        },
        _ => Some(OscType::String(s.to_string())),
    }
}

/// Coerce an incoming OscType to the type declared in `cmd.args[i].ty`.
/// Best-effort: types that don't cross map straight (e.g. Int → "float")
/// are converted; mismatches that make no sense pass through.
fn coerce_osc_to_type(v: &OscType, ty: osd::ArgType) -> OscType {
    use osd::ArgType;
    match (v, ty) {
        (OscType::Int(_), ArgType::Int) | (OscType::Float(_), ArgType::Float)
        | (OscType::Bool(_), ArgType::Bool) | (OscType::String(_), ArgType::String) => v.clone(),
        (OscType::Int(i), ArgType::Float) => OscType::Float(*i as f32),
        (OscType::Long(i), ArgType::Float) => OscType::Float(*i as f32),
        (OscType::Float(f), ArgType::Int) => OscType::Int(*f as i32),
        (OscType::Double(f), ArgType::Int) => OscType::Int(*f as i32),
        (OscType::Long(i), ArgType::Int) => OscType::Int(*i as i32),
        (OscType::Double(f), ArgType::Float) => OscType::Float(*f as f32),
        _ => v.clone(),
    }
}

/// The type a reply's `match_args` entry names (`int`, `float`, `string`,
/// `bool`); none for any other word.
fn match_arg_type(word: &str) -> Option<osd::ArgType> {
    match word {
        "int" => Some(osd::ArgType::Int),
        "float" => Some(osd::ArgType::Float),
        "string" => Some(osd::ArgType::String),
        "bool" => Some(osd::ArgType::Bool),
        _ => None,
    }
}

pub fn send_osc_to_target(sock: &UdpSocket, target: SocketAddr, msg: &OscMessage) {
    match rosc::encoder::encode(&OscPacket::Message(msg.clone())) {
        Ok(bytes) => { let _ = sock.send_to(&bytes, target); }
        Err(e) => eprintln!("OSC encode err for {}: {e}", msg.addr),
    }
}

pub fn run_osc_reply_loop(sock: UdpSocket, dev: Arc<Device>, send_osc: SendOsc) {
    let mut buf = [0u8; 65536];
    loop {
        let (n, _from) = match sock.recv_from(&mut buf) {
            Ok(p) => p,
            Err(e) => { eprintln!("OSC reply recv: {e}"); continue; }
        };
        match rosc::decoder::decode_udp(&buf[..n]) {
            Ok((_, pkt)) => handle_osc_reply_packet(pkt, &dev, &send_osc),
            Err(e) => eprintln!("OSC reply decode err: {e}"),
        }
    }
}

fn handle_osc_reply_packet(pkt: OscPacket, dev: &Device, send_osc: &SendOsc) {
    match pkt {
        OscPacket::Message(m) => {
            // Passthrough: any incoming OSC re-emitted as
            // <osc_prefix>/<incoming_addr> to the bridge's clients.
            if let Some(pt) = dev.device.transport.as_ref().and_then(|t| t.passthrough_prefix.as_deref()) {
                let stripped = if pt.is_empty() {
                    m.addr.clone()
                } else {
                    m.addr.strip_prefix(pt).unwrap_or(&m.addr).to_string()
                };
                let addr = format!("{}{}", dev.device.osc_prefix,
                    if stripped.starts_with('/') { stripped } else { format!("/{stripped}") });
                send_osc(OscMessage { addr, args: m.args });
                return;
            }
            for r in dev.replies.iter().flatten() {
                let Some(match_addr) = r.match_osc.as_ref().map(address_text) else { continue };
                let parts = parse_osc_tpl(&match_addr);
                let Some(mut caps) = match_osc_path(&parts, &m.addr) else { continue };
                let match_args = r.match_args.as_deref().unwrap_or_default();
                if m.args.len() < match_args.len() { continue }
                let mut typed: HashMap<String, OscType> = HashMap::new();
                let mut ok = true;
                for (i, ap) in match_args.iter().enumerate() {
                    let Some(ty) = match_arg_type(&ap.ty) else { ok = false; break };
                    let v = coerce_osc_to_type(&m.args[i], ty);
                    caps.insert(ap.name.clone(), osc_to_string(&v));
                    typed.insert(ap.name.clone(), v);
                    if !osc_type_matches(&m.args[i], ty) { ok = false; break; }
                }
                if !ok { continue }
                // Also expose path captures as typed bindings (best-effort: as strings).
                for (k, v) in &caps {
                    typed.entry(k.clone()).or_insert_with(|| OscType::String(v.clone()));
                }
                let out_msg = emit_osc_reply_typed(&r.emit_osc, r.emit_args.as_deref(), &dev.device.osc_prefix, &typed);
                send_osc(out_msg);
                return;
            }
        }
        OscPacket::Bundle(b) => {
            for p in b.content { handle_osc_reply_packet(p, dev, send_osc); }
        }
    }
}

fn osc_type_matches(v: &OscType, ty: osd::ArgType) -> bool {
    use osd::ArgType;
    matches!(
        (v, ty),
        (OscType::Int(_), ArgType::Int) | (OscType::Long(_), ArgType::Int)
        | (OscType::Float(_), ArgType::Float) | (OscType::Double(_), ArgType::Float)
        | (OscType::Bool(_), ArgType::Bool)
        | (OscType::String(_), ArgType::String)
    )
}

/// Twin of `emit_reply_osc` for OSC-mode replies: bindings carry typed
/// `OscType` values rather than u7/u14 bytes. The address's captures and the
/// `emit_args` capture from the bindings; literal arguments pass as written.
fn emit_osc_reply_typed(
    tpl: &Address,
    emit_args: Option<&[Scalar]>,
    prefix: &str,
    bindings: &HashMap<String, OscType>,
) -> OscMessage {
    let (tpl, emit_args) = reply_parts(tpl, emit_args);
    // Interpolate the addr (may itself hold captures).
    let addr_interp = interp_address(&tpl, bindings).unwrap_or_else(|| address_text(&tpl));
    let addr = if addr_interp.starts_with(prefix) || !addr_interp.starts_with('/') {
        if addr_interp.starts_with('/') { addr_interp.clone() } else { format!("{prefix}/{addr_interp}") }
    } else {
        format!("{prefix}{addr_interp}")
    };
    let mut args = Vec::new();
    for a in &emit_args {
        match a {
            Scalar::Capture(name) | Scalar::Arg(name) => {
                if let Some(v) = bindings.get(name) { args.push(v.clone()); }
            }
            Scalar::Number(n) => match n.as_i64() {
                Some(i) => args.push(OscType::Int(i as i32)),
                None => args.push(OscType::Float(n.as_f64().unwrap_or(0.0) as f32)),
            },
            Scalar::Bool(b) => args.push(OscType::Bool(*b)),
            Scalar::String(s) => args.push(OscType::String(s.clone())),
        }
    }
    OscMessage { addr, args }
}

#[cfg(test)]
mod tests;
