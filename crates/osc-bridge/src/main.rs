use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use midir::{MidiInput, MidiOutput};
use std::path::PathBuf;

use osc_bridge::{Device, Runtime, RuntimeOptions};

#[cfg(test)]
mod tests;

#[derive(Parser)]
#[command(name = "osc-bridge", version, about = "Declarative OSC ↔ MIDI/SysEx bridge")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List MIDI input and output ports.
    List,
    /// Back up a device's presets: record the SysEx it sends into a .syx file.
    /// Start the dump from the device's menu, or pass --out-port to send the
    /// driver's dump request.
    Backup {
        /// The device: a driver file, catalogue id or slug. Its driver says
        /// which messages are its sounds and how to ask for them.
        #[arg(long)]
        device: PathBuf,
        /// MIDI IN port the device sends on (index or name fragment).
        #[arg(long)]
        in_port: String,
        /// Output .syx file.
        #[arg(long)]
        out: PathBuf,
        /// MIDI OUT port: the driver's dump request is sent on it first.
        #[arg(long)]
        out_port: Option<String>,
        /// Stop this many seconds after the last message.
        #[arg(long, default_value_t = 3)]
        idle_secs: u64,
        /// Give up after this many seconds in any case.
        #[arg(long, default_value_t = 120)]
        max_secs: u64,
    },
    /// Restore presets from a .syx file. Overwrites the device's memory.
    Restore {
        /// The device: a driver file, catalogue id or slug. Its driver sets
        /// the pauses and refuses a bank that cannot be written.
        #[arg(long)]
        device: PathBuf,
        /// MIDI OUT port of the device (index or name fragment).
        #[arg(long)]
        out_port: String,
        /// The bank to write into (name or place), for a sound that carries
        /// no place; goes with --number.
        #[arg(long, requires = "number")]
        bank: Option<String>,
        /// The program number to write into, as the device shows it.
        #[arg(long, requires = "bank")]
        number: Option<i64>,
        /// The .syx file to send.
        file: PathBuf,
    },
    /// Let an AI agent reach this machine's MIDI devices through the hosted
    /// relay: prints a pairing code to give the agent, then carries MIDI
    /// between the relay and every port until stopped.
    Connect {
        /// Relay WebSocket address.
        #[arg(long, default_value = osc_bridge::connect::DEFAULT_RELAY)]
        server: String,
        /// Do not record the identity (model, firmware) of the devices seen.
        #[arg(long)]
        no_share: bool,
        /// Also offer a virtual synth (the catalogue's examples/test_synth),
        /// to try everything without hardware.
        #[arg(long)]
        test_synth: bool,
        /// Keep the device on the ports whose name contains this private:
        /// its identity is never recorded (repeatable).
        #[arg(long = "private-port", value_name = "NAME")]
        private_ports: Vec<String>,
    },
    /// Show or change what this machine shares with the SynthHub server.
    /// Sharing is on by default: drivers come from the server as they are
    /// needed, and reports and fixes go to it under an anonymous account.
    Privacy {
        /// `on` cuts the link to the server (only the drivers already cached
        /// are used, nothing is sent); `off` restores it.
        #[arg(long, value_parser = ["on", "off"])]
        offline: Option<String>,
        /// Keep this device private (catalogue id, slug, or `vendor/*` for a
        /// whole vendor): nothing about it is sent to the server (repeatable).
        /// The same list can be edited in the settings file it prints.
        #[arg(long, value_name = "DEVICE")]
        private: Vec<String>,
        /// Share this device again (repeatable).
        #[arg(long, value_name = "DEVICE")]
        public: Vec<String>,
    },
    /// Find connected MIDI devices and the catalogue drivers that match them.
    /// Sends the standard MIDI Identity Request unless --no-probe.
    Detect {
        /// Only list ports and match by name; send nothing.
        #[arg(long)]
        no_probe: bool,
        /// Match against the drivers of this folder instead of the SynthHub
        /// catalogue (offline: the drivers cached on this machine).
        #[arg(long)]
        devices_dir: Option<PathBuf>,
    },
    /// Print summary of a device JSON spec without starting the bridge.
    Inspect {
        /// A driver file, or a catalogue id or slug (see `run --device`).
        device: PathBuf,
    },
    /// Run the bridge for a given device JSON.
    Run(RunArgs),
    /// Write a device JSON in another format on stdout (`oscquery`, `midnam`,
    /// `deluge`, and the others of osd-formats; an unknown name lists them).
    /// With `--on`, put its export on a plugged device instead: the format,
    /// the path and the way come from that device's own driver. Without
    /// `--yes` nothing is sent and what would be written is shown.
    Export {
        /// The driver: a JSON file, or its catalogue id or slug (see `run`).
        #[arg(long)]
        device: PathBuf,
        /// The format to write on stdout (not with `--on`: the target's driver gives it).
        #[arg(long, required_unless_present = "on", conflicts_with = "on")]
        format: Option<String>,
        /// The device that receives the export: its driver file, catalogue id or slug.
        #[arg(long)]
        on: Option<PathBuf>,
        /// With `--on`: the file may be overwritten on the target; send.
        #[arg(long, requires = "on")]
        yes: bool,
    },
    /// Send one OSC message to a local bridge for quick testing.
    OscSend {
        #[arg(long, default_value = "127.0.0.1:7777")]
        target: String,
        addr: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
        /// Read a file and append its contents as a single additional string
        /// OSC arg. Bypasses shell quoting hell for large JSON payloads.
        #[arg(long = "from-file")]
        from_file: Option<PathBuf>,
    },
    /// Listen on a UDP port and pretty-print every OSC message received.
    OscListen {
        #[arg(long, default_value = "127.0.0.1:8888")]
        bind: String,
    },
    /// Validate a device JSON spec; warn on every `transform` / `script` use.
    Lint {
        device: PathBuf,
    },
    /// Run multiple devices in one process driven by a `bridge.toml`.
    Orchestrate {
        #[arg(long)]
        config: PathBuf,
    },
    /// Run a Model Context Protocol (MCP) server on stdio. Exposes the device
    /// catalogue and OSC surface to LLM clients (Claude Desktop, etc.).
    Mcp {
        /// Serve the drivers of this folder, as they are, instead of those of
        /// the SynthHub catalogue (fetched one at a time and cached).
        #[arg(long)]
        devices_dir: Option<PathBuf>,
        /// Default OSC target for the `send` and `get_status` tools when no
        /// `target` argument is supplied per call. Defaults to 127.0.0.1:7777
        /// (matching the standard `osc-bridge run` bind).
        #[arg(long, default_value = "127.0.0.1:7777")]
        default_target: String,
        /// Do not contact the SynthHub server for this run (also
        /// OSC_BRIDGE_OFFLINE=1; `osc-bridge privacy --offline on` for good).
        /// By default a local copy fetches each driver it needs from the
        /// server — as other users and agents fixed and confirmed it — and
        /// keeps it in its cache, shows how far each driver is confirmed, and sends report_result
        /// and the contribution tools there, under an anonymous account kept
        /// on this machine. MIDI ports are never shared.
        #[arg(long)]
        offline: bool,
    },
}

