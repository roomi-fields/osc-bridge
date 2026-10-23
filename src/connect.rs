//! `osc-bridge connect`: a relay door on this machine's MIDI ports.
//!
//! The command-line twin of the site's `/connect` page, for Safari users,
//! servers without a browser, and agents that run next to the hardware. It
//! opens a WebSocket to the hosted relay (protocol in `crate::relay_protocol`), shows
//! the pairing code, and moves bytes: `midi_out` frames to the named output
//! port, every message of every input to the server. Ports are rescanned
//! every few seconds, so a device plugged in later appears without restart.
//!
//! The account the server issues is kept in `<config>/osc-bridge/user`, so
//! the user stays the same from one run to the next.
//!
//! A message that may overwrite the device's memory (any SysEx but the
//! Identity Request) arrives as a `confirm` request: the door asks in the
//! terminal and sends it only if the person accepts.
//!
//! With `--test-synth`, the door also offers a virtual device, the catalogue's
//! `examples/test_synth` driver (`TestSynth`), so everything can be tried
//! without hardware.

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{unbounded, Sender};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

pub const DEFAULT_RELAY: &str = "wss://mcp.synthhub.roomi-fields.com/relay";
const RESCAN: Duration = Duration::from_secs(3);
const READ_TIMEOUT: Duration = Duration::from_millis(5);

/// The client name of our own MIDI connections. Some systems (ALSA) list
/// each open connection as a port of its own; those are not devices.
const CLIENT: &str = "osc-bridge-connect";

fn is_ours(name: &str) -> bool {
    name.starts_with(CLIENT)
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

/// The port name of the virtual test synth.
pub const TEST_SYNTH_PORT: &str = "osc-bridge Test Synth";

/// A virtual synth for tests: answers the MIDI Identity Request as
/// manufacturer 7D (reserved for non-commercial use), family 1, model 1,
/// firmware 1.0, and sends every channel message it receives back, as if the
/// control had been moved on the device. It keeps one sound — cutoff,
/// resonance, attack, release, waveform, set by their CC — which it sends as
/// a patch dump when asked (`DUMP_REQUEST`) and replaces when it receives a
/// patch (`PATCH` followed by the five values), like a real synth's edit
/// buffer.
pub struct TestSynth {
    sound: [u8; 5],
}

impl Default for TestSynth {
    fn default() -> Self {
        Self { sound: [64, 0, 0, 40, 0] }
    }
}

impl TestSynth {
    pub const IDENTITY_REPLY: [u8; 15] = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x7D, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0xF7];
    pub const DUMP_REQUEST: [u8; 5] = [0xF0, 0x7D, 0x01, 0x10, 0xF7];
    /// The patch message's header; the five values and F7 follow.
    pub const PATCH: [u8; 4] = [0xF0, 0x7D, 0x01, 0x20];
    /// The CC of each value of the sound, in order.
    const SOUND_CC: [u8; 5] = [74, 71, 73, 72, 70];

    /// What the synth sends in answer to `msg`.
    pub fn answer(&mut self, msg: &[u8]) -> Option<Vec<u8>> {
        match msg.first()? {
            0xF0 if msg == crate::detect::IDENTITY_REQUEST => Some(Self::IDENTITY_REPLY.to_vec()),
            0xF0 if msg == Self::DUMP_REQUEST => Some([&Self::PATCH[..], &self.sound, &[0xF7]].concat()),
            0xF0 if msg.len() == 10 && msg.starts_with(&Self::PATCH) && msg[9] == 0xF7 => {
                self.sound.copy_from_slice(&msg[4..9]);
                None
            }
            0xB0..=0xBF if msg.len() == 3 => {
                if let Some(i) = Self::SOUND_CC.iter().position(|&cc| cc == msg[1]) {
                    self.sound[i] = msg[2];
                }
                Some(msg.to_vec())
            }
            0x80..=0xEF => Some(msg.to_vec()),
            _ => None,
        }
    }
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// Play one message on `port`: a real output, or the test synth, whose
/// answer goes back as if received.
fn deliver(ports: &mut Ports, incoming: &Sender<(String, Vec<u8>)>, synth: &mut Option<TestSynth>, port: &str, data: &[u8]) {
    if let Some(synth) = synth.as_mut().filter(|_| port == TEST_SYNTH_PORT) {
        println!("  -> {port}  {}", hex(data));
        if let Some(reply) = synth.answer(data) {
            let _ = incoming.send((TEST_SYNTH_PORT.to_string(), reply));
        }
        return;
    }
    match ports.outputs.get_mut(port) {
        Some(conn) => match conn.send(data) {
            Ok(()) => println!("  -> {port}  {}", hex(data)),
            Err(e) => println!("  x  {port}  {}  ({e})", hex(data)),
        },
        None => println!("  x  {port} (no such output)  {}", hex(data)),
    }
}

/// The person's answers, one line each; `None` once the terminal is closed
/// (nobody left to ask: every request is refused).
fn answers() -> crossbeam_channel::Receiver<Option<String>> {
    let (tx, rx) = unbounded();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            if tx.send(line.ok()).is_err() { return; }
        }
        let _ = tx.send(None);
    });
    rx
}

