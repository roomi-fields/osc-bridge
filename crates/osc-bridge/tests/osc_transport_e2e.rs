//! End-to-end test for the OSC transport. Spawns the bridge in a thread
//! against a fake AbletonOSC running on the loopback, and verifies:
//!   1. The startup `subscriptions[on=startup]` is fired once to the target.
//!   2. An incoming OSC command on the bridge's bind port forwards correctly
//!      to the target with placeholder substitution.
//!   3. A reply pushed from the target to the bridge's `reply_port` re-emits
//!      to the configured osc-client with the device prefix.
//!
//! No hardware required.

use osc_bridge::device::{
    ArgSpec, Command, Device, DeviceMeta, ForwardArg, ForwardSpec, OscArgPattern,
    ReplyPattern, Subscription, Sysex, Transport,
};
use osc_bridge::{Runtime, RuntimeOptions};
use rosc::{OscMessage, OscPacket, OscType};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// Deadline of every wait: it weighs only on a failure.
const DEADLINE: Duration = Duration::from_secs(10);
/// One short read of a wait.
const SHORT_READ: Duration = Duration::from_millis(100);
/// Pause between two readiness probes.
const PROBE_EVERY: Duration = Duration::from_millis(50);

/// Waits on `sock` for a message at `addr`, by short repeated reads until
/// `deadline`; other addresses are ignored. Panics naming `addr` on expiry.
fn recv_osc(sock: &UdpSocket, addr: &str, deadline: Duration) -> OscMessage {
    sock.set_read_timeout(Some(SHORT_READ)).unwrap();
    let end = Instant::now() + deadline;
    let mut buf = [0u8; 65536];
    while Instant::now() < end {
        if let Ok((n, _)) = sock.recv_from(&mut buf) {
            if let Ok((_, OscPacket::Message(m))) = rosc::decoder::decode_udp(&buf[..n]) {
                if m.addr == addr {
                    return m;
                }
            }
        }
    }
    panic!("no OSC message at {addr} arrived within {deadline:?}");
}

/// The bridge is ready when it answers /bridge/status: probe every 50 ms from
/// a dedicated socket until the first /bridge/status/device or the deadline.
fn wait_bridge_ready(bridge_in_port: u16) {
    let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
    probe.set_read_timeout(Some(PROBE_EVERY)).unwrap();
    let q = rosc::encoder::encode(&OscPacket::Message(OscMessage {
        addr: "/bridge/status".into(),
        args: vec![],
    }))
    .unwrap();
    let end = Instant::now() + DEADLINE;
    let mut buf = [0u8; 65536];
    while Instant::now() < end {
        probe.send_to(&q, format!("127.0.0.1:{bridge_in_port}")).unwrap();
        if let Ok((n, _)) = probe.recv_from(&mut buf) {
            if let Ok((_, OscPacket::Message(m))) = rosc::decoder::decode_udp(&buf[..n]) {
                if m.addr == "/bridge/status/device" {
                    return;
                }
            }
        }
    }
    panic!("bridge never answered /bridge/status within {DEADLINE:?}");
}

#[test]
#[should_panic(expected = "/never/sent")]
fn recv_osc_fails_naming_the_expected_address_on_an_unfed_socket_criteria_5_6() {
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    recv_osc(&silent, "/never/sent", Duration::from_millis(300));
}

#[test]
fn recv_osc_ignores_other_addresses_criteria_5_8() {
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    for a in ["/bridge/status/device", "/wanted"] {
        let p = OscPacket::Message(OscMessage { addr: a.into(), args: vec![] });
        tx.send_to(&rosc::encoder::encode(&p).unwrap(), rx.local_addr().unwrap()).unwrap();
    }
    assert_eq!(recv_osc(&rx, "/wanted", DEADLINE).addr, "/wanted");
}

fn pick_free_port() -> u16 {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.local_addr().unwrap().port()
}

