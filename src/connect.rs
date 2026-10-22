//! `osc-bridge connect`: a relay door on this machine's MIDI ports.
//!
//! The command-line twin of the site's `/connect` page, for Safari users,
//! servers without a browser, and agents that run next to the hardware. It
//! opens a WebSocket to the hosted relay (protocol in `crate::relay`), shows
//! the pairing code, and moves bytes: `midi_out` frames to the named output
//! port, every message of every input to the server. Ports are rescanned
//! every few seconds, so a device plugged in later appears without restart.
//!
//! The account the server issues is kept in `<config>/osc-bridge/user`, so
//! the user stays the same from one run to the next.

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

pub const DEFAULT_RELAY: &str = "wss://mcp.osc-bridge.roomi-fields.com/relay";
const MCP: &str = "https://mcp.osc-bridge.roomi-fields.com";
const RESCAN: Duration = Duration::from_secs(3);
const READ_TIMEOUT: Duration = Duration::from_millis(5);

/// The client name of our own MIDI connections. Some systems (ALSA) list
/// each open connection as a port of its own; those are not devices.
const CLIENT: &str = "osc-bridge-connect";

fn is_ours(name: &str) -> bool {
    name.starts_with(CLIENT)
}

/// Where the account id is kept: `$XDG_CONFIG_HOME`, `~/.config` or
/// `%APPDATA%`, under `osc-bridge/user`.
fn account_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("osc-bridge").join("user"))
}

/// The open MIDI ports, by name, and the names last sent to the server.
#[derive(Default)]
struct Ports {
    outputs: HashMap<String, MidiOutputConnection>,
    inputs: Vec<MidiInputConnection<()>>,
    names: (Vec<String>, Vec<String>),
}

/// The names of the ports the system offers now.
fn port_names() -> Result<(Vec<String>, Vec<String>)> {
    let out = MidiOutput::new(CLIENT)?;
    let inp = MidiInput::new(CLIENT)?;
    let outs = out.ports().iter().filter_map(|p| out.port_name(p).ok()).filter(|n| !is_ours(n)).collect();
    let ins = inp.ports().iter().filter_map(|p| inp.port_name(p).ok()).filter(|n| !is_ours(n)).collect();
    Ok((outs, ins))
}

/// Open every port; each input forwards what it receives to `incoming`.
fn open_ports(incoming: &Sender<(String, Vec<u8>)>) -> Result<Ports> {
    let mut ports = Ports::default();
    let probe = MidiOutput::new(CLIENT)?;
    for port in probe.ports() {
        let Ok(name) = probe.port_name(&port) else { continue };
        if is_ours(&name) || ports.outputs.contains_key(&name) { continue; }
        let out = MidiOutput::new(CLIENT)?;
        match out.connect(&port, CLIENT) {
            Ok(conn) => { ports.outputs.insert(name.clone(), conn); ports.names.0.push(name); }
            Err(e) => eprintln!("  output {name}: {e}"),
        }
    }
    let probe = MidiInput::new(CLIENT)?;
    for port in probe.ports() {
        let Ok(name) = probe.port_name(&port) else { continue };
        if is_ours(&name) { continue; }
        let mut inp = MidiInput::new(CLIENT)?;
        inp.ignore(Ignore::None);
        let (tx, label) = (incoming.clone(), name.clone());
        match inp.connect(&port, CLIENT, move |_, msg, _| {
            if msg.first() != Some(&0xF8) && msg.first() != Some(&0xFE) {
                let _ = tx.send((label.clone(), msg.to_vec()));
            }
        }, ()) {
            Ok(conn) => { ports.inputs.push(conn); ports.names.1.push(name); }
            Err(e) => eprintln!("  input {name}: {e}"),
        }
    }
    Ok(ports)
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn set_read_timeout(ws: &WebSocket<MaybeTlsStream<TcpStream>>, t: Duration) -> Result<()> {
    match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s.set_read_timeout(Some(t))?,
        MaybeTlsStream::Rustls(s) => s.get_ref().set_read_timeout(Some(t))?,
        _ => return Err(anyhow!("unsupported relay stream")),
    }
    Ok(())
}