// The options of `run`.
#[derive(Args)]
struct RunArgs {
    /// The driver: a JSON file, or its catalogue id (arturia/minilab3)
    /// or slug (minilab3), fetched from the SynthHub server and kept in
    /// the cache (offline: the cached copy).
    #[arg(long)]
    device: PathBuf,
    /// MIDI OUT port index. Required for hardware (MIDI/SysEx) drivers,
    /// ignored when the driver declares `device.transport.kind = "osc"`.
    #[arg(long)]
    out_port: Option<usize>,
    /// Optional MIDI IN port index — enables MIDI→OSC feedback. Ignored
    /// when the driver declares `device.transport.kind = "osc"`.
    #[arg(long)]
    in_port: Option<usize>,
    /// UDP address to bind for incoming OSC.
    #[arg(long, default_value = "127.0.0.1:7777")]
    bind: String,
    /// OSC client address for outbound events. Repeatable — every listed
    /// client receives a copy of each MIDI-in event, SysEx reply, and
    /// `/bridge/status` response.
    #[arg(long = "osc-client")]
    osc_clients: Vec<String>,
    /// Optional WebSocket listen address (e.g. 127.0.0.1:7890) for
    /// browser clients. Binary WS frames carry raw OSC packets (same
    /// bytes as UDP); each connected client acts as an extra
    /// --osc-client and can send commands through the same dispatch.
    #[arg(long = "ws-bind")]
    ws_bind: Option<String>,
    /// Browser origin allowed to connect to the WebSocket, e.g.
    /// https://example.org (repeatable; `*` allows any site). Pages
    /// served from localhost and non-browser clients are always allowed.
    #[arg(long = "ws-allow-origin")]
    ws_allow_origins: Vec<String>,
    /// Optional HTTP listen address (e.g. 127.0.0.1:7891) answering
    /// OSCQuery requests: `/` gives the device's OSC namespace (routes,
    /// types, ranges), `/?HOST_INFO` the OSC port and transport.
    #[arg(long = "oscquery-bind")]
    oscquery_bind: Option<String>,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::List => cmd_list(),
        Cmd::Backup { device, in_port, out, out_port, idle_secs, max_secs } =>
            cmd_backup(device, in_port, out, out_port, idle_secs, max_secs),
        Cmd::Restore { device, out_port, bank, number, file } => {
            let dev = osc_bridge::device::load(osc_bridge::catalogue::locate(&device)?)?;
            let target = bank.as_deref().zip(number);
            let sent = osc_bridge::librarian::restore_to_device(&dev, &out_port, &file, target)?;
            eprintln!("Sent {sent} SysEx message(s) from {}", file.display());
            Ok(())
        }
        Cmd::Connect { server, no_share, test_synth, private_ports } =>
            osc_bridge::connect::run(&server, !no_share, test_synth, &private_ports),
        Cmd::Privacy { offline, private, public } => {
            let path = osc_bridge::online::settings_file().context("no configuration directory (HOME is not set)")?;
            let offline = offline.map(|o| o == "on");
            let cached = osc_bridge::mcp::DeviceIndex::build(&osc_bridge::online::cache_dir().unwrap_or_default());
            // A device not cached here is looked up on the server, unless the link is being cut.
            let server = (offline != Some(true) && osc_bridge::online::linked())
                .then(|| osc_bridge::online::Online::new(osc_bridge::online::SERVER));
            print!("{}", osc_bridge::mcp::privacy(&path, &cached, server.as_ref(), offline, &private, &public)?);
            Ok(())
        }
        Cmd::Detect { no_probe, devices_dir } => osc_bridge::mcp::print_detection(devices_dir.as_deref(), !no_probe),
        Cmd::Inspect { device } => cmd_inspect(device),
        Cmd::Run(args) => cmd_run(args),
        Cmd::Export { device, on: Some(on), yes, .. } => {
            let dev = osc_bridge::device::load(osc_bridge::catalogue::locate(&device)?)?;
            let target = osc_bridge::device::load(osc_bridge::catalogue::locate(&on)?)?;
            let mut midi = osc_bridge::take::Hardware::open()
                .map_err(|e| anyhow::anyhow!("{} cannot be reached: MIDI: {e}", osc_bridge::export::device_title(&target)))?;
            let done = osc_bridge::install::install(&dev, &target, yes, &mut midi, &mut osc_bridge::programs::SystemClock)
                .map_err(anyhow::Error::msg)?;
            for l in &done.lost {
                eprintln!("not carried: {}: {}", l.place, l.reason);
            }
            if done.written {
                println!("wrote {} ({} bytes, {}, by {}): {} message(s) sent", done.path, done.size, done.format, done.way, done.messages);
            } else {
                println!("nothing sent: it would write {} ({} bytes, {}, by {}) and may overwrite a file on the device; add --yes to send",
                    done.path, done.size, done.format, done.way);
            }
            Ok(())
        }
        Cmd::Export { device, format, .. } => {
            let dev = osc_bridge::device::load(osc_bridge::catalogue::locate(&device)?)?;
            let format = format.ok_or_else(|| anyhow::anyhow!("--format is required without --on"))?;
            let out = osc_bridge::export::export(&dev, format.parse()?)?;
            for l in &out.lost {
                eprintln!("not carried: {}: {}", l.place, l.reason);
            }
            print!("{}", out.value);
            Ok(())
        }
        Cmd::OscSend { target, addr, args, from_file } => cmd_osc_send(target, addr, args, from_file),
        Cmd::OscListen { bind } => cmd_osc_listen(bind),
        Cmd::Lint { device } => cmd_lint(device),
        Cmd::Orchestrate { config } => osc_bridge::orchestrator::Orchestrator::run(&config),
        Cmd::Mcp { devices_dir, default_target, offline } => {
            let default_target = default_target.parse::<std::net::SocketAddr>()
                .with_context(|| format!("parse --default-target {default_target}"))?;
            let settings = match osc_bridge::online::settings_file() {
                Some(path) => osc_bridge::online::Settings::load_or_create(&path)?,
                None => osc_bridge::online::Settings::default(),
            };
            let online = !offline && !settings.offline && !osc_bridge::online::offline_by_env();
            let opts = osc_bridge::mcp::McpOptions { devices_dir, default_target, online, private: settings.private };
            osc_bridge::mcp::run(opts)
        }
    }
}

