//! WebSocket transport tests. Two layers:
//!
//! 1. Transport-level: `ws::serve` + `WsClients` in isolation — binary frames
//!    decode to OSC packets and reach the dispatch callback; broadcasts reach
//!    every connected client; text frames are ignored; a disconnected client
//!    is pruned from the registry.
//! 2. End-to-end: a full `Runtime::run` on an OSC-passthrough device with
//!    `ws_bind` set — a WS client's frames flow to the UDP target and the
//!    target's replies flow back to the WS client, proving "WS client ≡
//!    --osc-client".
//!
//! No hardware required.

use osc_bridge::device::{Device, DeviceMeta, Sysex, Transport};
use osc_bridge::ws::{self, WsClients, WsDispatch};
use osc_bridge::{Runtime, RuntimeOptions};
use rosc::{OscMessage, OscPacket, OscType};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

fn encode(msg: OscMessage) -> Vec<u8> {
    rosc::encoder::encode(&OscPacket::Message(msg)).unwrap()
}

fn decode(bytes: &[u8]) -> OscMessage {
    match rosc::decoder::decode_udp(bytes).unwrap().1 {
        OscPacket::Message(m) => m,
        other => panic!("expected message, got {other:?}"),
    }
}

fn connect(port: u16) -> WebSocket<MaybeTlsStream<TcpStream>> {
    let (socket, _resp) = tungstenite::connect(format!("ws://127.0.0.1:{port}"))
        .expect("WS connect");
    if let MaybeTlsStream::Plain(s) = socket.get_ref() {
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    }
    socket
}

/// Read frames until a binary one arrives (skipping pings), with a deadline.
fn next_binary(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match socket.read() {
            Ok(Message::Binary(b)) => return b.to_vec(),
            Ok(_) => continue,
            Err(e) => panic!("WS client read: {e}"),
        }
    }
    panic!("no binary frame within deadline");
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if cond() { return; }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timeout waiting for {what}");
}

#[test]
fn ws_binary_frames_reach_dispatch_and_broadcasts_reach_client() {
    let (tx, rx) = mpsc::channel::<OscMessage>();
    let dispatch: WsDispatch = Arc::new(move |pkt, _from| {
        if let OscPacket::Message(m) = pkt { let _ = tx.send(m); }
    });
    let clients = WsClients::new();
    let addr = ws::serve("127.0.0.1:0", clients.clone(), dispatch).unwrap();

    let mut socket = connect(addr.port());
    wait_until("client registration", || clients.has_clients());

    // Inbound: binary WS frame → dispatch.
    let msg = OscMessage {
        addr: "/minilab3/pad/3/color".into(),
        args: vec![OscType::Int(127), OscType::Int(0), OscType::Int(64)],
    };
    socket.send(Message::Binary(encode(msg).into())).unwrap();
    let got = rx.recv_timeout(Duration::from_secs(2)).expect("dispatch");
    assert_eq!(got.addr, "/minilab3/pad/3/color");
    assert_eq!(got.args.len(), 3);

    // Text frames are ignored — nothing must reach dispatch.
    socket.send(Message::Text("not osc".into())).unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());

    // Outbound: broadcast → binary frame on the client, byte-identical OSC.
    let out = OscMessage {
        addr: "/minilab3/knob/1".into(),
        args: vec![OscType::Float(0.5)],
    };
    clients.broadcast(&encode(out));
    let m = decode(&next_binary(&mut socket));
    assert_eq!(m.addr, "/minilab3/knob/1");
    assert!(matches!(m.args[0], OscType::Float(f) if (f - 0.5).abs() < 0.001));
}

#[test]
fn ws_broadcast_fans_out_to_every_client_and_prunes_on_disconnect() {
    let dispatch: WsDispatch = Arc::new(|_pkt, _from| {});
    let clients = WsClients::new();
    let addr = ws::serve("127.0.0.1:0", clients.clone(), dispatch).unwrap();

    let mut a = connect(addr.port());
    let mut b = connect(addr.port());
    wait_until("both clients registered", || clients.client_count() == 2);

    let out = OscMessage { addr: "/bridge/status/device".into(),
                           args: vec![OscType::String("x".into())] };
    clients.broadcast(&encode(out));
    assert_eq!(decode(&next_binary(&mut a)).addr, "/bridge/status/device");
    assert_eq!(decode(&next_binary(&mut b)).addr, "/bridge/status/device");

    // Close one client; the registry prunes it on subsequent broadcasts.
    b.close(None).unwrap();
    drop(b);
    let out = OscMessage { addr: "/x".into(), args: vec![] };
    wait_until("pruning down to one client", || {
        clients.broadcast(&encode(out.clone()));
        clients.client_count() == 1
    });
    // The surviving client still receives frames.
    assert_eq!(decode(&next_binary(&mut a)).addr, "/x");
}

// ---- End-to-end: WS client ≡ --osc-client on a passthrough OSC device ----

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
                passthrough_prefix: Some(String::new()),
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

fn pick_free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn pick_free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn ws_client_end_to_end_on_osc_passthrough_runtime() {
    let fake_target = UdpSocket::bind("127.0.0.1:0").unwrap();
    fake_target.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let target_port = fake_target.local_addr().unwrap().port();

    let reply_port = pick_free_udp_port();
    let bridge_in_port = pick_free_udp_port();
    let ws_port = pick_free_tcp_port();

    let opts = RuntimeOptions {
        device: passthrough_like(target_port, reply_port),
        midi_out_port_idx: 0,
        midi_in_port_idx: None,
        osc_bind: format!("127.0.0.1:{bridge_in_port}"),
        osc_clients: vec![],
        ws_bind: Some(format!("127.0.0.1:{ws_port}")),
    };
    std::thread::spawn(move || { let _ = Runtime::run(opts); });
    std::thread::sleep(Duration::from_millis(300));

    let mut socket = connect(ws_port);

    // WS → bridge → UDP target: /sonicpi/cue/foo strips to /cue/foo.
    let cmd = OscMessage {
        addr: "/sonicpi/cue/foo".into(),
        args: vec![OscType::String("hello".into()), OscType::Int(42)],
    };
    socket.send(Message::Binary(encode(cmd).into())).unwrap();

    let mut buf = [0u8; 65536];
    let (n, _) = fake_target.recv_from(&mut buf).expect("passthrough forward");
    let m = decode(&buf[..n]);
    assert_eq!(m.addr, "/cue/foo");
    assert!(matches!(&m.args[0], OscType::String(s) if s == "hello"));
    assert!(matches!(m.args[1], OscType::Int(42)));

    // UDP target → bridge reply port → WS client: prefixed re-emission.
    let reply = OscMessage { addr: "/trigger/foo".into(),
                             args: vec![OscType::Float(1.5)] };
    fake_target
        .send_to(&encode(reply), format!("127.0.0.1:{reply_port}"))
        .unwrap();
    let m = decode(&next_binary(&mut socket));
    assert_eq!(m.addr, "/sonicpi/trigger/foo");
    assert!(matches!(m.args[0], OscType::Float(f) if (f - 1.5).abs() < 0.01));
}
