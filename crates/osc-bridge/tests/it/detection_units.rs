//! The ports of one device receive one answer: the report of a detection holds one entry per
//! device (its `ports`, each `{port, out_port, in_port}`), computed over all its ports; the
//! proof names the port that gave it, and the run command names that port.

use crate::common::{device05, files, temp};
use osc_bridge::capture::{Capture, FORMAT_VERSION};
use osc_bridge::recognition::Identity;
use osc_bridge::mcp::detect_capture;
use serde_json::{json, Value};
use std::path::Path;

const MINILAB_MIDI: &str = "Minilab3:Minilab3 MIDI 40:0";
const BOX_A: &str = "Box:Box A 20:0";
const BOX_B: &str = "Box:Box B 20:1";
const BOX_ID: &str = "acme/box.vendor.fw-2";

fn corpus(name: &str) -> Capture {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/captures").join(name);
    Capture::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn usb_body(v: u16, p: u16) -> String {
    format!(r#","identity":{{"usb":[{{"vendor_id":{v},"product_id":{p}}}]}}"#)
}

/// The catalogue of the corpus: each driver declares its USB ids.
fn corpus_catalogue() -> tempfile::TempDir {
    let root = temp("units-corpus");
    files(root.path(), &[
        ("behringer/bcr32.json", &device05("BCR-32", "/bcr32", &usb_body(0x1397, 0x12cf))),
        ("electra-one/electra-one.json", &device05("Electra One", "/electra", &usb_body(0x1fc9, 0x82cf))),
        ("arturia/minilab3.json", &device05("MiniLab 3", "/minilab3", &usb_body(0x1c75, 0x020b))),
    ]);
    root
}

fn shared_devices() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/devices")
}

/// The raw names of the ports of a device of the report, in order.
fn ports_of(device: &Value) -> Vec<String> {
    device["ports"].as_array().unwrap_or_else(|| panic!("no ports: {device}")).iter()
        .map(|p| p["port"].as_str().unwrap().to_string()).collect()
}

/// The groups of ports of a report, each sorted, the groups sorted.
fn groups(devices: &[Value]) -> Vec<Vec<String>> {
    let mut g: Vec<Vec<String>> = devices.iter().map(|d| { let mut p = ports_of(d); p.sort(); p }).collect();
    g.sort();
    g
}

fn device_with<'a>(devices: &'a [Value], port: &str) -> &'a Value {
    devices.iter().find(|d| ports_of(d).iter().any(|p| p == port)).unwrap_or_else(|| panic!("{port} in {devices:?}"))
}

fn strong(candidate: &Value) -> bool {
    candidate["proofs"].as_array().unwrap().iter().any(|p| p["nature"] == "strong")
}

fn capture_of(outputs: &[&str], received: &[(&str, Vec<u8>)], cards: &str, usbids: Value) -> Capture {
    Capture::parse(&json!({
        "format_version": FORMAT_VERSION, "outputs": outputs, "inputs": outputs,
        "cards": cards, "usbids": usbids, "clients": "", "requests": [],
        "received": received.iter().map(|(p, b)| json!({"port": p, "bytes": b, "ms": 5})).collect::<Vec<_>>(),
    }).to_string()).unwrap()
}

fn box_reply() -> Vec<u8> {
    Identity { manufacturer: vec![0, 33, 9], family: 1, model: 1, version: [2, 0, 0, 0] }.reply()
}

/// Two ports of one client; the Identity Reply of the Box comes on `answering`.
fn box_capture(answering: &str) -> Capture {
    capture_of(&[BOX_A, BOX_B], &[(answering, box_reply())], "", json!([]))
}

// ---- one device, one answer ------------------------------------------------------------

