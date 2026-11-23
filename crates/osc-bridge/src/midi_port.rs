//! The MIDI ports a command opens: the type the bridge puts MIDI through.
//!
//! `Midi` is how the ports are reached (the hardware, a relay, or a test synth), `Opener` what a
//! command takes its MIDI from, `guarded_open` the single guard of every opening of the MIDI
//! system, and `Hardware` the machine's own ports. `install`, `detect` and `mcp` read this module;
//! the link to the service reads it too, never the reverse.

use anyhow::{anyhow, Result};
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
        let pairs = crate::detect::replay(&crate::detect::observe_unguarded(false, Duration::ZERO, &[], &[])?);
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
}
