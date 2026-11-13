//! Take the sound a synth is playing, through the SynthHub librarian.
//!
//! The server knows how to talk to each synth (it runs the KnobKraft Orm
//! adaptations); this side only carries MIDI: it sends what the server says
//! to send, and hands back what the synth answers. The same exchange as the
//! site's "Connect my synth", for an agent on this machine.
//!
//! 1. Find: every port gets the Identity Request and the detect messages of
//!    the synths its name points at, then, if nothing answered, every other
//!    synth's; each reply is recognized by the server.
//! 2. Take: the server gives the request (edit buffer, or a program), the
//!    synth's answer goes back part by part until the server says the sound
//!    is complete, with its name.

use anyhow::{anyhow, Result};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
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

/// One call to the server's librarian: `op` is `recognize`, `request` or
/// `receive`.
pub type Ask<'a> = &'a dyn Fn(&str, &Value) -> Result<Value, String>;

/// A synth that answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub synth: String,
    pub name: String,
    pub channel: i64,
    pub port: String,
    pub devices: Vec<String>,
    pub firmware: Option<String>,
}

/// A sound taken from a synth.
#[derive(Debug, Clone, PartialEq)]
pub struct Taken {
    pub name: String,
    pub messages: Vec<Vec<u8>>,
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn squash(s: &str) -> String {
    s.to_lowercase().chars().filter(char::is_ascii_alphanumeric).collect()
}

/// Whether a port's name points at a synth: its model (the synth's name
/// without its first word, the maker) or one of its words of 3 letters or
/// more appears in the port name.
fn named(synth_name: &str, port: &str) -> bool {
    let p = squash(port);
    let words: Vec<&str> = synth_name.split_whitespace().collect();
    let model = squash(&words.iter().skip(1).copied().collect::<Vec<_>>().join(""));
    (model.len() >= 3 && p.contains(&model)) || words.iter().skip(1).map(|w| squash(w)).any(|w| w.len() >= 3 && p.contains(&w))
}

/// The firmware in a Universal Identity Reply, e.g. "1.0.2.0".
fn firmware_of(m: &[u8]) -> Option<String> {
    crate::detect::parse_identity_reply(m).map(|id| id.version.iter().map(u8::to_string).collect::<Vec<_>>().join("."))
}

fn messages_of(hexes: &str) -> Vec<Vec<u8>> {
    crate::librarian::parse_hex(hexes).map(|b| crate::librarian::split_sysex(&b)).unwrap_or_default()
}

/// The synths answering on `port` (every port if None), among those the
/// librarian lists (`lib`, as `GET /v1/librarian` gives it).
pub fn find(midi: &mut dyn Midi, lib: &Value, ask: Ask, port: Option<&str>) -> Result<Vec<Found>, String> {
    let all = midi.outputs();
    let outs: Vec<String> = match port.map(|p| (p, p.parse::<usize>())) {
        Some((_, Ok(i))) => all.get(i).cloned().into_iter().collect(),
        Some((p, _)) => all.iter().filter(|o| o.to_lowercase().contains(&p.to_lowercase())).cloned().collect(),
        None => all,
    };
    if outs.is_empty() {
        return Err(match port {
            Some(p) => format!("no MIDI output named like {p:?}; outputs: {:?}", midi.outputs()),
            None => "no MIDI output: plug the synth in (USB or a MIDI interface)".into(),
        });
    }
    let synths = lib["synths"].as_array().cloned().unwrap_or_default();
    let mut found: Vec<Found> = Vec::new();
    let mut firmware: HashMap<String, String> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    // A port given by the user gets every synth's messages at once.
    let rounds: &[u8] = match (port, midi.searches_everywhere()) {
        (Some(_), _) => &[0],
        (None, true) => &[1, 2],
        (None, false) => &[1],
    };
    for &round in rounds {
        let mut wait = 300u64;
        for out in &outs {
            let mut batch: Vec<Vec<u8>> = Vec::new();
            for s in &synths {
                let is_named = named(s["name"].as_str().unwrap_or_default(), out);
                if (round == 1 && !is_named) || (round == 2 && is_named) { continue; }
                for d in s["detect"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    if round != 1 && !d.starts_with("F0") { continue; }
                    for m in messages_of(d) {
                        if !batch.contains(&m) { batch.push(m); }
                    }
                }
                wait = wait.max(s["detect_wait_ms"].as_u64().unwrap_or(200));
            }
            if round != 2 { batch.push(crate::detect::IDENTITY_REQUEST.to_vec()); }
            if !batch.is_empty() {
                midi.send(out, &batch, &format!("find which synth is on {out} (asks it who it is; changes nothing)"))?;
            }
        }
        for (out, m) in midi.received(Duration::from_millis(wait + 300)) {
            if let Some(fw) = firmware_of(&m) { firmware.insert(out.clone(), fw); }
            let reply = hex(&m);
            if !seen.insert(format!("{out}|{reply}")) || seen.len() > 40 { continue; }
            let r = ask("recognize", &json!({ "message": reply }))?;
            for f in r["found"].as_array().into_iter().flatten() {
                let synth = f["synth"].as_str().unwrap_or_default().to_string();
                // One unit answering on several of its ports sends the same
                // reply on each: kept once, on the first.
                if !seen.insert(format!("{synth}|{reply}")) { continue; }
                found.push(Found {
                    name: f["name"].as_str().unwrap_or(&synth).to_string(),
                    synth,
                    channel: f["channel"].as_i64().unwrap_or(0),
                    port: out.clone(),
                    devices: f["devices"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect(),
                    firmware: None,
                });
            }
        }
        if !found.is_empty() { break; }
    }
    for f in &mut found { f.firmware = firmware.get(&f.port).cloned(); }
    Ok(found)
}

/// Ask `f` for its sound: the one it is playing, or `program` (from 0).
/// Waits up to `max` for the synth's answer.
pub fn take(midi: &mut dyn Midi, ask: Ask, f: &Found, program: Option<u64>, max: Duration) -> Result<Taken, String> {
    let r = ask("request", &json!({ "synth": f.synth, "channel": f.channel, "program": program }))?;
    let what = match program { Some(p) => format!("its program {}", p + 1), None => "the sound it is playing".into() };
    midi.send(&f.port, &messages_of(r["request"].as_str().unwrap_or_default()),
              &format!("ask {} for {what} (reads it; changes nothing)", f.name))?;
    let start = Instant::now();
    let mut so_far = String::new();
    while start.elapsed() < max {
        for (out, m) in midi.received(Duration::from_millis(100)) {
            if out != f.port { continue; }
            let message = hex(&m);
            let r = ask("receive", &json!({ "synth": f.synth, "channel": f.channel, "so_far": so_far, "message": message }))?;
            if r["part"] != json!(true) { continue; }
            so_far = if so_far.is_empty() { message } else { format!("{so_far} {message}") };
            let answer = messages_of(r["answer"].as_str().unwrap_or_default());
            if !answer.is_empty() {
                midi.send(&f.port, &answer, &format!("answer {} while it sends its sound (it asks for each part)", f.name))?;
            }
            if r["complete"] == json!(true) {
                let messages = messages_of(r["keep"].as_str().unwrap_or_default());
                return Ok(Taken { name: r["name"].as_str().unwrap_or_default().to_string(), messages });
            }
        }
    }
    Err(format!("{} did not send its sound within {} s: check that SysEx is allowed in its MIDI settings", f.name, max.as_secs()))
}

/// The MIDI ports of this machine: every output, and the input of the same
/// device (paired by name, as `detect_hardware` does) listened to.
pub struct Hardware {
    outputs: Vec<(String, MidiOutputConnection)>,
    _inputs: Vec<MidiInputConnection<()>>,
    incoming: Receiver<(String, Vec<u8>)>,
}

impl Hardware {
    pub fn open() -> Result<Self> {
        let pairs = crate::detect::scan(false, Duration::ZERO, &[])?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::TestSynth;

    /// The test synth on two ports (one unit, answering on both), and a
    /// silent port.
    struct Bench { synth: TestSynth, waiting: Vec<(String, Vec<u8>)>, sent: Vec<(String, Vec<u8>)>, whys: Vec<String> }
    impl Midi for Bench {
        fn outputs(&self) -> Vec<String> { vec!["Test Synth A".into(), "Test Synth B".into(), "Midi Through".into()] }
        fn send(&mut self, port: &str, messages: &[Vec<u8>], why: &str) -> Result<(), String> {
            self.whys.push(why.into());
            for data in messages {
                self.sent.push((port.into(), data.clone()));
                if port.starts_with("Test Synth") {
                    if let Some(r) = self.synth.answer(data) { self.waiting.push((port.into(), r)); }
                }
            }
            Ok(())
        }
        fn received(&mut self, _: Duration) -> Vec<(String, Vec<u8>)> { std::mem::take(&mut self.waiting) }
    }

    /// The server's librarian for the test synth, as the Test_Synth
    /// adaptation in SynthHub's tests answers.
    fn server(op: &str, body: &Value) -> Result<Value, String> {
        let msg = crate::librarian::parse_hex(body["message"].as_str().unwrap_or_default()).unwrap_or_default();
        Ok(match op {
            "recognize" if msg == TestSynth::IDENTITY_REPLY => json!({ "found": [{ "synth": "Test_Synth", "name": "Test Synth",
                "channel": 3, "devices": ["examples/test_synth"] }] }),
            "recognize" => json!({ "found": [] }),
            "request" => json!({ "request": hex(&TestSynth::DUMP_REQUEST) }),
            "receive" if msg.starts_with(&TestSynth::PATCH) => json!({ "part": true, "complete": true, "keep": hex(&msg),
                "name": "Init" }),
            "receive" => json!({ "part": false }),
            _ => return Err(format!("no op {op}")),
        })
    }

    fn lib() -> Value {
        json!({ "synths": [{ "synth": "Test_Synth", "name": "Test Synth", "detect": [hex(&crate::detect::IDENTITY_REQUEST)],
            "detect_wait_ms": 100 }] })
    }

    #[test]
    fn a_synth_on_two_ports_is_found_once_with_its_firmware_then_its_sound_taken() {
        let mut bench = Bench { synth: TestSynth::default(), waiting: vec![], sent: vec![], whys: vec![] };
        let found = find(&mut bench, &lib(), &server, None).unwrap();
        assert_eq!(found, vec![Found { synth: "Test_Synth".into(), name: "Test Synth".into(), channel: 3,
            port: "Test Synth A".into(), devices: vec!["examples/test_synth".into()], firmware: Some("1.0.0.0".into()) }]);
        let taken = take(&mut bench, &server, &found[0], None, Duration::from_secs(1)).unwrap();
        assert_eq!(taken, Taken { name: "Init".into(), messages: vec![[&TestSynth::PATCH[..], &[64, 0, 0, 40, 0, 0xF7]].concat()] });
        assert!(bench.sent.iter().any(|(p, m)| p == "Test Synth A" && m == &TestSynth::DUMP_REQUEST));
        assert_eq!(bench.whys.last().unwrap(), "ask Test Synth for the sound it is playing (reads it; changes nothing)");
        assert_eq!(bench.whys.iter().filter(|w| w.starts_with("find which synth is on")).count(), 3, "one batch per port");
    }

    #[test]
    fn a_port_given_is_the_only_one_asked() {
        let mut bench = Bench { synth: TestSynth::default(), waiting: vec![], sent: vec![], whys: vec![] };
        let found = find(&mut bench, &lib(), &server, Some("synth b")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].port, "Test Synth B");
        assert!(bench.sent.iter().all(|(p, _)| p == "Test Synth B"));
        assert_eq!(find(&mut bench, &lib(), &server, Some("0")).unwrap()[0].port, "Test Synth A", "a port by its index");
        assert!(find(&mut bench, &lib(), &server, Some("nope")).unwrap_err().contains("no MIDI output named like"));
    }

    #[test]
    fn a_silent_synth_says_so() {
        struct Silent;
        impl Midi for Silent {
            fn outputs(&self) -> Vec<String> { vec!["Test Synth".into()] }
            fn send(&mut self, _: &str, _: &[Vec<u8>], _: &str) -> Result<(), String> { Ok(()) }
            fn received(&mut self, _: Duration) -> Vec<(String, Vec<u8>)> { vec![] }
        }
        let f = Found { synth: "Test_Synth".into(), name: "Test Synth".into(), channel: 3, port: "Test Synth".into(),
            devices: vec![], firmware: None };
        assert!(take(&mut Silent, &server, &f, None, Duration::from_millis(200)).unwrap_err().contains("did not send its sound"));
    }

    #[test]
    fn a_port_name_points_at_a_synth_by_its_model() {
        assert!(named("Korg Minilogue XD", "minilogue xd MIDI 1"));
        assert!(named("Sequential Trigon-6", "Trigon-6"));
        assert!(!named("Waldorf M", "Midi Through Port-0"));
    }
}
