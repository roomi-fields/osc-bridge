//! SysEx librarian: back up and restore a device's presets as `.syx` files.
//!
//! Generic on purpose. Almost every synth can send its memory as SysEx
//! ("bulk dump", triggered from its menu or by a dump-request message) and
//! reload it the same way. A `.syx` file is just those messages back to back,
//! the format every editor and librarian reads.
//!
//! Restoring overwrites the device's memory: callers must make that explicit
//! to the user.

use anyhow::{anyhow, bail, Context, Result};
use midir::{Ignore, MidiInput, MidiOutput};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Split a byte stream into complete `F0 … F7` messages. Bytes outside a
/// message, and a truncated trailing message, are dropped.
pub fn split_sysex(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur: Option<Vec<u8>> = None;
    for &b in bytes {
        match (b, cur.as_mut()) {
            (0xF0, _) => cur = Some(vec![0xF0]),
            (0xF7, Some(m)) => {
                m.push(0xF7);
                out.push(cur.take().unwrap());
            }
            (_, Some(m)) if b < 0x80 => m.push(b),
            (_, Some(_)) => cur = None, // status byte inside SysEx: corrupt, drop
            _ => {}
        }
    }
    out
}

/// Parse `"F0 43 20 00 F7"` (spaces optional, `0x` prefixes allowed).
pub fn parse_hex(s: &str) -> Result<Vec<u8>> {
    let cleaned: String = s.split_whitespace()
        .map(|t| t.trim_start_matches("0x").trim_start_matches("0X"))
        .collect();
    if !cleaned.len().is_multiple_of(2) {
        bail!("odd number of hex digits in {s:?}");
    }
    (0..cleaned.len()).step_by(2)
        .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).map_err(|e| anyhow!("bad hex {s:?}: {e}")))
        .collect()
}

/// Resolve a port given as an index (`"3"`) or a name fragment
/// (`"minilab"`, case-insensitive).
fn resolve_port<T>(ports: &[T], names: &[String], wanted: &str) -> Result<usize> {
    if let Ok(i) = wanted.parse::<usize>() {
        if i < ports.len() { return Ok(i); }
        bail!("no MIDI port at index {i} ({} available)", ports.len());
    }
    let w = wanted.to_lowercase();
    let hits: Vec<usize> = names.iter().enumerate()
        .filter(|(_, n)| n.to_lowercase().contains(&w)).map(|(i, _)| i).collect();
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => bail!("no MIDI port named like {wanted:?}; ports: {names:?}"),
        _ => bail!("{wanted:?} matches several ports: {:?}", hits.iter().map(|&i| &names[i]).collect::<Vec<_>>()),
    }
}

/// What a capture brought back.
#[derive(Debug)]
pub struct Capture {
    pub messages: Vec<Vec<u8>>,
    pub bytes: usize,
}

/// Listen on `in_port` and collect every SysEx message. If `request` is
/// given, send it on `out_port` first (a dump request). Stops `idle` after
/// the last message once one has arrived, or after `max` in any case.
pub fn capture(in_port: &str, request: Option<(&str, &[u8])>, idle: Duration, max: Duration) -> Result<Capture> {
    let mut input = MidiInput::new("osc-bridge-librarian-in")?;
    input.ignore(Ignore::None);
    let ports = input.ports();
    let names: Vec<String> = ports.iter().map(|p| input.port_name(p).unwrap_or_default()).collect();
    let idx = resolve_port(&ports, &names, in_port)?;
    // Messages received so far, and when the last one arrived.
    type Received = (Vec<Vec<u8>>, Option<Instant>);
    let got: Arc<Mutex<Received>> = Arc::new(Mutex::new((Vec::new(), None)));
    let sink = got.clone();
    let _conn = input.connect(&ports[idx], "osc-bridge-librarian-in", move |_, msg, _| {
        if msg.first() == Some(&0xF0) {
            if let Ok(mut g) = sink.lock() {
                g.0.extend(split_sysex(msg));
                g.1 = Some(Instant::now());
            }
        }
    }, ()).map_err(|e| anyhow!("open MIDI in {:?}: {e}", names[idx]))?;

    if let Some((out_port, bytes)) = request {
        send_messages(out_port, &split_sysex(bytes), Duration::ZERO)?;
    }

    let start = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(20));
        let last = got.lock().map_err(|_| anyhow!("capture lock poisoned"))?.1;
        let done_idle = last.is_some_and(|t| t.elapsed() >= idle);
        if done_idle || start.elapsed() >= max {
            break;
        }
    }
    let messages = got.lock().map_err(|_| anyhow!("capture lock poisoned"))?.0.clone();
    let bytes = messages.iter().map(Vec::len).sum();
    Ok(Capture { messages, bytes })
}

/// Send each message on `out_port`, waiting `gap` between messages — older
/// devices drop data when a dump arrives faster than they can write it.
pub fn send_messages(out_port: &str, messages: &[Vec<u8>], gap: Duration) -> Result<()> {
    let output = MidiOutput::new("osc-bridge-librarian-out")?;
    let ports = output.ports();
    let names: Vec<String> = ports.iter().map(|p| output.port_name(p).unwrap_or_default()).collect();
    let idx = resolve_port(&ports, &names, out_port)?;
    let mut conn = output.connect(&ports[idx], "osc-bridge-librarian-out")
        .map_err(|e| anyhow!("open MIDI out {:?}: {e}", names[idx]))?;
    for (i, m) in messages.iter().enumerate() {
        conn.send(m).map_err(|e| anyhow!("send message {}: {e}", i + 1))?;
        if !gap.is_zero() && i + 1 < messages.len() {
            std::thread::sleep(gap);
        }
    }
    Ok(())
}

