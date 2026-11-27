//! A device described in 0.7 is driven as its earlier form was: for every control, parameter,
//! command and trigger of the test catalogue, the bytes emitted are those recorded on the same
//! drivers before they were converted to 0.7 (`tests/fixtures/emitted_bytes.json`: a call, the
//! frames it queued in hexadecimal, and whether it was answered with a fault).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crossbeam_channel::bounded;
use osc_bridge::runtime::{dispatch_to_device, DeviceDispatch, ReplyTo, SendOsc};
use rosc::{OscMessage, OscType};
use serde_json::Value;

/// What one call to one driver queued, and whether it was answered with a fault.
fn emitted(device: &osc_bridge::Device, address: &str, args: &[Value]) -> (Vec<String>, bool) {
    let (tx, rx) = bounded::<Vec<u8>>(256);
    let state = DeviceDispatch::build(device.clone(), tx);
    let faulted = Arc::new(Mutex::new(false));
    let (said, replied) = (faulted.clone(), faulted.clone());
    let send: SendOsc = Arc::new(move |m| *said.lock().unwrap() |= m.addr == "/bridge/fault");
    let reply: ReplyTo = Arc::new(move |_, m| *replied.lock().unwrap() |= m.addr == "/bridge/fault");
    let args = args.iter().map(|v| match v {
        Value::String(s) => OscType::String(s.clone()),
        v => OscType::Int(v.as_i64().unwrap() as i32),
    }).collect();
    dispatch_to_device(&state, OscMessage { addr: address.into(), args }, &send, &reply, "127.0.0.1:9999".parse().unwrap());
    let frames = rx.try_iter().map(|f| osc_bridge::mcp::hex_bytes(&f)).collect();
    let faulted = *faulted.lock().unwrap();
    (frames, faulted)
}

#[test]
fn every_test_driver_in_0_7_emits_the_bytes_its_earlier_form_emitted() {
    let recorded: Vec<Value> = serde_json::from_str(include_str!("../fixtures/emitted_bytes.json")).unwrap();
    assert!(recorded.len() > 1000, "{} calls recorded", recorded.len());
    let mut devices = BTreeMap::new();
    let mut differ = Vec::new();
    for call in &recorded {
        let id = call["device"].as_str().unwrap();
        let device = devices.entry(id.to_string()).or_insert_with(|| {
            let path = format!("tests/fixtures/devices/{id}.json");
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains("https://w3id.org/osd/0.7/device.schema.json"), "{id} is written in 0.7");
            osc_bridge::device::load(&path).unwrap_or_else(|e| panic!("{id}: {e:#}"))
        });
        let address = call["address"].as_str().unwrap();
        let args = call["args"].as_array().unwrap();
        let (frames, faulted) = emitted(device, address, args);
        let want: Vec<&str> = call["frames"].as_array().unwrap().iter().map(|f| f.as_str().unwrap()).collect();
        if frames != want || faulted != call["fault"].as_bool().unwrap() {
            differ.push(format!("{id} {address} {args:?}: {frames:?} (fault {faulted}), recorded {call}"));
        }
    }
    assert!(differ.is_empty(), "{} calls of {} differ:\n{}", differ.len(), recorded.len(),
        differ.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
}
