//! The MIDI ports a command opens: the type the bridge puts MIDI through.
//!
//! `Midi` is how the ports are reached (the hardware, a relay, or a test synth), `Opener` what a
//! command takes its MIDI from, `guarded_open` the single guard of every opening of the MIDI
//! system, and `Hardware` the machine's own ports. `install`, `detect` and `mcp` read this module;
//! the link to the service reads it too, never the reverse.

use anyhow::{anyhow, Result};
use crate::detect::ClientKind;
use std::sync::{Arc, Mutex};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// How the MIDI ports are reached: the hardware, a relay, or a test synth.
pub trait Midi {
    /// The output ports, by name.
    fn outputs(&self) -> Vec<String>;
    /// Send `messages` on `port`, in one go: a relay asks its person to
    /// accept them all at once, told `why`.
    fn send(&mut self, port: &str, messages: &[Vec<u8>], why: &str) -> Result<(), String>;
    /// The SysEx messages received within `wait`, each with the output port
    /// of the device that sent it.
    fn received(&mut self, wait: Duration) -> Vec<(String, Vec<u8>)>;
    /// Whether every synth's detect messages may go to every port when the
    /// synths the ports' names point at did not answer. A relay asks before
    /// each port's batch, so there the user names the port instead.
    fn searches_everywhere(&self) -> bool { true }
    /// What a detection observes through these ports, raw: the ports, the system's listings of
    /// its cards and clients, and, when `probe` is true, the requests sent (the Identity Request,
    /// then each of `probes` to its ports, held back where `consent` is wanted) with the SysEx
    /// received for `wait`. The hardware reads the machine; a fake device carries its own listings.
    fn capture(&mut self, _probe: bool, _wait: Duration, _probes: &[crate::recognition::Probe], _consent: &[String]) -> Result<crate::capture::Capture> {
        Err(anyhow!("these MIDI ports cannot be observed"))
    }
    /// The wire of one device (its inputs listened to, its outputs asked) for the learn mode.
    fn open_wire(&mut self, _device: &Unit) -> Result<Box<dyn Wire + Send>> {
        Err(anyhow!("these MIDI ports cannot be listened to"))
    }
}

/// The MIDI ports of this machine: every output, and the input of the same
/// device (paired by name, as `detect_hardware` does) listened to.
pub struct Hardware {
    outputs: Vec<(String, MidiOutputConnection)>,
    _inputs: Vec<MidiInputConnection<()>>,
    incoming: Receiver<(String, Vec<u8>)>,
}

/// What a command takes its MIDI from: opens the MIDI system, or says why it cannot. Tests give
/// the shared fake hardware; the program gives `open_hardware`.
pub type Opener<'a> = &'a dyn Fn() -> Result<Box<dyn Midi>>;

/// The single guard of every opening of the MIDI system: a failure keeps its cause, and a panic
/// inside the MIDI library (midir panics when ALSA cannot allocate a queue, a failure the
/// application cannot prevent) comes back as an error too. Both name the MIDI system and the cause.
pub fn guarded_open<T>(open: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(open)) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(anyhow!("MIDI system cannot be opened: {e}")),
        Err(p) => {
            let cause = p.downcast_ref::<&str>().map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "the MIDI library panicked".into());
            Err(anyhow!("MIDI system cannot be opened: {cause}"))
        }
    }
}

/// The default opener: the machine's MIDI hardware, through the guard.
pub fn open_hardware() -> Result<Box<dyn Midi>> {
    Ok(Box::new(Hardware::open()?))
}

impl Hardware {
    pub fn open() -> Result<Self> {
        guarded_open(Self::open_unguarded)
    }

    fn open_unguarded() -> Result<Self> {
        let pairs = crate::detect::replay(&crate::detect::observe_hardware(false, Duration::ZERO, &[], &[])?);
        let probe = MidiOutput::new("osc-bridge-take")?;
        let out_ports = probe.ports();
        let in_ports = MidiInput::new("osc-bridge-take")?.ports();
        let (tx, incoming) = channel();
        let (mut outputs, mut inputs) = (Vec::new(), Vec::new());
        for p in pairs {
            let (Some(oi), Some(name)) = (p.out_index, p.out_name) else { continue };
            let Some(port) = out_ports.get(oi) else { continue };
            let conn = MidiOutput::new("osc-bridge-take")?.connect(port, "osc-bridge-take")
                .map_err(|e| anyhow!("open MIDI out {name:?}: {e}"))?;
            outputs.push((name.clone(), conn));
            let Some(port) = p.in_index.and_then(|ii| in_ports.get(ii)) else { continue };
            let mut input = MidiInput::new("osc-bridge-take-in")?;
            input.ignore(Ignore::None);
            let tx = tx.clone();
            if let Ok(conn) = input.connect(port, "osc-bridge-take-in", move |_, msg, _| {
                for m in crate::librarian::split_sysex(msg) { let _ = tx.send((name.clone(), m)); }
            }, ()) {
                inputs.push(conn);
            }
        }
        Ok(Self { outputs, _inputs: inputs, incoming })
    }
}

