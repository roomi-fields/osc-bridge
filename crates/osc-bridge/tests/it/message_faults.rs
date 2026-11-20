//! A message the bridge cannot turn into a frame (or run, or queue) is answered to its sender
//! with `/bridge/fault <address> <reason>` (two strings); `/bridge/sync <id>` in the same packet
//! asks for `/bridge/done <id>` once what precedes it from that sender is handled, after any
//! fault. A message with no real sender (re-injected by a link, set off by an incoming MIDI
//! message) has its fault broadcast to the declared clients through `send_osc`, as
//! `/bridge/status` is. A correct message answers nothing. No socket, no MIDI port.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use crate::common::{device_of, no_reply};
use crossbeam_channel::{bounded, Receiver, Sender};
use osc_bridge::orchestrator::BenchPath;
use osc_bridge::runtime::{dispatch_packet_to_device, dispatch_to_device, DeviceDispatch, ReplyTo, SendOsc};
use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType};

const CUTOFF: &str = "/test_synth/filter/cutoff";

fn driver() -> osd::Device {
    device_of(r#"{
  "device": { "name": "Acme", "vendor": "Acme", "revision": "A1", "osc_prefix": "/acme" },
  "commands": [
    { "osc": "/set", "args": [ { "name": "v", "type": "u7", "range": [0, 127] } ], "frame": [] },
    { "osc": "/boom", "args": [], "frame": [], "script": "error('command script exploded')" }
  ],
  "custom_commands": [
    { "osc": "/cboom", "script": "error('custom script exploded')" }
  ]
}"#)
}

fn synth() -> osd::Device {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/devices/examples/test_synth.json");
    osc_bridge::device::load_with_docs(path).unwrap()
}

/// Everything a dispatch said: to its sender (with the sender's address) and to the clients.
#[derive(Clone, Default)]
struct Said {
    to_sender: Arc<Mutex<Vec<(SocketAddr, OscMessage)>>>,
    broadcast: Arc<Mutex<Vec<OscMessage>>>,
}

impl Said {
    fn reply_to(&self) -> ReplyTo {
        let sink = self.to_sender.clone();
        Arc::new(move |to, m| sink.lock().unwrap().push((to, m)))
    }
    fn send_osc(&self) -> SendOsc {
        let sink = self.broadcast.clone();
        Arc::new(move |m| sink.lock().unwrap().push(m))
    }
    fn to_sender(&self) -> Vec<(SocketAddr, OscMessage)> { self.to_sender.lock().unwrap().clone() }
    fn broadcast(&self) -> Vec<OscMessage> { self.broadcast.lock().unwrap().clone() }
}

fn state(device: osd::Device, room: usize) -> (DeviceDispatch, Receiver<Vec<u8>>, Sender<Vec<u8>>) {
    let (tx, rx) = bounded::<Vec<u8>>(room);
    (DeviceDispatch::build(device, tx.clone()), rx, tx)
}

fn client() -> SocketAddr { "127.0.0.1:9000".parse().unwrap() }
fn nobody() -> SocketAddr { "0.0.0.0:0".parse().unwrap() }

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage { OscMessage { addr: addr.into(), args } }
fn s(t: &str) -> OscType { OscType::String(t.into()) }

fn strings(m: &OscMessage) -> Vec<String> {
    m.args.iter().map(|a| match a { OscType::String(t) => t.clone(), o => panic!("not a string: {o:?}") }).collect()
}

/// The one fault `said` holds for the sender: its address and its reason.
fn the_fault(said: &Said) -> (String, String) {
    let got = said.to_sender();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, client(), "answered to the sender");
    assert_eq!(got[0].1.addr, "/bridge/fault");
    let a = strings(&got[0].1);
    assert_eq!(a.len(), 2, "two strings: the address and the reason: {a:?}");
    (a[0].clone(), a[1].clone())
}

fn send(st: &DeviceDispatch, said: &Said, m: OscMessage, from: SocketAddr) {
    dispatch_to_device(st, m, &said.send_osc(), &said.reply_to(), from);
}

