//! The orchestrator's message path, reached without a socket or a MIDI port.
//!
//! `BenchPath` is the one door the time bench drives: a message enters, the
//! owner device is found, the links are applied, and the frames reach the
//! emission queue of a loaded device that has no port.

use crossbeam_channel::{bounded, Receiver};
use osc_bridge::orchestrator::{BenchPath, RouteRule};
use osc_bridge::runtime::SendOsc;
use rosc::{OscMessage, OscType};
use std::sync::{Arc, Mutex};

const SOURCE: &str = "/test_synth/filter/cutoff";
const TARGET: &str = "/test_synth/filter/resonance";

fn rig(links: &[RouteRule]) -> (BenchPath, Receiver<Vec<u8>>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/devices/examples/test_synth.json");
    let device = osc_bridge::device::load_with_docs(path).unwrap();
    let (tx, rx) = bounded::<Vec<u8>>(64);
    (BenchPath::new(vec![(device, tx)], links), rx)
}

fn msg(addr: &str, v: i32) -> OscMessage {
    OscMessage { addr: addr.into(), args: vec![OscType::Int(v)] }
}

fn silent() -> SendOsc {
    Arc::new(|_| {})
}

#[test]
fn message_to_a_loaded_device_reaches_its_emission_queue() {
    let (path, rx) = rig(&[]);
    path.dispatch(msg(SOURCE, 64), &silent());
    let frames: Vec<Vec<u8>> = rx.try_iter().collect();
    assert_eq!(frames, vec![vec![0xB0, 74, 64]]);
}

#[test]
fn message_following_a_link_reaches_the_queue_twice_the_link_after_the_owner() {
    let link = RouteRule { from: SOURCE.into(), to: TARGET.into(), map: None };
    let (path, rx) = rig(&[link]);
    path.dispatch(msg(SOURCE, 64), &silent());
    let frames: Vec<Vec<u8>> = rx.try_iter().collect();
    assert_eq!(frames, vec![vec![0xB0, 74, 64], vec![0xB0, 71, 64]]);
}

#[test]
fn link_scaling_is_applied_before_the_frame_is_built() {
    let map = osc_bridge::orchestrator::MapRule { from: [0.0, 127.0], to: [0.0, 1.0] };
    let link = RouteRule { from: SOURCE.into(), to: TARGET.into(), map: Some(map) };
    let (path, rx) = rig(&[link]);
    path.dispatch(msg(SOURCE, 127), &silent());
    let frames: Vec<Vec<u8>> = rx.try_iter().collect();
    assert_eq!(frames, vec![vec![0xB0, 74, 127], vec![0xB0, 71, 1]]);
}

#[test]
fn message_no_device_owns_reaches_no_queue() {
    let (path, rx) = rig(&[]);
    path.dispatch(msg("/nobody/home", 1), &silent());
    assert!(rx.try_recv().is_err());
}

#[test]
fn link_control_address_is_answered_through_the_send_function() {
    let (path, rx) = rig(&[]);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let send: SendOsc = Arc::new(move |m| sink.lock().unwrap().push(m.addr));
    path.dispatch(OscMessage { addr: "/bridge/link/list".into(), args: vec![] }, &send);
    assert_eq!(*seen.lock().unwrap(), vec!["/bridge/link/list/end".to_string()]);
    assert!(rx.try_recv().is_err());
}
