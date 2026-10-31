//! OB-d0b criterion 1, through the runtime: `parameterMap.get` answers "the last
//! value sent or received" for the device (Electra One Lua extension,
//! https://docs.electra.one/developers/luaext.html). A value sent is an OSC
//! message reaching a command; a value received is a device reply whose
//! `emit_osc` is that command's address. Both go through the dispatch the
//! runtime uses, with no socket and no MIDI port.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use crossbeam_channel::{bounded, Receiver, Sender};
use osc_bridge::device::Device;
use osc_bridge::runtime::{dispatch_to_device, handle_midi_in, DeviceDispatch, SendOsc};
use rosc::{OscMessage, OscType};

fn driver() -> Device {
    let json = r#"{
  "device": { "name": "Acme", "vendor": "Acme", "revision": "A1", "osc_prefix": "/acme" },
  "commands": [
    { "osc": "/level_s1", "_electra": { "parameter": { "type": "sysex", "number": 1 } },
      "args": [ { "name": "v", "type": "u7", "range": [0, 127], "default": 100 } ], "frame": [] },
    { "osc": "/read", "args": [], "frame": [],
      "script": "ctx = ...\nctx.payload = { parameterMap.get(1, PT_SYSEX, 1) }\nreturn ctx" }
  ],
  "replies": [
    { "match_frame": [240, 66, "{v}", 247], "emit_osc": "/acme/level_s1 {v}" }
  ]
}"#;
    serde_json::from_str(json).expect("driver parses")
}

fn from() -> SocketAddr { SocketAddr::from(([0, 0, 0, 0], 0)) }

fn state() -> (DeviceDispatch, Receiver<Vec<u8>>) {
    let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = bounded(64);
    (DeviceDispatch::build(driver(), tx), rx)
}

fn send(state: &DeviceDispatch, addr: &str, args: Vec<OscType>) {
    let discard: SendOsc = Arc::new(|_| {});
    dispatch_to_device(state, OscMessage { addr: addr.into(), args }, &discard, from());
}

/// What the script of `/acme/read` answers: the last frame it queued.
fn read(state: &DeviceDispatch, rx: &Receiver<Vec<u8>>) -> Vec<u8> {
    while rx.try_recv().is_ok() {}
    send(state, "/acme/read", vec![]);
    rx.try_iter().last().expect("the read command queued its payload")
}

#[test]
fn c1_a_value_sent_to_the_command_is_the_value_parameter_map_get_answers() {
    let (st, rx) = state();
    assert_eq!(read(&st, &rx), vec![100], "the default before any value");
    send(&st, "/acme/level_s1", vec![OscType::Int(64)]);
    assert_eq!(read(&st, &rx), vec![64]);
    send(&st, "/acme/level_s1", vec![OscType::Int(12)]);
    assert_eq!(read(&st, &rx), vec![12], "a newer send replaces the older");
}

#[test]
fn c1_a_value_received_from_the_device_is_the_value_parameter_map_get_answers() {
    let (st, rx) = state();
    let seen: Arc<Mutex<Vec<OscMessage>>> = Arc::default();
    let sink = seen.clone();
    let out: Arc<dyn Fn(OscMessage) + Send + Sync> = Arc::new(move |m| sink.lock().unwrap().push(m));
    handle_midi_in(&[0xF0, 0x42, 77, 0xF7], &st.device, &out, &st.routes, &st.scripts);
    assert_eq!(seen.lock().unwrap().len(), 1, "the reply is decoded");
    assert_eq!(read(&st, &rx), vec![77]);
}