#[test]
fn a_command_that_cannot_be_built_is_a_fault_with_the_address_and_the_reason_to_its_sender() {
    let (st, rx, _tx) = state(driver(), 8);
    let said = Said::default();
    send(&st, &said, msg("/acme/set", vec![]), client());
    let (addr, reason) = the_fault(&said);
    assert_eq!(addr, "/acme/set");
    assert!(reason.contains("missing arg v"), "{reason}");
    assert!(said.broadcast().is_empty(), "a fault with a sender goes to the sender alone");
    assert!(rx.try_recv().is_err(), "nothing was queued");
}

#[test]
fn a_cc_argument_that_is_not_a_number_is_a_fault_to_its_sender() {
    let (st, rx, _tx) = state(synth(), 8);
    let said = Said::default();
    send(&st, &said, msg(CUTOFF, vec![s("loud")]), client());
    let (addr, reason) = the_fault(&said);
    assert_eq!(addr, CUTOFF);
    assert!(!reason.is_empty());
    assert!(rx.try_recv().is_err());
}

#[test]
fn an_address_nobody_handles_is_a_fault_to_its_sender() {
    let (st, _rx, _tx) = state(synth(), 8);
    let said = Said::default();
    send(&st, &said, msg("/test_synth/no/such/thing", vec![OscType::Int(1)]), client());
    let (addr, reason) = the_fault(&said);
    assert_eq!(addr, "/test_synth/no/such/thing");
    assert!(!reason.is_empty());
}

#[test]
fn a_full_emission_queue_is_a_fault_to_its_sender() {
    let (st, rx, _tx) = state(synth(), 1);
    let said = Said::default();
    send(&st, &said, msg(CUTOFF, vec![OscType::Int(10)]), client());
    assert!(said.to_sender().is_empty(), "the first message fits");
    send(&st, &said, msg(CUTOFF, vec![OscType::Int(11)]), client());
    let (addr, reason) = the_fault(&said);
    assert_eq!(addr, CUTOFF);
    assert!(!reason.is_empty());
    assert_eq!(rx.try_iter().count(), 1, "only the first frame is in the queue");
}

#[test]
fn a_command_script_that_fails_is_a_fault_carrying_the_script_error() {
    let (st, _rx, _tx) = state(driver(), 8);
    let said = Said::default();
    send(&st, &said, msg("/acme/boom", vec![]), client());
    let (addr, reason) = the_fault(&said);
    assert_eq!(addr, "/acme/boom");
    assert!(reason.contains("command script exploded"), "{reason}");
}

#[test]
fn a_custom_command_script_that_fails_is_a_fault_carrying_the_script_error() {
    let (st, _rx, _tx) = state(driver(), 8);
    let said = Said::default();
    send(&st, &said, msg("/acme/cboom", vec![]), client());
    let (addr, reason) = the_fault(&said);
    assert_eq!(addr, "/acme/cboom");
    assert!(reason.contains("custom script exploded"), "{reason}");
}

#[test]
fn a_fault_without_a_real_sender_is_broadcast_to_the_clients_by_send_osc() {
    let (st, _rx, _tx) = state(driver(), 8);
    let said = Said::default();
    send(&st, &said, msg("/acme/set", vec![]), nobody());
    assert!(said.to_sender().is_empty(), "no one to answer");
    let b = said.broadcast();
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(b[0].addr, "/bridge/fault");
    let a = strings(&b[0]);
    assert_eq!(a[0], "/acme/set");
    assert!(a[1].contains("missing arg v"), "{a:?}");
}

#[test]
fn a_message_the_orchestrator_re_injects_has_its_fault_broadcast() {
    let (tx, _rx) = bounded::<Vec<u8>>(8);
    let path = BenchPath::new(vec![(synth(), tx)], &[]);
    let said = Said::default();
    path.dispatch(msg(CUTOFF, vec![s("loud")]), &said.send_osc(), &said.reply_to());
    assert!(said.to_sender().is_empty());
    let b = said.broadcast();
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(b[0].addr, "/bridge/fault");
    assert_eq!(strings(&b[0])[0], CUTOFF);
}