fn cmd_backup(device: PathBuf, in_port: String, out: PathBuf, out_port: Option<String>,
              idle_secs: u64, max_secs: u64) -> Result<()> {
    use osc_bridge::librarian::{backup_from_device, write_syx};
    use std::time::Duration;
    let dev = osc_bridge::device::load(osc_bridge::catalogue::locate(&device)?)?;
    if out_port.is_none() {
        eprintln!("Listening — start the SysEx dump from the device's menu now.");
    }
    let cap = backup_from_device(&dev, &in_port, out_port.as_deref(),
                                 Duration::from_secs(idle_secs), Duration::from_secs(max_secs))?;
    if cap.messages.is_empty() {
        anyhow::bail!("no SysEx received within {max_secs} s");
    }
    write_syx(&out, &cap.messages)?;
    eprintln!("Saved {} message(s), {} bytes to {}", cap.messages.len(), cap.bytes, out.display());
    Ok(())
}

fn cmd_lint(path: PathBuf) -> Result<()> {
    let dev = osc_bridge::device::load(&path)?;
    let mut warnings = 0usize;
    let errors = 0usize;
    let warn = |what: &str, where_: &str, w: &mut usize| {
        eprintln!("  WARN: scripted fallback at {where_} ({what}) — prefer declarative if possible.");
        *w += 1;
    };
    for line in checksum_warnings(&dev) { eprintln!("{line}"); warnings += 1; }
    for c in dev.commands.iter().flatten() {
        if c.script.is_some() { warn("command.script", &format!("/{}", osc_bridge::device::address_text(&c.osc)), &mut warnings); }
    }
    for r in dev.replies.iter().flatten() {
        if r.script.is_some() { warn("reply.script", &osc_bridge::device::emit_text(r), &mut warnings); }
    }
    if let Some(pt) = &dev.params {
        for e in &pt.entries {
            if e.transform.is_some() { warn("param.transform", &e.osc, &mut warnings); }
            if e.transform_reverse.is_some() { warn("param.transform_reverse", &e.osc, &mut warnings); }
        }
    }
    if let Some(pt) = &dev.cc_params {
        for e in &pt.entries {
            if e.transform.is_some() { warn("cc_param.transform", &e.osc, &mut warnings); }
            if e.transform_reverse.is_some() { warn("cc_param.transform_reverse", &e.osc, &mut warnings); }
        }
    }
    println!("{}: parsed OK — {} commands, {} replies, {} params, {} cc_params",
        path.display(),
        dev.commands.as_ref().map_or(0, Vec::len),
        dev.replies.as_ref().map_or(0, Vec::len),
        dev.params.as_ref().map(|p| p.entries.len()).unwrap_or(0),
        dev.cc_params.as_ref().map(|p| p.entries.len()).unwrap_or(0),
    );
    if warnings > 0 { eprintln!("{warnings} warning(s)."); }
    if errors > 0 {
        eprintln!("{errors} error(s)."); std::process::exit(1);
    }
    Ok(())
}

