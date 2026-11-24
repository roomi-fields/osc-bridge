//! The bodies of the six commands that name a device (`run`, `inspect`, `backup`, `restore`,
//! `export`, `orchestrate`), with plain values and the writers the caller gives. The osc-bridge
//! binary and the program built on this library call them, so each command exists once.

use crate::{Device, Runtime, RuntimeOptions};
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The options of `run`.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// The driver: a JSON file, or its catalogue id or OSC prefix.
    pub device: PathBuf,
    /// MIDI OUT port index; required for a MIDI/SysEx driver.
    pub out_port: Option<usize>,
    /// MIDI IN port index, for the MIDI→OSC feedback.
    pub in_port: Option<usize>,
    /// UDP address to bind for incoming OSC.
    pub bind: String,
    /// OSC client addresses for outbound events.
    pub osc_clients: Vec<String>,
    /// WebSocket listen address.
    pub ws_bind: Option<String>,
    /// Browser origins allowed to connect to the WebSocket.
    pub ws_allow_origins: Vec<String>,
    /// HTTP listen address answering OSCQuery requests.
    pub oscquery_bind: Option<String>,
}

/// The driver file of a path, a catalogue id or an OSC prefix (the one place the commands read the catalogue).
fn locate(device: &Path) -> Result<PathBuf> {
    crate::synthub::catalogue::locate(device)
}

/// One `WARN` line per checksum the engine cannot compute; the build of that frame fails.
pub fn checksum_warnings(dev: &Device) -> Vec<String> {
    let faults = crate::device::faults(dev).into_iter()
        .map(|f| format!("  WARN: {} {}: {} — detection sends the universal request instead.", f.device, f.field, f.cause));
    let checksums = crate::device::checksum_problems(dev).into_iter()
        .map(|(place, problem)| format!("  WARN: checksum at {place}: {problem} — building this frame fails."));
    faults.chain(checksums).collect()
}

/// Run the bridge for a device; the note about an ignored `--in-port` goes on `err`.
pub fn run(options: RunOptions, err: &mut dyn Write) -> Result<()> {
    let RunOptions { device: path, out_port, in_port, bind, osc_clients: clients, ws_bind, ws_allow_origins, oscquery_bind } = options;
    let dev = crate::device::load_with_docs(locate(&path)?)?;
    let is_osc = dev.device.transport.as_ref().map(|t| t.kind) == Some(osd::TransportKind::Osc);
    let out_port = match (out_port, is_osc) {
        (Some(p), _) => p,
        (None, true) => 0, // runtime branches early; value is unused.
        (None, false) => anyhow::bail!(
            "--out-port is required for hardware (MIDI/SysEx) drivers. Run `osc-bridge list` to find the index, then pass `--out-port <N>`."
        ),
    };
    if is_osc && in_port.is_some() {
        writeln!(err, "note: --in-port is ignored for OSC-transport drivers")?;
    }
    let client_addrs: Vec<std::net::SocketAddr> = clients.iter()
        .map(|s| s.parse::<std::net::SocketAddr>()
            .with_context(|| format!("parsing --osc-client {s}")))
        .collect::<Result<Vec<_>>>()?;
    Runtime::run(RuntimeOptions {
        device: dev,
        midi_out_port_idx: out_port,
        midi_in_port_idx: if is_osc { None } else { in_port },
        osc_bind: bind,
        osc_clients: client_addrs,
        ws_bind,
        ws_allow_origins,
        oscquery_bind,
    })
}