#[test]
fn the_four_ports_of_the_minilab_3_are_one_device_with_one_answer() {
    let devices = detect_capture(corpus_catalogue().path(), &corpus("2026-11-21.json")).unwrap();
    let minilab = device_with(&devices, MINILAB_MIDI);
    assert_eq!(ports_of(minilab), [MINILAB_MIDI, "Minilab3:Minilab3 DIN THRU 40:1",
        "Minilab3:Minilab3 MCU/HUI 40:2", "Minilab3:Minilab3 ALV 40:3"], "{minilab}");
    assert_eq!(minilab["candidates"][0]["device"], "arturia/minilab3", "{minilab}");
    assert!(strong(&minilab["candidates"][0]), "{minilab}");
    assert_eq!(devices.iter().filter(|d| ports_of(d).iter().any(|p| p.starts_with("Minilab3"))).count(), 1);
}

#[test]
fn the_electra_one_and_the_bcr_32_of_the_bench_are_one_device_each() {
    let devices = detect_capture(corpus_catalogue().path(), &corpus("2026-11-21.json")).unwrap();
    assert_eq!(groups(&devices), vec![
        vec!["Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 24:0".to_string(), "Behringer BCR-32:Behringer BCR-32 BCR-32 ZAQ 24:1".to_string()],
        vec!["Electra Controller:Electra Controller Electra CTRL 28:2".to_string(), "Electra Controller:Electra Controller Electra Port 28:0".to_string(),
             "Electra Controller:Electra Controller Electra Port 28:1".to_string()],
        vec!["Midi Through:Midi Through Port-0 14:0".to_string()],
        vec!["Minilab3:Minilab3 ALV 40:3".to_string(), "Minilab3:Minilab3 DIN THRU 40:1".to_string(),
             "Minilab3:Minilab3 MCU/HUI 40:2".to_string(), MINILAB_MIDI.to_string()],
    ]);
    let answer = |port: &str| device_with(&devices, port)["candidates"][0]["device"].clone();
    assert_eq!(answer("Behringer BCR-32:Behringer BCR-32 BCR-32 ZAQ 24:1"), "behringer/bcr32");
    assert_eq!(answer("Electra Controller:Electra Controller Electra CTRL 28:2"), "electra-one/electra-one");
}

#[test]
fn the_bench_of_seven_devices_reports_one_device_per_client() {
    let devices = detect_capture(corpus_catalogue().path(), &corpus("2026-11-21-banc-7-appareils.json")).unwrap();
    assert_eq!(devices.len(), 8, "Midi Through and seven devices: {:?}", groups(&devices));
    assert_eq!(ports_of(device_with(&devices, "MatrixBrute:MatrixBrute MIDI Interface 36:1")).len(), 2);
    assert_eq!(ports_of(device_with(&devices, "Osmose:Osmose  48:1")).len(), 2);
}

#[test]
fn a_strong_proof_on_one_port_beats_the_name_alone_of_the_others_whichever_port_answers() {
    let catalogue = shared_devices();
    let on_b = detect_capture(&catalogue, &box_capture(BOX_B)).unwrap();
    let on_a = detect_capture(&catalogue, &box_capture(BOX_A)).unwrap();
    for devices in [&on_a, &on_b] {
        assert_eq!(devices.len(), 1, "{devices:?}");
        let first = &devices[0]["candidates"][0];
        assert_eq!(first["device"], BOX_ID, "{devices:?}");
        assert!(strong(first), "{devices:?}");
        assert!(first.get("state_label").is_none(), "recognised, not 'possible': {devices:?}");
    }
    assert_eq!(on_a[0]["candidates"][0]["score"], on_b[0]["candidates"][0]["score"], "one formula, per device");
}

#[test]
fn a_device_with_no_strong_proof_on_any_port_stays_possible_to_confirm() {
    let root = temp("units-name");
    files(root.path(), &[("acme/osmose.json", &device05("Osmose", "/osmose", ""))]);
    let cap = capture_of(&["Osmose:Osmose A 24:0", "Osmose:Osmose B 24:1"], &[], "", json!([]));
    let devices = detect_capture(root.path(), &cap).unwrap();
    assert_eq!(devices.len(), 1, "{devices:?}");
    assert!(!strong(&devices[0]["candidates"][0]), "{devices:?}");
    assert!(devices[0]["run_command"].is_null(), "{devices:?}");
}