impl Midi for Hardware {
    fn outputs(&self) -> Vec<String> {
        self.outputs.iter().map(|(n, _)| n.clone()).collect()
    }
    fn send(&mut self, port: &str, messages: &[Vec<u8>], _why: &str) -> Result<(), String> {
        let (_, conn) = self.outputs.iter_mut().find(|(n, _)| n == port).ok_or_else(|| format!("no MIDI output {port:?}"))?;
        for m in messages {
            conn.send(m).map_err(|e| format!("send on {port}: {e}"))?;
            // Some synths drop messages that arrive back to back.
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
    fn received(&mut self, wait: Duration) -> Vec<(String, Vec<u8>)> {
        let end = Instant::now() + wait;
        let mut got = Vec::new();
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match self.incoming.recv_timeout(left) {
                Ok(m) => got.push(m),
                Err(_) => break,
            }
        }
        got
    }
    fn capture(&mut self, probe: bool, wait: Duration, probes: &[crate::recognition::Probe], consent: &[String]) -> Result<crate::capture::Capture> {
        guarded_open(|| crate::detect::observe_hardware(probe, wait, probes, consent))
    }
    fn open_wire(&mut self, device: &Unit) -> Result<Box<dyn Wire + Send>> {
        Ok(Box::new(MidiWire::open(device)?))
    }
}

/// One physical device as its ports show it: all its inputs and outputs (raw names, with their
/// indexes in the capture) and the USB ids of its card, when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub in_indexes: Vec<usize>,
    pub out_indexes: Vec<usize>,
    pub usb: Option<(u16, u16)>,
    /// Whether its client is a device of the system or a program, as the list of clients says.
    pub client_kind: ClientKind,
}

impl Unit {
    /// A name for the device: its first output, else its first input.
    pub fn label(&self) -> &str {
        self.outputs.first().or(self.inputs.first()).map(String::as_str).unwrap_or("?")
    }
}

/// One message a device sent on one of its inputs, with the time since the request went out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub port: String,
    pub at: Duration,
    pub msg: Vec<u8>,
}

/// What the learn mode reads and writes of one device: the universal identity request goes
/// out on each output, then everything every input sends is heard for the time given.
pub trait Wire {
    fn send(&mut self, port: &str, msg: &[u8]);
    fn heard(&mut self, listen: Duration) -> Vec<Heard>;
}

/// A device's ports on the machine: every input listened to, every output asked.
pub struct MidiWire {
    outs: Vec<(String, midir::MidiOutputConnection)>,
    _inputs: Vec<MidiInputConnection<()>>,
    got: Arc<Mutex<Vec<Heard>>>,
    since: Arc<Mutex<Option<std::time::Instant>>>,
}

impl MidiWire {
    pub fn open(device: &Unit) -> Result<Self> {
        crate::midi_port::guarded_open(|| Self::open_unguarded(device))
    }

    fn open_unguarded(device: &Unit) -> Result<Self> {
        let got: Arc<Mutex<Vec<Heard>>> = Arc::new(Mutex::new(Vec::new()));
        let since: Arc<Mutex<Option<std::time::Instant>>> = Arc::new(Mutex::new(None));
        let mut inputs = Vec::new();
        for (ii, name) in device.in_indexes.iter().zip(&device.inputs) {
            let mut inp = MidiInput::new("osc-bridge-learn-in")?;
            inp.ignore(Ignore::None);
            if let Some(port) = inp.ports().get(*ii).cloned() {
                let (sink, t0, port_name) = (got.clone(), since.clone(), name.clone());
                if let Ok(conn) = inp.connect(&port, "osc-bridge-learn-in", move |_, msg, _| {
                    let at = t0.lock().ok().and_then(|t| t.map(|t| t.elapsed())).unwrap_or_default();
                    if let Ok(mut g) = sink.lock() { g.push(Heard { port: port_name.clone(), at, msg: msg.to_vec() }); }
                }, ()) {
                    inputs.push(conn);
                }
            }
        }
        let mut outs = Vec::new();
        for (oi, name) in device.out_indexes.iter().zip(&device.outputs) {
            let o = MidiOutput::new("osc-bridge-learn-out")?;
            if let Some(port) = o.ports().get(*oi) {
                if let Ok(conn) = o.connect(port, "osc-bridge-learn-out") { outs.push((name.clone(), conn)); }
            }
        }
        Ok(MidiWire { outs, _inputs: inputs, got, since })
    }
}

impl Wire for MidiWire {
    fn send(&mut self, port: &str, msg: &[u8]) {
        if let Ok(mut t) = self.since.lock() { t.get_or_insert_with(std::time::Instant::now); }
        if let Some((_, out)) = self.outs.iter_mut().find(|(n, _)| n == port) { let _ = out.send(msg); }
    }
    fn heard(&mut self, listen: Duration) -> Vec<Heard> {
        std::thread::sleep(listen);
        self.got.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
    }
}
