use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use midir::{MidiInput, MidiOutput};
use std::path::PathBuf;

use osc_bridge::{Device, Runtime, RuntimeOptions};

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
    /// Start the dump from the device's menu, or pass --request.
    Backup {
        /// MIDI IN port the device sends on (index or name fragment).
        #[arg(long)]
        in_port: String,
        /// Output .syx file.
        #[arg(long)]
        out: PathBuf,
        /// Dump-request SysEx to send first, as hex ("F0 43 20 00 F7").
        #[arg(long)]
        request: Option<String>,
        /// MIDI OUT port for --request (index or name fragment).
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
        /// MIDI OUT port of the device (index or name fragment).
        #[arg(long)]
        out_port: String,
        /// The .syx file to send.
        file: PathBuf,
        /// Pause between messages, for devices that drop fast dumps.
        #[arg(long, default_value_t = 50)]
        gap_ms: u64,
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
    /// Sharing is on by default: the catalogue stays up to date and reports
    /// and fixes go to the server under an anonymous account.
    Privacy {
        /// `on` cuts the link to the server for every `osc-bridge mcp` run
        /// (the bundled catalogue is used, nothing is sent); `off` restores it.
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
        /// Root directory of the device catalogue.
        #[arg(long, default_value = "devices")]
        devices_dir: PathBuf,
    },
    /// Print summary of a device JSON spec without starting the bridge.
    Inspect {
        device: PathBuf,
    },
    /// Run the bridge for a given device JSON.
    Run {
        /// Path to the device JSON (e.g. devices/arturia/minilab3.json).
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
    },
    /// Write a device JSON in another format on stdout: `oscquery` (its OSC
    /// namespace, JSON) or `midnam` (its MIDI names, MIDI Name Document XML).
    Export {
        /// Path to the device JSON (e.g. devices/korg/minilogue.json).
        #[arg(long)]
        device: PathBuf,
        #[arg(long, value_parser = osc_bridge::export::FORMATS)]
        format: String,
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
        /// Root directory to scan for device JSONs. Defaults to `./devices`.
        #[arg(long, default_value = "devices")]
        devices_dir: PathBuf,
        /// Default OSC target for the `send` and `get_status` tools when no
        /// `target` argument is supplied per call. Defaults to 127.0.0.1:7777
        /// (matching the standard `osc-bridge run` bind).
        #[arg(long, default_value = "127.0.0.1:7777")]
        default_target: String,
        /// Do not contact the SynthHub server for this run (also
        /// OSC_BRIDGE_OFFLINE=1; `osc-bridge privacy --offline on` for good).
        /// By default a local copy keeps its catalogue up to date from the
        /// server — the drivers other users and agents fixed and confirmed —
        /// shows how far each driver is confirmed, and sends report_result
        /// and the contribution tools there, under an anonymous account kept
        /// on this machine. MIDI ports are never shared.
        #[arg(long)]
        offline: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::List => cmd_list(),
        Cmd::Backup { in_port, out, request, out_port, idle_secs, max_secs } =>
            cmd_backup(in_port, out, request, out_port, idle_secs, max_secs),
        Cmd::Restore { out_port, file, gap_ms } => {
            use osc_bridge::librarian::{read_syx, send_messages};
            let msgs = read_syx(&file)?;
            send_messages(&out_port, &msgs, std::time::Duration::from_millis(gap_ms))?;
            eprintln!("Sent {} SysEx message(s) from {}", msgs.len(), file.display());
            Ok(())
        }
        Cmd::Connect { server, no_share, test_synth, private_ports } =>
            osc_bridge::connect::run(&server, !no_share, test_synth, &private_ports),
        Cmd::Privacy { offline, private, public } => {
            let path = osc_bridge::online::settings_file().context("no configuration directory (HOME is not set)")?;
            let devices = osc_bridge::online::usable_cache()
                .unwrap_or_else(|| osc_bridge::catalogue::resolve(std::path::Path::new("devices")));
            print!("{}", osc_bridge::mcp::privacy(&path, &devices, offline.map(|o| o == "on"), &private, &public)?);
            Ok(())
        }
        Cmd::Detect { no_probe, devices_dir } => osc_bridge::mcp::print_detection(&devices_dir, !no_probe),
        Cmd::Inspect { device } => cmd_inspect(device),
        Cmd::Run { device, out_port, in_port, bind, osc_clients, ws_bind, ws_allow_origins, oscquery_bind } =>
            cmd_run(device, out_port, in_port, bind, osc_clients, ws_bind, ws_allow_origins, oscquery_bind),
        Cmd::Export { device, format } => {
            let dev = Device::load(&device)?;
            print!("{}", osc_bridge::export::export(&dev, format.parse()?)?);
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

fn cmd_backup(in_port: String, out: PathBuf, request: Option<String>, out_port: Option<String>,
              idle_secs: u64, max_secs: u64) -> Result<()> {
    use osc_bridge::librarian::{capture, parse_hex, write_syx};
    use std::time::Duration;
    let req = match (&request, &out_port) {
        (Some(hex), Some(port)) => Some((port.as_str(), parse_hex(hex)?)),
        (Some(_), None) => anyhow::bail!("--request needs --out-port"),
        _ => None,
    };
    if req.is_none() {
        eprintln!("Listening — start the SysEx dump from the device's menu now.");
    }
    let cap = capture(&in_port, req.as_ref().map(|(p, b)| (*p, b.as_slice())),
                      Duration::from_secs(idle_secs), Duration::from_secs(max_secs))?;
    if cap.messages.is_empty() {
        anyhow::bail!("no SysEx received within {max_secs} s");
    }
    write_syx(&out, &cap.messages)?;
    eprintln!("Saved {} message(s), {} bytes to {}", cap.messages.len(), cap.bytes, out.display());
    Ok(())
}

fn cmd_lint(path: PathBuf) -> Result<()> {
    let dev = Device::load(&path)?;
    let mut warnings = 0usize;
    let errors = 0usize;
    let warn = |what: &str, where_: &str, w: &mut usize| {
        eprintln!("  WARN: scripted fallback at {where_} ({what}) — prefer declarative if possible.");
        *w += 1;
    };
    for c in &dev.commands {
        if c.script.is_some() { warn("command.script", &format!("/{}", c.osc), &mut warnings); }
    }
    for r in &dev.replies {
        if r.script.is_some() { warn("reply.script", &r.emit_osc, &mut warnings); }
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
        dev.commands.len(),
        dev.replies.len(),
        dev.params.as_ref().map(|p| p.entries.len()).unwrap_or(0),
        dev.cc_params.as_ref().map(|p| p.entries.len()).unwrap_or(0),
    );
    if warnings > 0 { eprintln!("{warnings} scripted-fallback warning(s)."); }
    if errors > 0 {
        eprintln!("{errors} error(s)."); std::process::exit(1);
    }
    Ok(())
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
    let dev = Device::load(&path)?;
    println!("Device:   {} ({})", dev.device.name, dev.device.revision);
    println!("Vendor:   {}", dev.device.vendor);
    println!("OSC prefix: {}", dev.device.osc_prefix);
    println!("MID:      {}", hex_vec(&dev.device.manufacturer_id));
    println!("Dev id:   {}", hex_vec(&dev.device.device_id));
    println!("Rate limit: {:?} Hz", dev.device.rate_limit_hz);
    println!("Commands: {} entries", dev.commands.len());
    for c in &dev.commands {
        println!("  {}  args={}", c.osc, c.args.len());
    }
    if let Some(pt) = &dev.params {
        println!("Params:   {} entries", pt.entries.len());
        for e in pt.entries.iter().take(5) {
            println!("  {}  ({:#04x},{:#04x},{:#04x},{:#04x})  range=[{},{}]",
                e.osc, e.pr, e.p, e.c, e.r, e.range[0], e.range[1]);
        }
        if pt.entries.len() > 5 { println!("  ... +{} more", pt.entries.len() - 5); }
    }
    println!("Replies:  {} patterns", dev.replies.len());
    Ok(())
}

fn hex_vec(v: &[u8]) -> String {
    v.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
}

fn cmd_run(path: PathBuf, out_port: Option<usize>, in_port: Option<usize>, bind: String, clients: Vec<String>,
           ws_bind: Option<String>, ws_allow_origins: Vec<String>, oscquery_bind: Option<String>) -> Result<()> {
    let dev = Device::load(&path)?;
    let is_osc = dev.device.transport.as_ref().map(|t| t.kind.as_str()) == Some("osc");
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
            let now = chrono::Local::now().format("%H:%M:%S%.3f");
            println!("{pad}{now}  {:<40} {}", m.addr, args.join(" "));
        }
    }
}