#[test]
fn a_correct_message_answers_nothing() {
    let (st, rx, _tx) = state(synth(), 8);
    let said = Said::default();
    send(&st, &said, msg(CUTOFF, vec![OscType::Int(64)]), client());
    send(&st, &said, msg(CUTOFF, vec![OscType::Int(65)]), nobody());
    assert!(said.to_sender().is_empty(), "{:?}", said.to_sender());
    assert!(said.broadcast().is_empty(), "{:?}", said.broadcast());
    assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![vec![0xB0, 74, 64], vec![0xB0, 74, 65]]);
}

fn packet(msgs: Vec<OscMessage>) -> OscPacket {
    OscPacket::Bundle(OscBundle {
        timetag: OscTime { seconds: 0, fractional: 1 },
        content: msgs.into_iter().map(OscPacket::Message).collect(),
    })
}

fn send_packet(st: &DeviceDispatch, said: &Said, pkt: OscPacket, from: SocketAddr) {
    dispatch_packet_to_device(st, pkt, &said.send_osc(), &said.reply_to(), from);
}

#[test]
fn a_sync_in_the_packet_is_answered_done_with_its_id_after_the_faults_of_what_precedes_it() {
    let (st, _rx, _tx) = state(driver(), 8);
    let said = Said::default();
    send_packet(&st, &said, packet(vec![msg("/acme/set", vec![]), msg("/bridge/sync", vec![s("id-7")])]), client());
    let got = said.to_sender();
    let order: Vec<&str> = got.iter().map(|(_, m)| m.addr.as_str()).collect();
    assert_eq!(order, ["/bridge/fault", "/bridge/done"], "{got:?}");
    assert!(got.iter().all(|(to, _)| *to == client()));
    assert_eq!(got[1].1.args, vec![s("id-7")]);
    assert!(said.broadcast().is_empty());
}

#[test]
fn a_sync_after_a_correct_message_is_answered_done_alone() {
    let (st, rx, _tx) = state(synth(), 8);
    let said = Said::default();
    send_packet(&st, &said, packet(vec![msg(CUTOFF, vec![OscType::Int(9)]), msg("/bridge/sync", vec![OscType::Int(3)])]), client());
    let got = said.to_sender();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].1.addr, "/bridge/done");
    assert_eq!(got[0].1.args, vec![OscType::Int(3)]);
    assert_eq!(rx.try_iter().count(), 1, "the message was handled");
}

#[test]
fn a_packet_without_a_sync_gets_no_done() {
    let (st, _rx, _tx) = state(synth(), 8);
    let said = Said::default();
    send_packet(&st, &said, packet(vec![msg(CUTOFF, vec![OscType::Int(9)]), msg(CUTOFF, vec![OscType::Int(10)])]), client());
    assert!(said.to_sender().is_empty());
    assert!(said.broadcast().is_empty());
    let lone = OscPacket::Message(msg(CUTOFF, vec![OscType::Int(11)]));
    send_packet(&st, &said, lone, client());
    assert!(said.to_sender().is_empty());
}

#[test]
fn the_sync_is_never_taken_for_a_fault_nor_sent_to_the_device() {
    let (st, rx, _tx) = state(synth(), 8);
    let said = Said::default();
    send_packet(&st, &said, OscPacket::Message(msg("/bridge/sync", vec![s("only")])), client());
    let got = said.to_sender();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].1.addr, "/bridge/done");
    assert!(rx.try_recv().is_err());
}

#[test]
fn a_message_sent_with_the_no_reply_route_still_builds_its_frame() {
    // The relay's `encode` and the tests that read only the queue pass a route that drops replies.
    let (st, rx, _tx) = state(synth(), 8);
    dispatch_to_device(&st, msg(CUTOFF, vec![OscType::Int(5)]), &Said::default().send_osc(), &no_reply(), client());
    assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![vec![0xB0, 74, 5]]);
}

