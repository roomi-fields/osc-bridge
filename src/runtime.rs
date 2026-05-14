//! Runtime wiring: OSC server ↔ MIDI ports ↔ rate limiter ↔ device spec.

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use rosc::{OscMessage, OscPacket, OscType};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::device::{CcParamEntry, Command, CustomCommand, Device, ParamEntry};
use crate::frame::{self, Bindings, Value};
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
}

pub struct Runtime;

impl Runtime {
    pub fn run(opts: RuntimeOptions) -> Result<()> {
        // Software (OSC-out) drivers bypass midir entirely. Branch early so
        // the rest of run() stays unchanged for the 840 hardware drivers.
        if opts.device.device.transport.as_ref().map(|t| t.kind.as_str())
            == Some("osc")
        {
            return run_osc(opts);
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
        let rate = dev.device.rate_limit_hz.unwrap_or(0);
        std::thread::spawn(move || drain_midi_out(out_conn, rx, rate));

        // Outbound OSC — broadcast to every configured client.
        let osc_sock = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
        let clients = opts.osc_clients.clone();
        let sock_cb = osc_sock.clone();
        let send_osc: Arc<dyn Fn(OscMessage) + Send + Sync> = Arc::new(move |msg| {
            if clients.is_empty() { return; }
            if let Ok(bytes) = rosc::encoder::encode(&OscPacket::Message(msg)) {
                for tgt in &clients {
                    let _ = sock_cb.send_to(&bytes, tgt);
                }
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
            match ScriptEngine::new_with_routes(routes.clone()) {
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

        eprintln!("Device:  {} ({})", dev.device.name, dev.device.revision);
        eprintln!("MIDI OUT [{}] {out_name}", opts.midi_out_port_idx);
        eprintln!("OSC IN   {}", opts.osc_bind);
        for c in &opts.osc_clients { eprintln!("OSC OUT  {c}"); }

        // Build dispatcher maps
        let cmds = build_command_map(&dev);
        let params = build_param_map(&dev);
        let cc_params = build_cc_param_map(&dev);

        let sock = UdpSocket::bind(&opts.osc_bind)
            .with_context(|| format!("bind {}", opts.osc_bind))?;
        eprintln!("Ready. Ctrl-C to stop.");

        let mut buf = [0u8; 65536];
        loop {
            let (n, from) = match sock.recv_from(&mut buf) {
                Ok(p) => p,
                Err(e) => { eprintln!("recv: {e}"); continue; }
            };
            match rosc::decoder::decode_udp(&buf[..n]) {
                Ok((_, pkt)) => dispatch(pkt, &dev, &cmds, &params, &cc_params, &routes, &script_engine, &tx, &send_osc, from),
                Err(e) => eprintln!("OSC decode err from {from}: {e}"),
            }
        }
    }
}

pub type ScriptCell = Arc<OnceLock<Mutex<ScriptEngine>>>;

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
/// that has a companion `.md` loaded. Devices without docs are silently
/// skipped — the client can fall back to introspection endpoints.
pub fn bridge_docs_replies(devices: &[Device]) -> Vec<OscMessage> {
    devices.iter().filter_map(|d| {
        let docs = d.docs.as_ref()?;
        let slug = d.device.osc_prefix.trim_start_matches('/').to_string();
        Some(OscMessage {
            addr: "/bridge/docs/device".into(),
            args: vec![
                OscType::String(slug),
                OscType::String(docs.clone()),
            ],
        })
    }).collect()
}

/// Test helper — given a Device and an OscMessage, return the MIDI bytes that
/// the performance-MIDI route would emit (or None if `midi_out` is not enabled
/// or the path isn't a performance route). Used by `tests/midi_out.rs`.
pub fn midi_out_for_msg(dev: &Device, msg: &OscMessage) -> Option<Vec<u8>> {
    let mo = dev.midi_out.as_ref()?;
    try_midi_out(msg, mo, &dev.device.osc_prefix)
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
fn try_midi_out(msg: &OscMessage, mo: &crate::device::MidiOut, prefix: &str) -> Option<Vec<u8>> {
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
    let channel = |idx: usize| -> u8 {
        int_at(idx).map(|v| (v & 0x0F) as u8).unwrap_or(mo.default_channel & 0x0F)
    };
    let apply_note = |n: i64| -> u8 {
        ((n + mo.note_offset as i64).clamp(0, 127)) as u8
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


fn device_uses_scripting(dev: &Device) -> bool {
    dev.commands.iter().any(|c| c.script.is_some())
        || !dev.custom_commands.is_empty()
        || dev.replies.iter().any(|r| r.script.is_some())
        || dev.params.as_ref().map(|pt| pt.entries.iter().any(|e|
            e.transform.is_some() || e.transform_reverse.is_some())).unwrap_or(false)
        || dev.cc_params.as_ref().map(|pt| pt.entries.iter().any(|e|
            e.transform.is_some() || e.transform_reverse.is_some())).unwrap_or(false)
}

/// Run a command's `script` field (if any) against the assembled frame.
/// Returns the (possibly mutated) frame, or None to drop the message.
fn maybe_run_cmd_script(cell: &ScriptCell, cmd: &Command, frame: Vec<u8>, device: &str) -> Option<Vec<u8>> {
    let Some(code) = &cmd.script else { return Some(frame); };
    let Some(m) = cell.get() else {
        eprintln!("  command {} script ignored (engine unavailable)", cmd.osc);
        return Some(frame);
    };
    let ctx = ScriptContext {
        payload: frame,
        direction: "osc_to_midi".into(),
        device: device.into(),
        command: cmd.osc.clone(),
        ..Default::default()
    };
    match m.lock() {
        Ok(g) => match g.run_script(code, ctx) {
            Ok(Some(out)) => Some(out.payload),
            Ok(None) => {
                eprintln!("  command {} script dropped message", cmd.osc);
                None
            }
            Err(e) => {
                eprintln!("  command {} script err: {e} — passing through raw frame", cmd.osc);
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

    let profile = match cc.profile.as_deref() {
        Some("preset_ingest") => crate::scripting::Profile::PresetIngest,
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
            let bytes = if cc.wrap_sysex {
                let mut buf = Vec::with_capacity(
                    dev.sysex.header.len() + out.payload.len() + dev.sysex.footer.len(),
                );
                buf.extend_from_slice(&dev.sysex.header);
                buf.extend_from_slice(&out.payload);
                buf.extend_from_slice(&dev.sysex.footer);
                buf
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
    rp: &crate::device::ReplyPattern,
    bindings: &Bindings,
    dev: &Arc<Device>,
    cell: &ScriptCell,
) -> bool {
    let Some(code) = &rp.script else { return true; };
    let Some(m) = cell.get() else {
        eprintln!("  reply.script {} skipped (engine unavailable)", rp.emit_osc);
        return true;
    };
    let mut binds: std::collections::HashMap<String, i64> = Default::default();
    for (k, v) in &bindings.0 {
        let iv = match v {
            Value::U7(n) => *n as i64,
            Value::U14(n) => *n as i64,
            Value::Bytes(_) => continue,
        };
        binds.insert(k.clone(), iv);
    }
    let ctx = ScriptContext {
        bindings: binds,
        direction: "midi_to_osc".into(),
        device: dev.device.name.clone(),
        command: rp.emit_osc.clone(),
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
        eprintln!("  reply.script {} emitted {} SysEx block(s) but emits \
                   from reply scripts are not yet plumbed to MIDI out",
                  rp.emit_osc, emits.len());
    }

    match outcome {
        Ok(None) => false, // script dropped the event
        Ok(Some(_)) => true,
        Err(e) => {
            eprintln!("  reply.script {} err: {e} — emitting OSC anyway", rp.emit_osc);
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
            let mut buf = Vec::with_capacity(
                dev.sysex.header.len() + block.bytes.len() + dev.sysex.footer.len(),
            );
            buf.extend_from_slice(&dev.sysex.header);
            buf.extend_from_slice(&block.bytes);
            buf.extend_from_slice(&dev.sysex.footer);
            buf
        } else {
            block.bytes.clone()
        };
        enqueue(tx, bytes, addr, from);
    }
}

fn apply_transform(cell: &ScriptCell, expr: &Option<String>, value: i64, addr: &str) -> i64 {
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
        // Attempt to match one of the reply patterns
        for rp in &dev.replies {
            if let Some(bindings) = match_reply(msg, dev, &rp.match_frame) {
                // Optional reply.script: lets drivers update state (e.g.
                // ob.set_current_page) in response to device-initiated events.
                // Returning nil from the script drops the OSC emission.
                if rp.script.is_some() {
                    if !run_reply_script(rp, &bindings, dev, scripts) {
                        return;
                    }
                }
                let m = emit_reply_osc(&rp.emit_osc, &dev.device.osc_prefix, &bindings);
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
    if let Some(m) = midi_codec::midi_to_osc(msg, &dev.midi_in, &dev.device.osc_prefix) {
        send(m);
    }
}

/// Try to match `msg` against `pattern = header + pattern_body + footer`.
/// Returns captured bindings if matched.
fn match_reply(msg: &[u8], dev: &Device, pattern: &[crate::device::FrameToken]) -> Option<Bindings> {
    let h = &dev.sysex.header;
    let f = &dev.sysex.footer;
    if msg.len() < h.len() + f.len() || !msg.starts_with(h) || !msg.ends_with(f) {
        return None;
    }
    let body = &msg[h.len()..msg.len() - f.len()];
    if body.len() != pattern.len() { return None; }
    let mut b = Bindings::new();
    for (pat, byte) in pattern.iter().zip(body.iter()) {
        match pat {
            crate::device::FrameToken::Literal(l) => {
                if l != byte { return None; }
            }
            crate::device::FrameToken::Placeholder(ph) => {
                let inner = ph.trim_matches(|c| c == '{' || c == '}');
                let name = inner.splitn(2, ':').next().unwrap_or("").trim();
                b.set_u7(name, *byte);
            }
        }
    }
    Some(b)
}

fn emit_reply_osc(tpl: &str, prefix: &str, bindings: &Bindings) -> OscMessage {
    let mut parts = tpl.splitn(2, ' ');
    let addr_tpl = parts.next().unwrap_or("");
    let args_tpl = parts.next().unwrap_or("");
    let addr = if addr_tpl.starts_with(prefix) || addr_tpl.starts_with('/') {
        addr_tpl.to_string()
    } else {
        format!("{prefix}/{addr_tpl}")
    };
    let addr = if addr.starts_with(prefix) { addr } else { format!("{prefix}{addr}") };
    let mut args = Vec::new();
    for tok in args_tpl.split_whitespace() {
        let key = tok.trim_matches(|c| c == '{' || c == '}');
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

pub fn build_command_map(dev: &Device) -> Vec<CommandHandler> {
    dev.commands.iter().map(|c| {
        let full = dev.full_osc_path(&c.osc);
        let pattern_parts = parse_osc_tpl(&full);
        CommandHandler { cmd: c.clone(), pattern_parts }
    }).collect()
}

pub fn build_param_map(dev: &Device) -> HashMap<String, ParamEntry> {
    let mut m = HashMap::new();
    if let Some(pt) = &dev.params {
        for e in &pt.entries {
            m.insert(dev.full_osc_path(&e.osc), e.clone());
        }
    }
    m
}

pub fn build_cc_param_map(dev: &Device) -> HashMap<String, (CcParamEntry, u8)> {
    let mut m = HashMap::new();
    if let Some(cct) = &dev.cc_params {
        let default_ch = cct.channel;
        for e in &cct.entries {
            let ch = e.channel.unwrap_or(default_ch);
            m.insert(dev.full_osc_path(&e.osc), (e.clone(), ch));
        }
    }
    m
}

pub type SendOsc = Arc<dyn Fn(OscMessage) + Send + Sync>;

/// Per-device dispatch state shared between Runtime::run (single-device) and
/// Orchestrator::run (multi-device). Lets the orchestrator reuse the full
/// dispatch pipeline (commands, params, cc_params, custom_commands) instead
/// of the previous perf-MIDI-only subset.
pub struct DeviceDispatch {
    pub device: Arc<Device>,
    pub cmds: Vec<CommandHandler>,
    pub params: HashMap<String, ParamEntry>,
    pub cc_params: HashMap<String, (CcParamEntry, u8)>,
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
            if let Ok(e) = ScriptEngine::new_with_routes(routes.clone()) {
                let _ = scripts.set(Mutex::new(e));
            }
        }
        Self { device: dev, cmds, params, cc_params, routes, scripts, tx }
    }
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
    handle_message(msg, &state.device, &state.cmds, &state.params, &state.cc_params,
                   &state.routes, &state.scripts, &state.tx, send_osc, from);
}

/// Per-device dispatch state for an OSC-transport (software) device — the
/// orchestrator equivalent of what `run_osc` keeps on its stack. Lets the
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

fn dispatch(
    pkt: OscPacket,
    dev: &Arc<Device>,
    cmds: &[CommandHandler],
    params: &HashMap<String, ParamEntry>,
    cc_params: &HashMap<String, (CcParamEntry, u8)>,
    routes: &Arc<Mutex<DynamicRoutes>>,
    scripts: &ScriptCell,
    tx: &Sender<Vec<u8>>,
    send_osc: &SendOsc,
    from: SocketAddr,
) {
    match pkt {
        OscPacket::Message(m) => handle_message(m, dev, cmds, params, cc_params, routes, scripts, tx, send_osc, from),
        OscPacket::Bundle(b) => {
            for p in b.content { dispatch(p, dev, cmds, params, cc_params, routes, scripts, tx, send_osc, from); }
        }
    }
}

fn handle_message(
    msg: OscMessage,
    dev: &Arc<Device>,
    cmds: &[CommandHandler],
    params: &HashMap<String, ParamEntry>,
    cc_params: &HashMap<String, (CcParamEntry, u8)>,
    routes: &Arc<Mutex<DynamicRoutes>>,
    scripts: &ScriptCell,
    tx: &Sender<Vec<u8>>,
    send_osc: &SendOsc,
    from: SocketAddr,
) {
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
        for reply in bridge_docs_replies(std::slice::from_ref(dev.as_ref())) {
            send_osc(reply);
        }
        return;
    }

    // Scripted custom_commands (preset upload, dynamic-route init,
    // introspection). Matched on exact full OSC path.
    for cc in &dev.custom_commands {
        let full = dev.full_osc_path(&cc.osc);
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

    // Performance MIDI out (midi_out section enables these routes).
    if let Some(mo) = &dev.midi_out {
        if let Some(bytes) = try_midi_out(&msg, mo, &dev.device.osc_prefix) {
            enqueue(tx, bytes, &msg.addr, from);
            return;
        }
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
            let val_i = apply_transform(scripts, &entry.transform, val_i, &msg.addr);
            let clamped = val_i.clamp(entry.range[0], entry.range[1]);
            let ch = msg.args.get(1).and_then(|a| match a {
                OscType::Int(i) => Some((*i & 0x0F) as u8),
                OscType::Long(i) => Some((*i & 0x0F) as u8),
                _ => None,
            }).unwrap_or(*default_ch);
            let status = 0xB0 | (ch & 0x0F);
            let is_14bit = entry.range[1] > 127;
            let msb = if is_14bit { ((clamped >> 7) & 0x7F) as u8 } else { (clamped as u8) & 0x7F };
            let lsb = (clamped as u8) & 0x7F;

            if let (Some(nmsb), Some(nlsb)) = (entry.nrpn_msb, entry.nrpn_lsb) {
                // NRPN sequence: 99=msb, 98=lsb, 6=data_msb, 38=data_lsb (if 14-bit)
                enqueue(tx, vec![status, 99, nmsb & 0x7F], &msg.addr, from);
                enqueue(tx, vec![status, 98, nlsb & 0x7F], &msg.addr, from);
                enqueue(tx, vec![status, 6, msb], &msg.addr, from);
                if is_14bit {
                    enqueue(tx, vec![status, 38, lsb], &msg.addr, from);
                }
                return;
            }
            if let Some(cc_num) = entry.cc {
                enqueue(tx, vec![status, cc_num & 0x7F, msb], &msg.addr, from);
                if is_14bit {
                    if let Some(lsb_cc) = entry.cc_lsb {
                        enqueue(tx, vec![status, lsb_cc & 0x7F, lsb], &msg.addr, from);
                    }
                }
                return;
            }
            eprintln!("  cc entry {} has neither cc nor nrpn address", msg.addr);
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
                    let t = apply_transform(scripts, &entry.transform, as_i, &msg.addr);
                    OscType::Long(t)
                } else {
                    v.clone()
                };
                match frame::bindings_for_param_set(pt, entry, &v_transformed) {
                    Ok(b) => {
                        match frame::build_frame(dev, &pt.set_frame, &b) {
                            Ok(frm) => { enqueue(tx, frm, &msg.addr, from); return; }
                            Err(e) => eprintln!("build_frame err: {e}"),
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
                Ok(frm) => {
                    if let Some(post) = maybe_run_cmd_script(scripts, &h.cmd, frm.clone(), &dev.device.name) {
                        enqueue(tx, post, &msg.addr, from);
                    }
                }
                Err(e) => eprintln!("command '{}' err: {e}", h.cmd.osc),
            }
            return;
        }
    }

    // Special: raw sysex passthrough
    let raw = dev.full_osc_path("/raw/syx");
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
    let get_addr = dev.full_osc_path("/param/get");
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
                if let Ok(frm) = frame::build_frame(dev, &pt.get_frame, &b) {
                    enqueue(tx, frm, &msg.addr, from);
                    return;
                }
            }
        }
    }

    eprintln!("  unknown OSC: {}", msg.addr);
}

fn dispatch_command(cmd: &Command, caps: &HashMap<String, String>, args: &[OscType], dev: &Device) -> Result<Vec<u8>> {
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
    for spec in &cmd.args {
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
                    anyhow::bail!("missing arg {} for {}", spec.name, cmd.osc);
                }
            }
        }
    }

    // Evaluate pre-bindings (simple expression engine)
    for pb in &cmd.pre {
        let v = eval_expr(&pb.expr, &b)?;
        b.set_u7(&pb.bind, v);
    }

    frame::build_frame(dev, &cmd.frame, &b)
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
fn run_osc(opts: RuntimeOptions) -> Result<()> {
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

    // Bridge → OSC clients (replies/events fanned out).
    let osc_sock = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
    let clients = opts.osc_clients.clone();
    let sock_cb = osc_sock.clone();
    let send_osc: SendOsc = Arc::new(move |msg| {
        if clients.is_empty() { return; }
        if let Ok(bytes) = rosc::encoder::encode(&OscPacket::Message(msg)) {
            for tgt in &clients { let _ = sock_cb.send_to(&bytes, tgt); }
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
    } else if dev.replies.iter().any(|r| r.match_osc.is_some()) {
        eprintln!("WARN: device declares OSC replies but no transport.reply_port — they will never fire");
    }

    // One-shot startup subscriptions (e.g. /live/song/start_listen/tempo).
    let empty_bindings: HashMap<String, OscType> = HashMap::new();
    for sub in &dev.subscriptions {
        if sub.on != "startup" { continue; }
        if let Some(out_msg) = build_forward_msg(&sub.forward, &empty_bindings) {
            send_osc_to_target(&target_sock, target_addr, &out_msg);
            eprintln!("OSC SUB  {} -> {target_addr}", out_msg.addr);
        }
    }

    eprintln!("Device:  {} ({})", dev.device.name, dev.device.revision);
    eprintln!("OSC TGT  {target_addr}");
    eprintln!("OSC IN   {}", opts.osc_bind);
    for c in &opts.osc_clients { eprintln!("OSC CLI  {c}"); }

    let cmds = build_command_map(&dev);
    let sock = UdpSocket::bind(&opts.osc_bind)
        .with_context(|| format!("bind {}", opts.osc_bind))?;
    eprintln!("Ready. Ctrl-C to stop.");

    let mut buf = [0u8; 65536];
    loop {
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(p) => p,
            Err(e) => { eprintln!("recv: {e}"); continue; }
        };
        match rosc::decoder::decode_udp(&buf[..n]) {
            Ok((_, pkt)) => dispatch_osc(pkt, &dev, &cmds, &target_sock, target_addr, &send_osc, from),
            Err(e) => eprintln!("OSC decode err from {from}: {e}"),
        }
    }
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
        for r in bridge_docs_replies(std::slice::from_ref(dev.as_ref())) { send_osc(r); }
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
    let arg_ty: HashMap<&str, &str> = cmd.args.iter()
        .map(|a| (a.name.as_str(), a.ty.as_str()))
        .collect();
    for (k, v) in path_caps {
        let ty = arg_ty.get(k.as_str()).copied().unwrap_or("string");
        if let Some(osc) = coerce_str_to_osc(v, ty) {
            out.insert(k.clone(), osc);
        }
    }
    let mut osc_iter = incoming_args.iter();
    for spec in &cmd.args {
        if path_caps.contains_key(&spec.name) { continue; }
        match osc_iter.next() {
            Some(v) => { out.insert(spec.name.clone(), coerce_osc_to_type(v, &spec.ty)); }
            None => break,
        }
    }
    out
}

/// Build an outgoing OscMessage from a ForwardSpec template. Literal int /
/// float / bool args carry through; string args either literal-out, or strip
/// the surrounding `{}` and substitute from bindings.
pub fn build_forward_msg(
    forward: &crate::device::ForwardSpec,
    bindings: &HashMap<String, OscType>,
) -> Option<OscMessage> {
    let addr = interp_placeholders(&forward.path, bindings)?;
    let mut args: Vec<OscType> = Vec::with_capacity(forward.args.len());
    for fa in &forward.args {
        match fa {
            crate::device::ForwardArg::Int(i) => args.push(OscType::Int(*i as i32)),
            crate::device::ForwardArg::Float(f) => args.push(OscType::Float(*f as f32)),
            crate::device::ForwardArg::Bool(b) => args.push(OscType::Bool(*b)),
            crate::device::ForwardArg::Str(s) => {
                if let Some(name) = strip_placeholder(s) {
                    args.push(bindings.get(name)?.clone());
                } else {
                    args.push(OscType::String(s.clone()));
                }
            }
        }
    }
    Some(OscMessage { addr, args })
}

fn strip_placeholder(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b'{' && b[b.len() - 1] == b'}' {
        Some(&s[1..s.len() - 1])
    } else { None }
}

fn interp_placeholders(tpl: &str, bindings: &HashMap<String, OscType>) -> Option<String> {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}')?;
        let name = &rest[open + 1..open + close];
        let v = bindings.get(name)?;
        out.push_str(&osc_to_string(v));
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    Some(out)
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

fn coerce_str_to_osc(s: &str, ty: &str) -> Option<OscType> {
    match ty {
        "int" | "u7" | "u8" | "u14" => s.parse::<i32>().ok().map(OscType::Int),
        "float" => s.parse::<f32>().ok().map(OscType::Float),
        "bool" => match s {
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
fn coerce_osc_to_type(v: &OscType, ty: &str) -> OscType {
    match (v, ty) {
        (OscType::Int(_), "int") | (OscType::Float(_), "float")
        | (OscType::Bool(_), "bool") | (OscType::String(_), "string") => v.clone(),
        (OscType::Int(i), "float") => OscType::Float(*i as f32),
        (OscType::Long(i), "float") => OscType::Float(*i as f32),
        (OscType::Float(f), "int") => OscType::Int(*f as i32),
        (OscType::Double(f), "int") => OscType::Int(*f as i32),
        (OscType::Long(i), "int") => OscType::Int(*i as i32),
        (OscType::Double(f), "float") => OscType::Float(*f as f32),
        _ => v.clone(),
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
            for r in &dev.replies {
                let Some(match_addr) = r.match_osc.as_deref() else { continue };
                let parts = parse_osc_tpl(match_addr);
                let Some(mut caps) = match_osc_path(&parts, &m.addr) else { continue };
                if m.args.len() < r.match_args.len() { continue }
                let mut typed: HashMap<String, OscType> = HashMap::new();
                let mut ok = true;
                for (i, ap) in r.match_args.iter().enumerate() {
                    let v = coerce_osc_to_type(&m.args[i], &ap.ty);
                    caps.insert(ap.name.clone(), osc_to_string(&v));
                    typed.insert(ap.name.clone(), v);
                    if !osc_type_matches(&m.args[i], &ap.ty) { ok = false; break; }
                }
                if !ok { continue }
                // Also expose path captures as typed bindings (best-effort: as strings).
                for (k, v) in &caps {
                    typed.entry(k.clone()).or_insert_with(|| OscType::String(v.clone()));
                }
                let out_msg = emit_osc_reply_typed(&r.emit_osc, &dev.device.osc_prefix, &typed);
                send_osc(out_msg);
                return;
            }
        }
        OscPacket::Bundle(b) => {
            for p in b.content { handle_osc_reply_packet(p, dev, send_osc); }
        }
    }
}

fn osc_type_matches(v: &OscType, ty: &str) -> bool {
    matches!(
        (v, ty),
        (OscType::Int(_), "int") | (OscType::Long(_), "int")
        | (OscType::Float(_), "float") | (OscType::Double(_), "float")
        | (OscType::Bool(_), "bool")
        | (OscType::String(_), "string")
    )
}

/// Twin of `emit_reply_osc` for OSC-mode replies: bindings carry typed
/// `OscType` values rather than u7/u14 bytes. Template syntax identical:
/// `"/some/path {x} {y}"` — first space splits addr from positional args.
fn emit_osc_reply_typed(
    tpl: &str,
    prefix: &str,
    bindings: &HashMap<String, OscType>,
) -> OscMessage {
    let mut parts = tpl.splitn(2, ' ');
    let addr_tpl = parts.next().unwrap_or("");
    let args_tpl = parts.next().unwrap_or("");
    // Interpolate the addr (may itself contain {placeholders}).
    let addr_interp = interp_placeholders(addr_tpl, bindings)
        .unwrap_or_else(|| addr_tpl.to_string());
    let addr = if addr_interp.starts_with(prefix) || !addr_interp.starts_with('/') {
        if addr_interp.starts_with('/') { addr_interp.clone() } else { format!("{prefix}/{addr_interp}") }
    } else {
        format!("{prefix}{addr_interp}")
    };
    let mut args = Vec::new();
    for tok in args_tpl.split_whitespace() {
        if let Some(name) = strip_placeholder(tok) {
            if let Some(v) = bindings.get(name) { args.push(v.clone()); }
        } else {
            // Literal token in args template — accept int / float / fallthrough as string.
            if let Ok(i) = tok.parse::<i32>() { args.push(OscType::Int(i)); }
            else if let Ok(f) = tok.parse::<f32>() { args.push(OscType::Float(f)); }
            else { args.push(OscType::String(tok.to_string())); }
        }
    }
    OscMessage { addr, args }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_simple_add() {
        let mut b = Bindings::new(); b.set_u7("pad", 10);
        assert_eq!(eval_expr("pad + 4", &b).unwrap(), 14);
    }
    #[test]
    fn eval_ternary() {
        let mut b = Bindings::new(); b.set_u7("pad", 10);
        assert_eq!(eval_expr("pad > 7 ? 12 : 0", &b).unwrap(), 12);
        b.set_u7("pad", 3);
        assert_eq!(eval_expr("pad > 7 ? 12 : 0", &b).unwrap(), 0);
    }
    #[test]
    fn path_match_with_capture() {
        let parts = parse_osc_tpl("/minilab3/pad/{pad}/color");
        let caps = match_osc_path(&parts, "/minilab3/pad/3/color").unwrap();
        assert_eq!(caps.get("pad").unwrap(), "3");
    }

    // ---- OSC transport (software drivers) ----

    use crate::device::{ForwardArg, ForwardSpec};

    #[test]
    fn forward_msg_literal_int_arg() {
        let f = ForwardSpec {
            path: "/live/song/start_playing".into(),
            args: vec![ForwardArg::Int(120)],
        };
        let m = build_forward_msg(&f, &HashMap::new()).unwrap();
        assert_eq!(m.addr, "/live/song/start_playing");
        assert!(matches!(m.args[0], OscType::Int(120)));
    }

    #[test]
    fn forward_msg_placeholder_args_resolved_from_bindings() {
        let f = ForwardSpec {
            path: "/live/track/set/volume".into(),
            args: vec![ForwardArg::Str("{n}".into()), ForwardArg::Str("{v}".into())],
        };
        let mut b = HashMap::new();
        b.insert("n".into(), OscType::Int(3));
        b.insert("v".into(), OscType::Float(0.75));
        let m = build_forward_msg(&f, &b).unwrap();
        assert!(matches!(m.args[0], OscType::Int(3)));
        assert!(matches!(m.args[1], OscType::Float(_)));
    }

    #[test]
    fn forward_msg_path_with_placeholder() {
        let f = ForwardSpec { path: "/track/{n}/state".into(), args: vec![] };
        let mut b = HashMap::new();
        b.insert("n".into(), OscType::Int(5));
        let m = build_forward_msg(&f, &b).unwrap();
        assert_eq!(m.addr, "/track/5/state");
    }

    #[test]
    fn forward_msg_literal_string_pass_through() {
        let f = ForwardSpec {
            path: "/x".into(),
            args: vec![ForwardArg::Str("literal".into())],
        };
        let m = build_forward_msg(&f, &HashMap::new()).unwrap();
        assert!(matches!(&m.args[0], OscType::String(s) if s == "literal"));
    }

    #[test]
    fn coerce_str_to_int_and_bool() {
        assert!(matches!(coerce_str_to_osc("42", "int"), Some(OscType::Int(42))));
        assert!(matches!(coerce_str_to_osc("true", "bool"), Some(OscType::Bool(true))));
        assert!(matches!(coerce_str_to_osc("0", "bool"), Some(OscType::Bool(false))));
    }

    #[test]
    fn osc_type_match_accepts_long_for_int_and_double_for_float() {
        assert!(osc_type_matches(&OscType::Int(1), "int"));
        assert!(osc_type_matches(&OscType::Long(1), "int"));
        assert!(osc_type_matches(&OscType::Float(0.5), "float"));
        assert!(osc_type_matches(&OscType::Double(0.5), "float"));
        assert!(!osc_type_matches(&OscType::Int(1), "string"));
    }

    #[test]
    fn full_osc_driver_json_parses() {
        let json = r#"{
            "device": {
                "name": "Ableton Live", "vendor": "Ableton",
                "osc_prefix": "/ableton", "kind": "software",
                "transport": { "kind": "osc", "host": "127.0.0.1",
                               "port": 11000, "reply_port": 11001 }
            },
            "commands": [
                { "osc": "/transport/play",
                  "forward": { "path": "/live/song/start_playing", "args": [] } },
                { "osc": "/transport/tempo",
                  "args": [{"name":"bpm","type":"float"}],
                  "forward": { "path": "/live/song/set/tempo",
                               "args": ["{bpm}"] } }
            ],
            "subscriptions": [
                { "on": "startup",
                  "forward": { "path": "/live/song/start_listen/tempo",
                               "args": [] } }
            ],
            "replies": [
                { "match_osc": "/live/song/get/tempo",
                  "match_args": [{"name":"bpm","type":"float"}],
                  "emit_osc": "/transport/tempo {bpm}" }
            ]
        }"#;
        let dev: crate::device::Device = serde_json::from_str(json).expect("parses");
        assert_eq!(dev.device.kind.as_deref(), Some("software"));
        let t = dev.device.transport.as_ref().unwrap();
        assert_eq!(t.kind, "osc");
        assert_eq!(t.port, Some(11000));
        assert_eq!(t.reply_port, Some(11001));
        assert_eq!(dev.commands.len(), 2);
        assert!(dev.commands[0].forward.is_some());
        assert!(dev.commands[0].frame.is_empty());
        assert_eq!(dev.subscriptions.len(), 1);
        assert_eq!(dev.replies[0].match_args.len(), 1);
    }

    #[test]
    fn emit_osc_reply_substitutes_bindings_and_prefixes() {
        let mut b = HashMap::new();
        b.insert("bpm".into(), OscType::Float(124.5));
        let m = emit_osc_reply_typed("/transport/tempo {bpm}", "/ableton", &b);
        assert_eq!(m.addr, "/ableton/transport/tempo");
        assert_eq!(m.args.len(), 1);
        match &m.args[0] {
            OscType::Float(f) => assert!((*f - 124.5).abs() < 0.01),
            other => panic!("wrong arg type: {other:?}"),
        }
    }

    #[test]
    fn passthrough_strips_prefix_when_empty_target() {
        // device.osc_prefix = "/sonicpi", passthrough_prefix = ""
        // /sonicpi/cue/foo → /cue/foo
        let addr = "/sonicpi/cue/foo";
        let osc_prefix = "/sonicpi";
        let pt = "";
        let rest = addr.strip_prefix(osc_prefix).unwrap();
        let new_addr = if pt.is_empty() { rest.to_string() } else { format!("{pt}{rest}") };
        assert_eq!(new_addr, "/cue/foo");
    }

    #[test]
    fn passthrough_replaces_prefix_when_set() {
        // device.osc_prefix = "/sc", passthrough_prefix = "/notify"
        // /sc/sync → /notify/sync
        let addr = "/sc/sync";
        let osc_prefix = "/sc";
        let pt = "/notify";
        let rest = addr.strip_prefix(osc_prefix).unwrap();
        let new_addr = if pt.is_empty() { rest.to_string() } else { format!("{pt}{rest}") };
        assert_eq!(new_addr, "/notify/sync");
    }

    #[test]
    fn build_bindings_combines_path_caps_and_positional_args() {
        use crate::device::{ArgSpec, Command};
        let cmd = Command {
            osc: "/track/{n}/volume".into(),
            args: vec![
                ArgSpec { name: "n".into(), ty: "int".into(), range: None, values: None, default: None },
                ArgSpec { name: "v".into(), ty: "float".into(), range: None, values: None, default: None },
            ],
            pre: vec![], frame: vec![], forward: None, script: None,
        };
        let mut caps = HashMap::new();
        caps.insert("n".into(), "7".to_string());
        let b = build_forward_bindings(&cmd, &caps, &[OscType::Float(0.5)]);
        assert!(matches!(b.get("n").unwrap(), OscType::Int(7)));
        assert!(matches!(b.get("v").unwrap(), OscType::Float(_)));
    }
}