/// Connect this machine's MIDI ports to the relay at `server` until the
/// connection closes or the process is stopped.
pub fn run(server: &str, share: bool) -> Result<()> {
    // One TLS crypto provider for the process (several may be compiled in).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let file = account_file();
    let known = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| s.trim().to_string());
    let url = match &known {
        Some(id) if !id.is_empty() => format!("{server}?user={id}"),
        _ => server.to_string(),
    };
    let (mut ws, _) = tungstenite::connect(url.as_str()).with_context(|| format!("connect to {server}"))?;
    let hello: Value = loop {
        match ws.read()? {
            Message::Text(t) => break serde_json::from_str(&t)?,
            Message::Close(_) => return Err(anyhow!("the relay closed the connection")),
            _ => {}
        }
    };
    if hello["type"] == "error" {
        return Err(anyhow!("relay: {}", hello["error"]));
    }
    let (code, user) = (hello["code"].as_str().unwrap_or("?"), hello["user"].as_str().unwrap_or(""));
    if let Some(f) = &file {
        if known.as_deref() != Some(user) {
            if let Some(dir) = f.parent() { let _ = std::fs::create_dir_all(dir); }
            let _ = std::fs::write(f, user);
        }
    }
    println!("Relay code: {code}");
    println!();
    println!("1. In your AI assistant, add this remote MCP server once:");
    println!("   {MCP}/u/{user}/mcp");
    println!("2. Tell the agent: \"My osc-bridge relay code is {code}\"");
    println!("3. Keep this running while you play (Ctrl-C to stop).");
    println!();
    ws.send(Message::Text(json!({ "type": "share", "enabled": share }).to_string().into()))?;
    set_read_timeout(&ws, READ_TIMEOUT)?;

    let (in_tx, in_rx): (Sender<(String, Vec<u8>)>, Receiver<(String, Vec<u8>)>) = unbounded();
    let mut ports = open_ports(&in_tx)?;
    let mut announce = true;
    let mut last_scan = Instant::now();
    loop {
        if last_scan.elapsed() >= RESCAN {
            last_scan = Instant::now();
            if let Ok(names) = port_names() {
                let (mut o, mut i) = names;
                let (mut po, mut pi) = ports.names.clone();
                o.sort(); i.sort(); po.sort(); pi.sort();
                if (o, i) != (po, pi) {
                    drop(ports);
                    ports = open_ports(&in_tx)?;
                    announce = true;
                }
            }
        }
        if announce {
            announce = false;
            println!("Devices: outputs [{}], inputs [{}]", ports.names.0.join(", "), ports.names.1.join(", "));
            ws.send(Message::Text(json!({ "type": "ports", "outputs": ports.names.0, "inputs": ports.names.1 }).to_string().into()))?;
        }
        while let Ok((port, data)) = in_rx.try_recv() {
            println!("  <- {port}  {}", hex(&data));
            ws.send(Message::Text(json!({ "type": "midi_in", "port": port, "data": data }).to_string().into()))?;
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                match v["type"].as_str() {
                    Some("midi_out") => {
                        let port = v["port"].as_str().unwrap_or("");
                        let data: Vec<u8> = v["data"].as_array().map(|a| a.iter().filter_map(Value::as_u64).map(|b| b as u8).collect()).unwrap_or_default();
                        match ports.outputs.get_mut(port) {
                            Some(conn) => match conn.send(&data) {
                                Ok(()) => println!("  -> {port}  {}", hex(&data)),
                                Err(e) => println!("  x  {port}  {}  ({e})", hex(&data)),
                            },
                            None => println!("  x  {port} (no such output)  {}", hex(&data)),
                        }
                    }
                    Some("paired") => println!("An agent is connected. Only it can use this code; restart for a new one."),
                    Some("error") => println!("relay error: {}", v["error"]),
                    _ => {}
                }
            }
            Ok(Message::Close(_)) => { println!("The relay closed the session."); return Ok(()); }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                println!("The relay closed the session.");
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }
    }
}