#[test]
fn the_proof_names_the_port_that_gave_it() {
    let devices = detect_capture(&shared_devices(), &box_capture(BOX_B)).unwrap();
    let proofs = devices[0]["candidates"][0]["proofs"].as_array().unwrap();
    let reply = proofs.iter().find(|p| p["kind"] == "identity reply").unwrap_or_else(|| panic!("{devices:?}"));
    assert_eq!(reply["port"], BOX_B, "{devices:?}");
}

#[test]
fn the_other_ports_are_listed_as_ports_of_the_device() {
    let devices = detect_capture(&shared_devices(), &box_capture(BOX_B)).unwrap();
    assert_eq!(ports_of(&devices[0]), [BOX_A, BOX_B]);
    assert_eq!(devices[0]["ports"][1]["out_port"], 1, "{devices:?}");
    assert_eq!(devices[0]["ports"][1]["in_port"], 1, "{devices:?}");
}

// ---- the run command ---------------------------------------------------------------------

#[test]
fn the_run_command_names_the_port_that_carries_the_proof() {
    for (answering, index) in [(BOX_B, 1), (BOX_A, 0)] {
        let devices = detect_capture(&shared_devices(), &box_capture(answering)).unwrap();
        let run = devices[0]["run_command"].as_str().unwrap_or_else(|| panic!("{devices:?}"));
        assert!(run.contains(&format!("--out-port {index}")) && run.contains(&format!("--in-port {index}")), "{run}");
    }
}

// ---- two devices of one model ------------------------------------------------------------

#[test]
fn two_devices_of_the_same_model_plugged_together_stay_two_devices() {
    let cards = " 6 [Minilab3       ]: USB-Audio - Minilab3\n                      Arturia Minilab3 at usb-0000:00:14.0-2, full speed\n";
    let ports = ["Minilab3:Minilab3 MIDI 40:0", "Minilab3:Minilab3 ALV 40:3", "Minilab3:Minilab3 MIDI 41:0", "Minilab3:Minilab3 ALV 41:3"];
    let cap = capture_of(&ports, &[], cards, json!([{"card": 6, "text": "1c75:020b\n"}]));
    let devices = detect_capture(corpus_catalogue().path(), &cap).unwrap();
    assert_eq!(groups(&devices), vec![
        vec![ports[1].to_string(), ports[0].to_string()],
        vec![ports[3].to_string(), ports[2].to_string()],
    ]);
}

// ---- the grouping, without card or USB id ---------------------------------------------------

#[test]
fn without_a_card_the_ports_group_by_client_number_else_by_client_name_and_by_nothing_else() {
    let ports = [
        "Zed:Zed A 20:0", "Zed:Zed B 20:1",     // one client number
        "Zed:Zed A 21:0",                       // the same name, another number: another device
        "Bar:Bar A 30:0", "Baz:Baz X 30:1",     // one number, two names: one device
        "Zedd:Zedd A 22:0",                     // a similar name, another number
        "Foo:Foo A", "Foo:Foo B",               // no number: the client name
        "Qux:Foo C",                            // no number, another client name
    ];
    let cap = capture_of(&ports, &[], "", json!([]));
    let devices = detect_capture(temp("units-empty").path(), &cap).unwrap();
    assert_eq!(groups(&devices), vec![
        vec!["Bar:Bar A 30:0".to_string(), "Baz:Baz X 30:1".to_string()],
        vec!["Foo:Foo A".to_string(), "Foo:Foo B".to_string()],
        vec!["Qux:Foo C".to_string()],
        vec!["Zed:Zed A 20:0".to_string(), "Zed:Zed B 20:1".to_string()],
        vec!["Zed:Zed A 21:0".to_string()],
        vec!["Zedd:Zedd A 22:0".to_string()],
    ]);
}

#[test]
fn a_software_port_never_receives_the_card_of_a_real_device_nor_joins_its_ports() {
    let devices = detect_capture(corpus_catalogue().path(), &corpus("2026-11-22-ports-logiciels.json")).unwrap();
    for d in &devices {
        assert!(!ports_of(d).is_empty(), "every entry is a device with its ports: {d}");
        assert!(d["candidates"].as_array().unwrap().is_empty(), "{d}");
        assert!(d["run_command"].is_null(), "{d}");
    }
}