fn ableton_like(target_port: u16, reply_port: u16) -> Device {
    Device {
        device: DeviceMeta {
            name: "Ableton Live (fake)".into(),
            vendor: "Ableton".into(),
            revision: "Live 12.1 + AbletonOSC 0.4".into(),
            osc_prefix: "/ableton".into(),
            manufacturer_id: vec![],
            device_id: vec![],
            rate_limit_hz: None,
            kind: Some("software".into()),
            transport: Some(Transport {
                kind: "osc".into(),
                host: Some("127.0.0.1".into()),
                port: Some(target_port),
                reply_port: Some(reply_port),
                rate_limit_hz: None,
                passthrough_prefix: None,
            }),
        },
        sysex: Sysex::default(),
        commands: vec![
            Command {
                osc: "/transport/tempo".into(),
                args: vec![ArgSpec {
                    name: "bpm".into(),
                    ty: "float".into(),
                    range: None, values: None, default: None,
                }],
                pre: vec![],
                frame: vec![],
                forward: Some(ForwardSpec {
                    path: "/live/song/set/tempo".into(),
                    args: vec![ForwardArg::Str("{bpm}".into())],
                }),
                script: None,
                electra: None,
            },
        ],
        params: None,
        cc_params: None,
        midi_out: None,
        midi_in: Default::default(),
        replies: vec![ReplyPattern {
            match_frame: vec![],
            match_osc: Some("/live/song/get/tempo".into()),
            match_args: vec![OscArgPattern { name: "bpm".into(), ty: "float".into() }],
            emit_osc: "/transport/tempo {bpm}".into(),
            script: None,
        }],
        subscriptions: vec![Subscription {
            on: "startup".into(),
            forward: ForwardSpec {
                path: "/live/song/start_listen/tempo".into(),
                args: vec![],
            },
        }],
        docs: None,
        custom_commands: vec![],
        triggers: None, identity: None, programs: None, midi_of: None, sources: vec![], midi_of_sources: vec![],
    }
}

fn passthrough_like(target_port: u16, reply_port: u16) -> Device {
    Device {
        device: DeviceMeta {
            name: "Passthrough (fake)".into(),
            vendor: "Test".into(),
            revision: "1".into(),
            osc_prefix: "/sonicpi".into(),
            manufacturer_id: vec![],
            device_id: vec![],
            rate_limit_hz: None,
            kind: Some("software".into()),
            transport: Some(Transport {
                kind: "osc".into(),
                host: Some("127.0.0.1".into()),
                port: Some(target_port),
                reply_port: Some(reply_port),
                rate_limit_hz: None,
                passthrough_prefix: Some(String::new()), // strip /sonicpi entirely
            }),
        },
        sysex: Sysex::default(),
        commands: vec![],
        params: None,
        cc_params: None,
        midi_out: None,
        midi_in: Default::default(),
        replies: vec![],
        subscriptions: vec![],
        docs: None,
        custom_commands: vec![],
        triggers: None, identity: None, programs: None, midi_of: None, sources: vec![], midi_of_sources: vec![],
    }
}

#[test]
fn osc_passthrough_roundtrip() {
    let fake_target = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target_port = fake_target.local_addr().unwrap().port();

    let reply_port = pick_free_port();
    let bridge_in_port = pick_free_port();

    let client_listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    let client_addr = client_listener.local_addr().unwrap();

    let opts = RuntimeOptions {
        device: passthrough_like(target_port, reply_port),
        midi_out_port_idx: 0,
        midi_in_port_idx: None,
        osc_bind: format!("127.0.0.1:{bridge_in_port}"),
        osc_clients: vec![client_addr],
        ws_bind: None,
        ws_allow_origins: vec![],
        oscquery_bind: None,
    };

    std::thread::spawn(move || { let _ = Runtime::run(opts); });
    wait_bridge_ready(bridge_in_port);

    // 1. /sonicpi/cue/foo "hello" 42 → /cue/foo "hello" 42 (verbatim args)
    let test_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let bridge_addr: SocketAddr = format!("127.0.0.1:{bridge_in_port}").parse().unwrap();
    let cmd = OscPacket::Message(OscMessage {
        addr: "/sonicpi/cue/foo".into(),
        args: vec![OscType::String("hello".into()), OscType::Int(42)],
    });
    test_sock.send_to(&rosc::encoder::encode(&cmd).unwrap(), bridge_addr).unwrap();

    let m = recv_osc(&fake_target, "/cue/foo", DEADLINE);
    assert_eq!(m.args.len(), 2);
    assert!(matches!(&m.args[0], OscType::String(s) if s == "hello"));
    assert!(matches!(m.args[1], OscType::Int(42)));

    // 2. Target → bridge reply_port: /trigger/foo 1.5 → client sees /sonicpi/trigger/foo 1.5
    let reply_pkt = OscPacket::Message(OscMessage {
        addr: "/trigger/foo".into(),
        args: vec![OscType::Float(1.5)],
    });
    let reply_addr: SocketAddr = format!("127.0.0.1:{reply_port}").parse().unwrap();
    fake_target.send_to(&rosc::encoder::encode(&reply_pkt).unwrap(), reply_addr).unwrap();

    let m = recv_osc(&client_listener, "/sonicpi/trigger/foo", DEADLINE);
    match m.args[0] {
        OscType::Float(f) => assert!((f - 1.5).abs() < 0.01),
        ref other => panic!("expected float, got {other:?}"),
    }
}