/// Writes the `.syx` file `file` to `dev` on `out_port`, as its driver says
/// (`programs::restore`): `target` is a bank and number for a sound that
/// carries no place. A bank the driver says cannot be written is refused
/// before the port is opened. The number of messages sent.
pub fn restore_to_device(dev: &osd::Device, out_port: &str, file: &Path, target: Option<(&str, i64)>) -> Result<usize> {
    let msgs = read_syx(file)?;
    let channel = dev.midi_out.as_ref().and_then(|m| m.default_channel).unwrap_or(0) & 0x0F;
    let number = dev.identity.as_ref().and_then(|i| crate::device::kept(&i.device_id)).and_then(|d| d.default).map(|d| d as u8);
    if let Some((bank, n)) = target {
        crate::programs::store(dev, bank, n, Some(channel)).map_err(|e| anyhow!(e))?;
    }
    let output = MidiOutput::new("osc-bridge-librarian-out")?;
    let ports = output.ports();
    let names: Vec<String> = ports.iter().map(|p| output.port_name(p).unwrap_or_default()).collect();
    let idx = resolve_port(&ports, &names, out_port)?;
    let mut conn = output.connect(&ports[idx], "osc-bridge-librarian-out")
        .map_err(|e| anyhow!("open MIDI out {:?}: {e}", names[idx]))?;
    crate::programs::restore(dev, &msgs, target, channel, number, &mut crate::programs::SystemClock,
        &mut |m: &[u8]| conn.send(m).map_err(|e| e.to_string())).map_err(|e| anyhow!(e))
}

/// Records the sounds `dev` sends on `in_port`, taking every message its
/// driver's sound transfers recognise whatever the channel. With `out_port`,
/// the driver's request is sent first (`programs.dump`).
pub fn backup_from_device(dev: &osd::Device, in_port: &str, out_port: Option<&str>, idle: Duration, max: Duration) -> Result<Capture> {
    /// Records the wait the driver asks instead of sleeping: `capture` is the wait.
    struct Asked(Duration);
    impl crate::programs::Clock for Asked {
        fn sleep(&mut self, d: Duration) {
            self.0 += d;
        }
    }
    let mut asked = Asked(Duration::ZERO);
    let request = match out_port {
        Some(port) => Some((port, crate::programs::dump_paced(dev, None, None, &mut asked).map_err(|e| anyhow!(e))?.concat())),
        None => None,
    };
    // The reply timeout the description states bounds the wait for the answer.
    let max = if asked.0.is_zero() { max } else { asked.0 };
    let cap = capture(in_port, request.as_ref().map(|(p, b)| (*p, b.as_slice())), idle, max)?;
    let messages = crate::programs::backup_filter(dev, &cap.messages);
    let bytes = messages.iter().map(Vec::len).sum();
    Ok(Capture { messages, bytes })
}

pub fn write_syx(path: &Path, messages: &[Vec<u8>]) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    std::fs::write(path, messages.concat()).with_context(|| format!("write {}", path.display()))
}

pub fn read_syx(path: &Path) -> Result<Vec<Vec<u8>>> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let messages = split_sysex(&bytes);
    if messages.is_empty() {
        bail!("{} holds no complete SysEx message", path.display());
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_concatenated_messages_and_drops_garbage() {
        let stream = [0x00, 0xF0, 0x43, 0x10, 0xF7, 0x55, 0xF0, 0x41, 0x7F, 0xF7, 0xF0, 0x01];
        assert_eq!(split_sysex(&stream), vec![vec![0xF0, 0x43, 0x10, 0xF7], vec![0xF0, 0x41, 0x7F, 0xF7]]);
    }

    #[test]
    fn status_byte_inside_a_message_discards_it() {
        assert!(split_sysex(&[0xF0, 0x43, 0x90, 0x10, 0xF7]).is_empty());
    }

    #[test]
    fn parses_hex_with_or_without_spaces() {
        assert_eq!(parse_hex("F0 43 20 00 F7").unwrap(), vec![0xF0, 0x43, 0x20, 0x00, 0xF7]);
        assert_eq!(parse_hex("0xF0 0x7E f7").unwrap(), vec![0xF0, 0x7E, 0xF7]);
        assert!(parse_hex("F0 4").is_err());
    }

    #[test]
    fn resolves_ports_by_index_or_unique_name_fragment() {
        let names = vec!["Midi Through".to_string(), "Minilab3 MIDI".to_string(), "TR-8S".to_string()];
        let ports = vec![(); 3];
        assert_eq!(resolve_port(&ports, &names, "2").unwrap(), 2);
        assert_eq!(resolve_port(&ports, &names, "minilab").unwrap(), 1);
        assert!(resolve_port(&ports, &names, "midi").is_err(), "ambiguous");
        assert!(resolve_port(&ports, &names, "9").is_err());
    }

    #[test]
    fn syx_file_round_trips() {
        let dir = tempfile::Builder::new().prefix("osc-bridge-syx-").tempdir().unwrap();
        let path = dir.path().join("bank.syx");
        let msgs = vec![vec![0xF0, 0x43, 0x00, 0xF7], vec![0xF0, 0x43, 0x01, 0x02, 0xF7]];
        write_syx(&path, &msgs).unwrap();
        assert_eq!(read_syx(&path).unwrap(), msgs);
    }
}
