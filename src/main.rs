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
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::List => cmd_list(),
        Cmd::Inspect { device } => cmd_inspect(device),
        Cmd::Run { device, out_port, in_port, bind, osc_clients, ws_bind } =>
            cmd_run(device, out_port, in_port, bind, osc_clients, ws_bind),
        Cmd::OscSend { target, addr, args, from_file } => cmd_osc_send(target, addr, args, from_file),
        Cmd::OscListen { bind } => cmd_osc_listen(bind),
        Cmd::Lint { device } => cmd_lint(device),
        Cmd::Orchestrate { config } => osc_bridge::orchestrator::Orchestrator::run(&config),
        Cmd::Mcp { devices_dir, default_target } => {
            let default_target = default_target.parse::<std::net::SocketAddr>()
                .with_context(|| format!("parse --default-target {default_target}"))?;
            osc_bridge::mcp::run(osc_bridge::mcp::McpOptions {
                devices_dir,
                default_target,
            })
        }
    }
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

fn cmd_run(path: PathBuf, out_port: Option<usize>, in_port: Option<usize>, bind: String, clients: Vec<String>, ws_bind: Option<String>) -> Result<()> {
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