#[test]
fn osc_transport_full_roundtrip() {
    // The fake AbletonOSC: bind first so its port is locked in.
    let fake_target = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target_port = fake_target.local_addr().unwrap().port();

    // The bridge's reply-listener port and client-facing bind port.
    let reply_port = pick_free_port();
    let bridge_in_port = pick_free_port();

    // The osc-client listener (what receives reply re-emissions from the bridge).
    let client_listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    let client_addr = client_listener.local_addr().unwrap();

    let opts = RuntimeOptions {
        device: ableton_like(target_port, reply_port),
        midi_out_port_idx: 0,
        midi_in_port_idx: None,
        osc_bind: format!("127.0.0.1:{bridge_in_port}"),
        osc_clients: vec![client_addr],
        ws_bind: None,
        ws_allow_origins: vec![],
        oscquery_bind: None,
    };

    std::thread::spawn(move || { let _ = Runtime::run(opts); });
    wait_bridge_ready(bridge_in_port);

    // 1. Startup subscription reaches the target.
    recv_osc(&fake_target, "/live/song/start_listen/tempo", DEADLINE);

    // 2. /ableton/transport/tempo 124.5 → bridge → /live/song/set/tempo 124.5
    let test_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let bridge_addr: SocketAddr = format!("127.0.0.1:{bridge_in_port}").parse().unwrap();
    let cmd = OscPacket::Message(OscMessage {
        addr: "/ableton/transport/tempo".into(),
        args: vec![OscType::Float(124.5)],
    });
    test_sock.send_to(&rosc::encoder::encode(&cmd).unwrap(), bridge_addr).unwrap();

    let m = recv_osc(&fake_target, "/live/song/set/tempo", DEADLINE);
    assert_eq!(m.args.len(), 1);
    match m.args[0] {
        OscType::Float(f) => assert!((f - 124.5).abs() < 0.01),
        ref other => panic!("expected float, got {other:?}"),
    }

    // 3. Target → bridge reply_port (push from AbletonOSC) →
    //    bridge re-emits to client_listener with /ableton/ prefix.
    let reply_pkt = OscPacket::Message(OscMessage {
        addr: "/live/song/get/tempo".into(),
        args: vec![OscType::Float(126.0)],
    });
    let reply_addr: SocketAddr = format!("127.0.0.1:{reply_port}").parse().unwrap();
    fake_target.send_to(&rosc::encoder::encode(&reply_pkt).unwrap(), reply_addr).unwrap();

    let m = recv_osc(&client_listener, "/ableton/transport/tempo", DEADLINE);
    assert_eq!(m.args.len(), 1);
    match m.args[0] {
        OscType::Float(f) => assert!((f - 126.0).abs() < 0.01),
        ref other => panic!("expected float, got {other:?}"),
    }
}

#[test]
fn bridge_status_answers_a_caller_that_is_not_an_osc_client() {
    let fake_target = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target_port = fake_target.local_addr().unwrap().port();
    let reply_port = pick_free_port();
    let bridge_in_port = pick_free_port();
    let opts = RuntimeOptions {
        device: passthrough_like(target_port, reply_port),
        midi_out_port_idx: 0,
        midi_in_port_idx: None,
        osc_bind: format!("127.0.0.1:{bridge_in_port}"),
        osc_clients: vec![],
        ws_bind: None,
        ws_allow_origins: vec![],
        oscquery_bind: None,
    };
    std::thread::spawn(move || { let _ = Runtime::run(opts); });
    wait_bridge_ready(bridge_in_port);

    // The MCP get_status tool asks from an ephemeral port like this one.
    let caller = UdpSocket::bind("127.0.0.1:0").unwrap();
    let q = OscPacket::Message(OscMessage { addr: "/bridge/status".into(), args: vec![] });
    caller.send_to(&rosc::encoder::encode(&q).unwrap(), format!("127.0.0.1:{bridge_in_port}")).unwrap();

    let m = recv_osc(&caller, "/bridge/status/device", DEADLINE);
    assert!(matches!(&m.args[0], OscType::String(s) if s == "sonicpi"));
}