/// One `WARN` line per checksum the engine cannot compute; the build of that frame fails.
fn checksum_warnings(dev: &Device) -> Vec<String> {
    let faults = osc_bridge::device::faults(dev).into_iter()
        .map(|f| format!("  WARN: {} {}: {} — detection sends the universal request instead.", f.device, f.field, f.cause));
    let checksums = osc_bridge::device::checksum_problems(dev).into_iter()
        .map(|(place, problem)| format!("  WARN: checksum at {place}: {problem} — building this frame fails."));
    faults.chain(checksums).collect()
}

fn cmd_list() -> Result<()> {
    let input = MidiInput::new("osc-bridge-list")?;
    println!("=== MIDI inputs ===");
    for (i, p) in input.ports().iter().enumerate() {
        println!("  [{i}] {}", input.port_name(p).unwrap_or_else(|_| "?".into()));
    }
    let output = MidiOutput::new("osc-bridge-list")?;
    println!("=== MIDI outputs ===");
    for (i, p) in output.ports().iter().enumerate() {
        println!("  [{i}] {}", output.port_name(p).unwrap_or_else(|_| "?".into()));
    }
    Ok(())
}

fn cmd_inspect(path: PathBuf) -> Result<()> {
    let dev = osc_bridge::device::load(osc_bridge::catalogue::locate(&path)?)?;
    for line in checksum_warnings(&dev) { eprintln!("{line}"); }
    let d = &dev.device;
    println!("Device:   {} ({})", d.name, d.revision.as_deref().unwrap_or_default());
    println!("Vendor:   {}", d.vendor.as_deref().unwrap_or_default());
    println!("OSC prefix: {}", d.osc_prefix);
    println!("MID:      {}", hex_vec(d.manufacturer_id.as_deref().unwrap_or_default()));
    println!("Dev id:   {}", hex_vec(d.device_id.as_deref().unwrap_or_default()));
    println!("Rate limit: {:?} Hz", d.rate_limit_hz);
    let commands = dev.commands.as_deref().unwrap_or_default();
    println!("Commands: {} entries", commands.len());
    for c in commands {
        println!("  {}  args={}", osc_bridge::device::address_text(&c.osc), c.args.as_ref().map_or(0, Vec::len));
    }
    if let Some(pt) = &dev.params {
        println!("Params:   {} entries", pt.entries.len());
        for e in pt.entries.iter().take(5) {
            let [lo, hi] = e.range.unwrap_or([0, 127]);
            println!("  {}  ({:#04x},{:#04x},{:#04x},{:#04x})  range=[{},{}]",
                e.osc, e.pr.unwrap_or(0), e.p.unwrap_or(0), e.c.unwrap_or(0), e.r.unwrap_or(0), lo, hi);
        }
        if pt.entries.len() > 5 { println!("  ... +{} more", pt.entries.len() - 5); }
    }
    println!("Replies:  {} patterns", dev.replies.as_ref().map_or(0, Vec::len));
    Ok(())
}

