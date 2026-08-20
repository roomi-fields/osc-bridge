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
use std::time::Duration;

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
    }
}

#[test]
fn osc_passthrough_roundtrip() {
    let fake_target = UdpSocket::bind("127.0.0.1:0").unwrap();
    fake_target.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let target_port = fake_target.local_addr().unwrap().port();

    let reply_port = pick_free_port();
    let bridge_in_port = pick_free_port();

    let client_listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    client_listener.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let client_addr = client_listener.local_addr().unwrap();

    let opts = RuntimeOptions {
        device: passthrough_like(target_port, reply_port),
        midi_out_port_idx: 0,
        midi_in_port_idx: None,
        osc_bind: format!("127.0.0.1:{bridge_in_port}"),
        osc_clients: vec![client_addr],
        ws_bind: None,
    };

    std::thread::spawn(move || { let _ = Runtime::run(opts); });
    std::thread::sleep(Duration::from_millis(300));

    let mut buf = [0u8; 65536];

    // 1. /sonicpi/cue/foo "hello" 42 → /cue/foo "hello" 42 (verbatim args)
    let test_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let bridge_addr: SocketAddr = format!("127.0.0.1:{bridge_in_port}").parse().unwrap();
    let cmd = OscPacket::Message(OscMessage {
        addr: "/sonicpi/cue/foo".into(),
        args: vec![OscType::String("hello".into()), OscType::Int(42)],
    });
    test_sock.send_to(&rosc::encoder::encode(&cmd).unwrap(), bridge_addr).unwrap();

    let (n, _) = fake_target.recv_from(&mut buf).expect("passthrough forward");
    let (_, pkt) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
    match pkt {
        OscPacket::Message(m) => {
            assert_eq!(m.addr, "/cue/foo");
            assert_eq!(m.args.len(), 2);
            assert!(matches!(&m.args[0], OscType::String(s) if s == "hello"));
            assert!(matches!(m.args[1], OscType::Int(42)));
        }
        other => panic!("expected message, got {other:?}"),
    }

    // 2. Target → bridge reply_port: /trigger/foo 1.5 → client sees /sonicpi/trigger/foo 1.5
    let reply_pkt = OscPacket::Message(OscMessage {
        addr: "/trigger/foo".into(),
        args: vec![OscType::Float(1.5)],
    });
    let reply_addr: SocketAddr = format!("127.0.0.1:{reply_port}").parse().unwrap();
    fake_target.send_to(&rosc::encoder::encode(&reply_pkt).unwrap(), reply_addr).unwrap();

    let (n, _) = client_listener.recv_from(&mut buf).expect("passthrough reply");
    let (_, pkt) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
    match pkt {
        OscPacket::Message(m) => {
            assert_eq!(m.addr, "/sonicpi/trigger/foo");
            match m.args[0] {
                OscType::Float(f) => assert!((f - 1.5).abs() < 0.01),
                ref other => panic!("expected float, got {other:?}"),
            }
        }
        other => panic!("expected message, got {other:?}"),
    }
}

#[test]
fn osc_transport_full_roundtrip() {
    // The fake AbletonOSC: bind first so its port is locked in.
    let fake_target = UdpSocket::bind("127.0.0.1:0").unwrap();
    fake_target.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let target_port = fake_target.local_addr().unwrap().port();

    // The bridge's reply-listener port and client-facing bind port.
    let reply_port = pick_free_port();
    let bridge_in_port = pick_free_port();

    // The osc-client listener (what receives reply re-emissions from the bridge).
    let client_listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    client_listener.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let client_addr = client_listener.local_addr().unwrap();

    let opts = RuntimeOptions {
        device: ableton_like(target_port, reply_port),
        midi_out_port_idx: 0,
        midi_in_port_idx: None,
        osc_bind: format!("127.0.0.1:{bridge_in_port}"),
        osc_clients: vec![client_addr],
        ws_bind: None,
    };

    std::thread::spawn(move || { let _ = Runtime::run(opts); });
    std::thread::sleep(Duration::from_millis(300));

    let mut buf = [0u8; 65536];

    // 1. Startup subscription reaches the target.
    let (n, _) = fake_target.recv_from(&mut buf).expect("subscription should arrive");
    let (_, pkt) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
    match pkt {
        OscPacket::Message(m) => assert_eq!(m.addr, "/live/song/start_listen/tempo"),
        other => panic!("expected subscription message, got {other:?}"),
    }

    // 2. /ableton/transport/tempo 124.5 → bridge → /live/song/set/tempo 124.5
    let test_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let bridge_addr: SocketAddr = format!("127.0.0.1:{bridge_in_port}").parse().unwrap();
    let cmd = OscPacket::Message(OscMessage {
        addr: "/ableton/transport/tempo".into(),
        args: vec![OscType::Float(124.5)],
    });
    test_sock.send_to(&rosc::encoder::encode(&cmd).unwrap(), bridge_addr).unwrap();

    let (n, _) = fake_target.recv_from(&mut buf).expect("forward should arrive");
    let (_, pkt) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
    match pkt {
        OscPacket::Message(m) => {
            assert_eq!(m.addr, "/live/song/set/tempo");
            assert_eq!(m.args.len(), 1);
            match m.args[0] {
                OscType::Float(f) => assert!((f - 124.5).abs() < 0.01),
                ref other => panic!("expected float, got {other:?}"),
            }
        }
        other => panic!("expected message, got {other:?}"),
    }

    // 3. Target → bridge reply_port (push from AbletonOSC) →
    //    bridge re-emits to client_listener with /ableton/ prefix.
    let reply_pkt = OscPacket::Message(OscMessage {
        addr: "/live/song/get/tempo".into(),
        args: vec![OscType::Float(126.0)],
    });
    let reply_addr: SocketAddr = format!("127.0.0.1:{reply_port}").parse().unwrap();
    fake_target.send_to(&rosc::encoder::encode(&reply_pkt).unwrap(), reply_addr).unwrap();

    let (n, _) = client_listener.recv_from(&mut buf).expect("reply re-emission should arrive");
    let (_, pkt) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
    match pkt {
        OscPacket::Message(m) => {
            assert_eq!(m.addr, "/ableton/transport/tempo");
            assert_eq!(m.args.len(), 1);
            match m.args[0] {
                OscType::Float(f) => assert!((f - 126.0).abs() < 0.01),
                ref other => panic!("expected float, got {other:?}"),
            }
        }
        other => panic!("expected message, got {other:?}"),
    }
}
