//! End-to-end test for inter-device routing in the orchestrator, using a
//! 100% software (OSC-transport) setup — no MIDI hardware required.
//!
//! Topology:
//!   test client ──/ctrl/knob/1 80──▶ orchestrator
//!                                      │  [[route]] /ctrl/knob/1 → /synth/cutoff
//!                                      │           map [0,127] → [0,1]
//!                                      ▼
//!                              /synth device (OSC transport)
//!                                      │  command /cutoff → forward /fx/cutoff {v}
//!                                      ▼
//!                              fake synth target (UDP loopback)
//!
//! Asserts the fake target receives `/fx/cutoff 0.63` (80/127 ≈ 0.6299).

use osc_bridge::orchestrator::Orchestrator;
use rosc::{OscMessage, OscPacket, OscType};
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::time::Duration;

fn pick_free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn unique_tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "osc-bridge-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn inter_device_route_remaps_and_forwards() {
    // Fake synth target — bind first so its port is reserved.
    let fake_synth = UdpSocket::bind("127.0.0.1:0").unwrap();
    fake_synth.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let synth_port = fake_synth.local_addr().unwrap().port();

    let orch_bind_port = pick_free_port();

    // Temp device JSON: a /synth OSC device whose /cutoff command forwards
    // to /fx/cutoff on the fake target.
    let dir = unique_tmp_dir("routing");
    let synth_json = dir.join("synth.json");
    std::fs::write(&synth_json, format!(r#"{{
        "device": {{
            "name": "Test Synth", "vendor": "Test",
            "osc_prefix": "/synth", "kind": "software",
            "transport": {{ "kind": "osc", "host": "127.0.0.1", "port": {synth_port} }}
        }},
        "commands": [
            {{
                "osc": "/cutoff",
                "args": [{{ "name": "v", "type": "float" }}],
                "forward": {{ "path": "/fx/cutoff", "args": ["{{v}}"] }}
            }}
        ]
    }}"#)).unwrap();

    // Temp bridge.toml: the /synth device + a route from /ctrl/knob/1.
    let bridge_toml = dir.join("bridge.toml");
    std::fs::write(&bridge_toml, format!(r#"
[osc]
bind = "127.0.0.1:{orch_bind_port}"

[[devices]]
spec = "{}"

[[routes]]
from = "/ctrl/knob/1"
to = "/synth/cutoff"
map.from = [0, 127]
map.to = [0, 1]
"#, synth_json.display().to_string().replace('\\', "\\\\"))).unwrap();

    // Spawn the orchestrator.
    let toml_path = bridge_toml.clone();
    std::thread::spawn(move || { let _ = Orchestrator::run(&toml_path); });
    std::thread::sleep(Duration::from_millis(400));

    // Send /ctrl/knob/1 80 to the orchestrator. There is no device owning
    // /ctrl — the message exists purely as a route source.
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    let orch_addr: SocketAddr = format!("127.0.0.1:{orch_bind_port}").parse().unwrap();
    let msg = OscPacket::Message(OscMessage {
        addr: "/ctrl/knob/1".into(),
        args: vec![OscType::Int(80)],
    });
    client.send_to(&rosc::encoder::encode(&msg).unwrap(), orch_addr).unwrap();

    // The route should remap 80 (of 0..127) to ~0.63, dispatch into /synth,
    // whose /cutoff command forwards /fx/cutoff <v> to the fake target.
    let mut buf = [0u8; 65536];
    let (n, _) = fake_synth.recv_from(&mut buf).expect("routed forward should arrive");
    let (_, pkt) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
    match pkt {
        OscPacket::Message(m) => {
            assert_eq!(m.addr, "/fx/cutoff");
            assert_eq!(m.args.len(), 1);
            match m.args[0] {
                OscType::Float(f) => assert!(
                    (f - 0.6299).abs() < 0.01,
                    "expected ~0.63 (80/127), got {f}"
                ),
                ref other => panic!("expected float, got {other:?}"),
            }
        }
        other => panic!("expected message, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn route_without_map_passes_args_through() {
    let fake_synth = UdpSocket::bind("127.0.0.1:0").unwrap();
    fake_synth.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let synth_port = fake_synth.local_addr().unwrap().port();
    let orch_bind_port = pick_free_port();

    let dir = unique_tmp_dir("routing-nomap");
    let synth_json = dir.join("synth.json");
    std::fs::write(&synth_json, format!(r#"{{
        "device": {{
            "name": "Test Synth", "vendor": "Test",
            "osc_prefix": "/synth", "kind": "software",
            "transport": {{ "kind": "osc", "host": "127.0.0.1", "port": {synth_port} }}
        }},
        "commands": [
            {{
                "osc": "/gate",
                "args": [{{ "name": "on", "type": "int" }}],
                "forward": {{ "path": "/fx/gate", "args": ["{{on}}"] }}
            }}
        ]
    }}"#)).unwrap();

    let bridge_toml = dir.join("bridge.toml");
    std::fs::write(&bridge_toml, format!(r#"
[osc]
bind = "127.0.0.1:{orch_bind_port}"

[[devices]]
spec = "{}"

[[routes]]
from = "/pad/{{n}}/hit"
to = "/synth/gate"
"#, synth_json.display().to_string().replace('\\', "\\\\"))).unwrap();

    let toml_path = bridge_toml.clone();
    std::thread::spawn(move || { let _ = Orchestrator::run(&toml_path); });
    std::thread::sleep(Duration::from_millis(400));

    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    let orch_addr: SocketAddr = format!("127.0.0.1:{orch_bind_port}").parse().unwrap();
    // /pad/3/hit 1 — the {n} capture is consumed by the route pattern, the
    // arg (1) passes through unchanged since there is no `map`.
    let msg = OscPacket::Message(OscMessage {
        addr: "/pad/3/hit".into(),
        args: vec![OscType::Int(1)],
    });
    client.send_to(&rosc::encoder::encode(&msg).unwrap(), orch_addr).unwrap();

    let mut buf = [0u8; 65536];
    let (n, _) = fake_synth.recv_from(&mut buf).expect("routed forward should arrive");
    let (_, pkt) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
    match pkt {
        OscPacket::Message(m) => {
            assert_eq!(m.addr, "/fx/gate");
            assert!(matches!(m.args[0], OscType::Int(1)));
        }
        other => panic!("expected message, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}