fn hex_vec(v: &[u8]) -> String {
    v.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
}

fn cmd_run(args: RunArgs) -> Result<()> {
    let RunArgs { device: path, out_port, in_port, bind, osc_clients: clients, ws_bind, ws_allow_origins, oscquery_bind } = args;
    let dev = osc_bridge::device::load_with_docs(osc_bridge::catalogue::locate(&path)?)?;
    let is_osc = dev.device.transport.as_ref().map(|t| t.kind) == Some(osd::TransportKind::Osc);
    let out_port = match (out_port, is_osc) {
        (Some(p), _) => p,
        (None, true) => 0, // runtime branches early; value is unused.
        (None, false) => anyhow::bail!(
            "--out-port is required for hardware (MIDI/SysEx) drivers. Run `osc-bridge list` to find the index, then pass `--out-port <N>`."
        ),
    };
    if is_osc && in_port.is_some() {
        eprintln!("note: --in-port is ignored for OSC-transport drivers");
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

fn cmd_osc_send(target: String, addr: String, args: Vec<String>, from_file: Option<PathBuf>) -> Result<()> {
    use rosc::{OscMessage, OscPacket, OscType, encoder};
    let mut osc_args: Vec<OscType> = args.iter().map(|s| {
        if let Ok(i) = s.parse::<i32>() { OscType::Int(i) }
        else if let Ok(f) = s.parse::<f32>() { OscType::Float(f) }
        else { OscType::String(s.clone()) }
    }).collect();
    if let Some(path) = &from_file {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        osc_args.push(OscType::String(s));
    }
    let pkt = OscPacket::Message(OscMessage { addr: addr.clone(), args: osc_args });
    let bytes = encoder::encode(&pkt).map_err(|e| anyhow::anyhow!("osc encode: {e:?}"))?;
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    sock.send_to(&bytes, &target)?;
    eprintln!("Sent {} bytes to {target} {addr} (+{} inline args{})",
             bytes.len(), args.len(),
             from_file.as_ref().map(|p| format!(", file={}", p.display())).unwrap_or_default());
    Ok(())
}

fn cmd_osc_listen(bind: String) -> Result<()> {
    let sock = std::net::UdpSocket::bind(&bind)
        .with_context(|| format!("bind {bind}"))?;
    eprintln!("Listening OSC on {bind}. Ctrl-C to stop.");
    eprintln!("-------------------------------------------------------------");
    let mut buf = [0u8; 65536];
    loop {
        let (n, _from) = sock.recv_from(&mut buf)?;
        match rosc::decoder::decode_udp(&buf[..n]) {
            Ok((_, pkt)) => print_osc(pkt, 0),
            Err(e) => eprintln!("decode err: {e}"),
        }
    }
}

/// Wall-clock time printed before a logged message: `HH:MM:SS.mmm`,
/// milliseconds truncated and zero-padded to three digits.
fn format_log_time(t: &jiff::Zoned) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

fn print_osc(pkt: rosc::OscPacket, indent: usize) {
    use rosc::{OscPacket, OscType};
    let pad = "  ".repeat(indent);
    match pkt {
        OscPacket::Bundle(b) => {
            println!("{pad}BUNDLE @ {:?}", b.timetag);
            for p in b.content { print_osc(p, indent + 1); }
        }
        OscPacket::Message(m) => {
            let args: Vec<String> = m.args.iter().map(|a| match a {
                OscType::Int(i) => i.to_string(),
                OscType::Long(i) => i.to_string(),
                OscType::Float(f) => format!("{f:.3}"),
                OscType::Double(f) => format!("{f:.3}"),
                OscType::String(s) => format!("\"{s}\""),
                OscType::Bool(b) => b.to_string(),
                other => format!("{other:?}"),
            }).collect();
            let now = format_log_time(&jiff::Zoned::now());
            println!("{pad}{now}  {:<40} {}", m.addr, args.join(" "));
        }
    }
}