/// The port names that contain one of the `--private-port` fragments.
fn private_port_names<'a>(names: impl Iterator<Item = &'a String>, fragments: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names.filter(|n| fragments.iter().any(|f| n.to_lowercase().contains(&f.to_lowercase())))
        .cloned().collect();
    out.sort();
    out.dedup();
    out
}

fn bytes_of(v: &Value) -> Vec<u8> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_u64).map(|b| b as u8).collect()).unwrap_or_default()
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
pub fn run(server: &str, share: bool, test_synth: bool, private_ports: &[String]) -> Result<()> {
    // One TLS crypto provider for the process (several may be compiled in).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let file = crate::online::account_file();
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
    println!("   {}/u/{user}/mcp", crate::online::SERVER);
    println!("2. Tell the agent: \"My osc-bridge relay code is {code}\"");
    println!("3. Keep this running while you play (Ctrl-C to stop).");
    println!();
    set_read_timeout(&ws, READ_TIMEOUT)?;

    let (in_tx, in_rx) = unbounded::<(String, Vec<u8>)>();
    let mut ports = open_ports(&in_tx)?;
    let typed = answers();
    let mut asking: VecDeque<Value> = VecDeque::new();
    let mut asked = false;
    let (mut always, mut nobody) = (false, false);
    let mut announce = true;
    let mut synth = test_synth.then(TestSynth::default);
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
            let (mut outs, mut ins) = ports.names.clone();
            if test_synth {
                outs.push(TEST_SYNTH_PORT.into());
                ins.push(TEST_SYNTH_PORT.into());
            }
            println!("Devices: outputs [{}], inputs [{}]", outs.join(", "), ins.join(", "));
            ws.send(Message::Text(json!({ "type": "ports", "outputs": outs, "inputs": ins }).to_string().into()))?;
            let private = private_port_names(outs.iter().chain(&ins), private_ports);
            if !private.is_empty() {
                println!("Private (identity never recorded): {}", private.join(", "));
            }
            ws.send(Message::Text(json!({ "type": "share", "enabled": share, "private": private }).to_string().into()))?;
        }
        while let Ok(line) = typed.try_recv() {
            let word = line.as_deref().map(|l| l.trim().to_lowercase());
            if word.is_none() { nobody = true; }
            if !asked { continue; }
            let accepted = matches!(word.as_deref(), Some("y" | "yes" | "a" | "always"));
            always |= matches!(word.as_deref(), Some("a" | "always"));
            if let Some(req) = asking.pop_front() {
                asked = false;
                answer(&mut ws, &mut ports, &in_tx, &mut synth, &req, accepted)?;
            }
        }
        if let Some(req) = asking.front() {
            if always || nobody {
                let req = asking.pop_front().expect("front");
                if nobody && !always { println!("  x  refused (no terminal to ask)"); }
                answer(&mut ws, &mut ports, &in_tx, &mut synth, &req, always)?;
            } else if !asked {
                asked = true;
                let n = req["messages"].as_array().map_or(0, Vec::len);
                println!();
                println!("Your agent wants to send {n} SysEx message(s) ({} bytes) to {}.", req["bytes"], req["port"].as_str().unwrap_or("?"));
                println!("This can replace sounds or settings stored in the device.");
                println!("Accept? [y]es / [n]o / [a]lways until this program stops");
            }
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
                        let data = bytes_of(&v["data"]);
                        if crate::relay_protocol::needs_confirmation(&data) {
                            println!("  x  {port}  {}  (not confirmed)", hex(&data));
                            continue;
                        }
                        deliver(&mut ports, &in_tx, &mut synth, port, &data);
                    }
                    Some("confirm") => asking.push_back(v),
                    Some("confirm_cancel") => {
                        if let Some(i) = asking.iter().position(|r| r["id"] == v["id"]) {
                            asking.remove(i);
                            if i == 0 && asked { asked = false; println!("  (the agent withdrew its request)"); }
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

/// Send an accepted batch, spaced by its `gap_ms`, then tell the server.
fn answer(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, ports: &mut Ports, incoming: &Sender<(String, Vec<u8>)>,
          synth: &mut Option<TestSynth>, req: &Value, accepted: bool) -> Result<()> {
    if accepted {
        let port = req["port"].as_str().unwrap_or("");
        let gap = Duration::from_millis(req["gap_ms"].as_u64().unwrap_or(0).min(2000));
        let messages = req["messages"].as_array().cloned().unwrap_or_default();
        for (i, m) in messages.iter().enumerate() {
            if i > 0 { std::thread::sleep(gap); }
            deliver(ports, incoming, synth, port, &bytes_of(m));
        }
    } else {
        println!("  x  refused: nothing sent");
    }
    ws.send(Message::Text(json!({ "type": "confirm_reply", "id": req["id"], "accepted": accepted }).to_string().into()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_synth_identifies_itself_and_echoes_what_it_receives() {
        let mut synth = TestSynth::default();
        let id = crate::detect::parse_identity_reply(&synth.answer(&crate::detect::IDENTITY_REQUEST).unwrap()).unwrap();
        assert_eq!((id.manufacturer, id.family, id.model, id.version), (vec![0x7D], 1, 1, [1, 0, 0, 0]));
        assert_eq!(synth.answer(&[0xB0, 74, 64]), Some(vec![0xB0, 74, 64]));
        assert_eq!(synth.answer(&[0xF8]), None);
    }

    #[test]
    fn the_test_synth_dumps_and_takes_its_sound_like_an_edit_buffer() {
        let mut synth = TestSynth::default();
        synth.answer(&[0xB0, 74, 90]);
        assert_eq!(synth.answer(&TestSynth::DUMP_REQUEST), Some(vec![0xF0, 0x7D, 0x01, 0x20, 90, 0, 0, 40, 0, 0xF7]));
        assert_eq!(synth.answer(&[0xF0, 0x7D, 0x01, 0x20, 10, 20, 30, 40, 2, 0xF7]), None);
        assert_eq!(synth.answer(&TestSynth::DUMP_REQUEST), Some(vec![0xF0, 0x7D, 0x01, 0x20, 10, 20, 30, 40, 2, 0xF7]));
    }

    #[test]
    fn private_ports_are_matched_by_a_fragment_of_their_name() {
        let names = ["minilogue SOUND".to_string(), "MiniLab3 MIDI".into(), "minilogue SOUND".into()];
        assert_eq!(private_port_names(names.iter(), &["MINILOGUE".into()]), ["minilogue SOUND"]);
        assert!(private_port_names(names.iter(), &[]).is_empty());
    }
}