/// Print the summary of a device on `out`, the checksum alerts on `err`.
pub fn inspect(path: &Path, out: &mut dyn Write, err: &mut dyn Write) -> Result<()> {
    let dev = crate::device::load(locate(path)?)?;
    for line in checksum_warnings(&dev) { writeln!(err, "{line}")?; }
    let d = &dev.device;
    writeln!(out, "Device:   {} ({})", d.name, d.revision.as_deref().unwrap_or_default())?;
    writeln!(out, "Vendor:   {}", d.vendor.as_deref().unwrap_or_default())?;
    writeln!(out, "OSC prefix: {}", d.osc_prefix)?;
    writeln!(out, "MID:      {}", hex_vec(d.manufacturer_id.as_deref().unwrap_or_default()))?;
    writeln!(out, "Dev id:   {}", hex_vec(d.device_id.as_deref().unwrap_or_default()))?;
    writeln!(out, "Rate limit: {:?} Hz", d.rate_limit_hz)?;
    let commands = dev.commands.as_deref().unwrap_or_default();
    writeln!(out, "Commands: {} entries", commands.len())?;
    for c in commands {
        writeln!(out, "  {}  args={}", crate::device::address_text(&c.osc), c.args.as_ref().map_or(0, Vec::len))?;
    }
    if let Some(pt) = &dev.params {
        writeln!(out, "Params:   {} entries", pt.entries.len())?;
        for e in pt.entries.iter().take(5) {
            let [lo, hi] = e.range.unwrap_or([0, 127]);
            writeln!(out, "  {}  ({:#04x},{:#04x},{:#04x},{:#04x})  range=[{},{}]",
                e.osc, e.pr.unwrap_or(0), e.p.unwrap_or(0), e.c.unwrap_or(0), e.r.unwrap_or(0), lo, hi)?;
        }
        if pt.entries.len() > 5 { writeln!(out, "  ... +{} more", pt.entries.len() - 5)?; }
    }
    writeln!(out, "Replies:  {} patterns", dev.replies.as_ref().map_or(0, Vec::len))?;
    Ok(())
}

fn hex_vec(v: &[u8]) -> String {
    v.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
}

/// Record the SysEx a device sends into a `.syx` file.
pub fn backup(device: &Path, in_port: &str, out: &Path, out_port: Option<&str>,
              idle_secs: u64, max_secs: u64, err: &mut dyn Write) -> Result<()> {
    use crate::librarian::{backup_from_device, write_syx};
    use std::time::Duration;
    let dev = crate::device::load(locate(device)?)?;
    if out_port.is_none() {
        writeln!(err, "Listening — start the SysEx dump from the device's menu now.")?;
    }
    let cap = backup_from_device(&dev, in_port, out_port,
                                 Duration::from_secs(idle_secs), Duration::from_secs(max_secs))?;
    if cap.messages.is_empty() {
        anyhow::bail!("no SysEx received within {max_secs} s");
    }
    write_syx(out, &cap.messages)?;
    writeln!(err, "Saved {} message(s), {} bytes to {}", cap.messages.len(), cap.bytes, out.display())?;
    Ok(())
}

/// Send a `.syx` file to a device; `target` is the bank and number for a sound that carries no place.
pub fn restore(device: &Path, out_port: &str, target: Option<(&str, i64)>, file: &Path,
               err: &mut dyn Write) -> Result<()> {
    let dev = crate::device::load(locate(device)?)?;
    let sent = crate::librarian::restore_to_device(&dev, out_port, file, target)?;
    writeln!(err, "Sent {sent} SysEx message(s) from {}", file.display())?;
    Ok(())
}

/// Write a device in another format on `out` (`format`), or put its export on the plugged device
/// `on` (`(driver, yes)`; without `yes` nothing is sent). The losses go on `err`.
pub fn export(device: &Path, format: Option<&str>, on: Option<(&Path, bool)>,
              out: &mut dyn Write, err: &mut dyn Write) -> Result<()> {
    if let Some((on, yes)) = on {
        let done = crate::install::install_command(
            &locate(device)?,
            &locate(on)?,
            yes,
            &crate::midi_port::open_hardware,
        )?;
        for l in &done.lost {
            writeln!(err, "not carried: {}: {}", l.place, l.reason)?;
        }
        if done.written {
            writeln!(out, "wrote {} ({} bytes, {}, by {}): {} message(s) sent", done.path, done.size, done.format, done.way, done.messages)?;
        } else {
            writeln!(out, "nothing sent: it would write {} ({} bytes, {}, by {}) and may overwrite a file on the device; add --yes to send",
                done.path, done.size, done.format, done.way)?;
        }
        return Ok(());
    }
    let dev = crate::device::load(locate(device)?)?;
    let format = format.ok_or_else(|| anyhow::anyhow!("--format is required without --on"))?;
    let exported = crate::export::export(&dev, format.parse()?)?;
    for l in &exported.lost {
        writeln!(err, "not carried: {}: {}", l.place, l.reason)?;
    }
    write!(out, "{}", exported.value)?;
    Ok(())
}

/// Run several devices in one process, driven by a `bridge.toml`.
pub fn orchestrate(config: &Path) -> Result<()> {
    crate::orchestrator::Orchestrator::run(config, locate)
}