fn two_devices(links: &[osc_bridge::orchestrator::RouteRule]) -> (BenchPath, Receiver<Vec<u8>>, Receiver<Vec<u8>>) {
    let (tx_a, rx_a) = bounded::<Vec<u8>>(8);
    let (tx_b, rx_b) = bounded::<Vec<u8>>(8);
    (BenchPath::new(vec![(synth(), tx_a), (driver(), tx_b)], links), rx_a, rx_b)
}

#[test]
fn an_orchestrator_answers_an_address_no_device_owns_with_an_unknown_address_fault() {
    // The bench door carries no real sender, so the fault is broadcast, as for any fault there.
    let (path, rx_a, rx_b) = two_devices(&[]);
    let said = Said::default();
    path.dispatch(msg("/nobody/home", vec![OscType::Int(1)]), &said.send_osc(), &said.reply_to());
    let b = said.broadcast();
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(b[0].addr, "/bridge/fault");
    assert_eq!(strings(&b[0]), vec!["/nobody/home".to_string(), "unknown address".to_string()]);
    assert!(rx_a.try_recv().is_err() && rx_b.try_recv().is_err(), "no device receives a byte");
}

#[test]
fn an_unowned_address_that_a_link_follows_reaches_the_link_target_without_a_fault() {
    let link = osc_bridge::orchestrator::RouteRule { from: "/free/in".into(), to: CUTOFF.into(), map: None };
    let (path, rx_a, rx_b) = two_devices(&[link]);
    let said = Said::default();
    path.dispatch(msg("/free/in", vec![OscType::Int(64)]), &said.send_osc(), &said.reply_to());
    assert!(said.to_sender().is_empty(), "{:?}", said.to_sender());
    assert!(said.broadcast().is_empty(), "{:?}", said.broadcast());
    assert_eq!(rx_a.try_iter().collect::<Vec<_>>(), vec![vec![0xB0, 74, 64]]);
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn an_unowned_address_that_a_link_follows_to_an_unowned_target_answers_no_fault() {
    let link = osc_bridge::orchestrator::RouteRule { from: "/free/in".into(), to: "/free/out".into(), map: None };
    let (path, rx_a, rx_b) = two_devices(&[link]);
    let said = Said::default();
    path.dispatch(msg("/free/in", vec![OscType::Int(64)]), &said.send_osc(), &said.reply_to());
    assert!(said.to_sender().is_empty(), "{:?}", said.to_sender());
    assert!(said.broadcast().is_empty(), "{:?}", said.broadcast());
    assert!(rx_a.try_recv().is_err() && rx_b.try_recv().is_err());
}

#[test]
fn an_address_under_bridge_that_the_orchestrator_handles_answers_no_unknown_address_fault() {
    let (path, _a, _b) = two_devices(&[]);
    for addr in ["/bridge/status", "/bridge/docs", "/bridge/link/list"] {
        let said = Said::default();
        path.dispatch(msg(addr, vec![]), &said.send_osc(), &said.reply_to());
        assert!(said.to_sender().iter().all(|(_, m)| m.addr != "/bridge/fault"), "{addr}: {:?}", said.to_sender());
        assert!(said.broadcast().iter().all(|m| m.addr != "/bridge/fault"), "{addr}: {:?}", said.broadcast());
    }
}

#[test]
fn a_message_the_owner_device_rejects_gets_one_fault_not_two() {
    let (path, _a, _b) = two_devices(&[]);
    let said = Said::default();
    path.dispatch(msg(CUTOFF, vec![s("loud")]), &said.send_osc(), &said.reply_to());
    let b = said.broadcast();
    assert_eq!(b.len(), 1, "{b:?}");
    let a = strings(&b[0]);
    assert_eq!(a[0], CUTOFF);
    assert_ne!(a[1], "unknown address");
}
