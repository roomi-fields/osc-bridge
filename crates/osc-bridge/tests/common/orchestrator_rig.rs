//! The shared start of an orchestrator in the tests.
//!
//! The rig holds the result of `Orchestrator::run` (a channel), so it knows
//! when the orchestrator ended and reads its error. It judges the orchestrator
//! ready when the control forward it asked for (`/rig/ping`, a route to a
//! sentinel device whose target is the rig's own private socket) arrives on
//! that socket: an answer from a stranger holding the port is not readiness.
//! The question is asked again until answered (UDP does not resend). When
//! `run` ends with an error before the rig is ready (a port already taken),
//! the rig starts again on a fresh port, at most `MAX_STARTS` starts in all.

#![allow(dead_code)]

use osc_bridge::orchestrator::Orchestrator;
use rosc::{OscMessage, OscPacket};
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Starts of the orchestrator the rig makes at most.
pub const MAX_STARTS: usize = 3;
/// The control message the rig sends, and what the sentinel forwards.
pub const RIG_PING: &str = "/rig/ping";
const RIG_FORWARD: &str = "/fx/rigping";
/// A guard rail on one start, far above its normal length.
const DEADLINE: Duration = Duration::from_secs(10);

/// A running orchestrator the rig has judged its own.
pub struct Started {
    /// The address of the orchestrator's OSC input.
    pub orch: SocketAddr,
    /// The folder holding the configuration and the device files.
    pub dir: tempfile::TempDir,
    /// How many starts the rig made (1 when the first one held).
    pub starts: usize,
    _sentinel_target: UdpSocket,
}

fn free_udp() -> u16 {
    UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Start an orchestrator. `config(osc_port, dir)` writes what it needs under
/// `dir` and returns `(osc_extra, body)`: more keys of the `[osc]` table, and
/// the `[[devices]]` and `[[routes]]` tables. The rig adds its sentinel.
pub fn start_orchestrator(tag: &str, config: impl Fn(u16, &Path) -> (String, String)) -> Started {
    start_orchestrator_picking(tag, free_udp, config)
}

/// `start_orchestrator`, the OSC port of each start drawn by `pick_port`.
pub fn start_orchestrator_picking(
    tag: &str,
    mut pick_port: impl FnMut() -> u16,
    config: impl Fn(u16, &Path) -> (String, String),
) -> Started {
    let dir = crate::common::temp(tag);
    let target = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target_port = target.local_addr().unwrap().port();
    let sentinel = dir.path().join("rig-sentinel.json");
    std::fs::write(&sentinel, format!(r#"{{
        "device": {{ "name": "Rig sentinel", "vendor": "Test", "osc_prefix": "/rigsentinel", "kind": "software",
            "transport": {{ "kind": "osc", "host": "127.0.0.1", "port": {target_port} }} }},
        "commands": [{{ "osc": "/ping", "forward": {{ "path": "{RIG_FORWARD}" }} }}]
    }}"#)).unwrap();
    target.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let ping = rosc::encoder::encode(&OscPacket::Message(OscMessage { addr: RIG_PING.into(), args: vec![] })).unwrap();
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();

    let mut last_error = String::from("none");
    let mut sends_total = 0usize;
    let mut last_addr = String::new();
    for start in 1..=MAX_STARTS {
        let port = pick_port();
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        last_addr = addr.to_string();
        let (osc_extra, body) = config(port, dir.path());
        let toml = dir.path().join(format!("bridge-{start}.toml"));
        std::fs::write(&toml, format!(r#"
[osc]
bind = "{addr}"
{osc_extra}

[[devices]]
spec = "{}"

{body}

[[routes]]
from = "{RIG_PING}"
to = "/rigsentinel/ping"
"#, sentinel.display().to_string().replace('\\', "\\\\"))).unwrap();

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = Orchestrator::run(&toml, |spec: &Path| Ok(spec.to_path_buf()));
            let _ = tx.send(result.map(|()| "returned without an error".to_string()).unwrap_or_else(|e| format!("{e:#}")));
        });

        let end = Instant::now() + DEADLINE;
        let mut buf = [0u8; 65536];
        let mut ended = false;
        while Instant::now() < end {
            client.send_to(&ping, addr).unwrap();
            sends_total += 1;
            if let Ok((n, _)) = target.recv_from(&mut buf) {
                if let Ok((_, OscPacket::Message(m))) = rosc::decoder::decode_udp(&buf[..n]) {
                    if m.addr == RIG_FORWARD {
                        return Started { orch: addr, dir, starts: start, _sentinel_target: target };
                    }
                }
            }
            if let Ok(error) = rx.try_recv() {
                last_error = error;
                ended = true;
                break;
            }
        }
        if !ended {
            last_error = format!("Orchestrator::run had not ended and its forward never reached the rig within {DEADLINE:?}");
        }
    }
    panic!("the orchestrator did not start after {MAX_STARTS} starts; last Orchestrator::run error: {last_error}; \
        the rig sent {RIG_PING} to {last_addr}, {sends_total} times in all");
}
