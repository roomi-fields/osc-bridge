//! A capture keeps what a detection observed, raw (`osc_bridge::capture`); the detection
//! replays it with no device (`detect::replay`), through the derivation the scan uses; a
//! corpus of captures, each with its neighbour file `<name>.expected.json` (port → device),
//! proves the recognition rules on what real devices showed.

use crate::common::{device05, files, temp};
use osc_bridge::capture::{Asound, Capture, FORMAT_VERSION};
use osc_bridge::detect::{replay, Identity};
use serde_json::{json, Value};
use std::path::Path;

/// The identity reply of an Arturia MiniLab 3.
fn minilab_reply() -> Vec<u8> {
    Identity { manufacturer: vec![0x00, 0x20, 0x6B], family: 2, model: 4, version: [1, 2, 3, 4] }.reply()
}

/// The system's list of clients as `/proc/asound/seq/clients` gives it, the MiniLab 3 a kernel client.
const MINILAB3_CLIENTS: &str = "Client info\n  cur  clients : 2\n  peak clients : 2\n  max  clients : 192\n\nClient   0 : \"System\" [Kernel]\n  Port   0 : \"Timer\" (Rwe-)\nClient  40 : \"Minilab3\" [Kernel]\n  Port   0 : \"Minilab3 MIDI\" (RWe-)\n";

/// The capture of the bench's MiniLab 3, written by hand from the listing the system gave.
fn minilab3_text() -> String {
    json!({
        "format_version": FORMAT_VERSION,
        "outputs": ["Minilab3:Minilab3 MIDI 40:0"],
        "inputs": ["Minilab3:Minilab3 MIDI 40:0"],
        "cards": " 6 [Minilab3       ]: USB-Audio - Minilab3\n                      Arturia Minilab3 at usb-0000:00:14.0-2, full speed\n",
        "clients": MINILAB3_CLIENTS,
        "usbids": [{"card": 6, "text": "1c75:020b\n"}],
        "requests": [{"port": "Minilab3:Minilab3 MIDI 40:0", "bytes": [240, 126, 127, 6, 1, 247]}],
        "received": [{"port": "Minilab3:Minilab3 MIDI 40:0", "bytes": minilab_reply(), "ms": 12}],
    }).to_string()
}

// ---- the format ---------------------------------------------------------------

#[test]
fn a_capture_is_read_back_as_it_was_written() {
    let c = Capture::parse(&minilab3_text()).unwrap();
    let again = Capture::parse(&c.to_json()).unwrap();
    assert_eq!(c.to_json(), again.to_json());
    let v: Value = serde_json::from_str(&c.to_json()).unwrap();
    assert_eq!(v["format_version"], json!(FORMAT_VERSION));
    assert_eq!(v["outputs"], json!(["Minilab3:Minilab3 MIDI 40:0"]));
    assert_eq!(v["usbids"][0]["text"], json!("1c75:020b\n"));
    assert_eq!(v["received"][0]["ms"], json!(12));
}

#[test]
fn a_capture_of_an_unknown_format_version_is_refused_naming_the_version() {
    let mut v: Value = serde_json::from_str(&minilab3_text()).unwrap();
    v["format_version"] = json!(9999);
    let err = Capture::parse(&v.to_string()).expect_err("refused").to_string();
    assert!(err.contains("9999"), "{err}");
}

#[test]
fn a_capture_keeps_ports_and_messages_in_the_order_the_system_gave_them() {
    let v = json!({
        "format_version": FORMAT_VERSION,
        "outputs": ["Zed:Zed 20:0", "Alpha:Alpha 21:0", "Midi Through:Midi Through Port-0 14:0"],
        "inputs": ["Zed:Zed 20:0", "Midi Through:Midi Through Port-0 14:0"],
        "cards": "", "usbids": [], "clients": "",
        "requests": [{"port": "Zed:Zed 20:0", "bytes": [1]}, {"port": "Alpha:Alpha 21:0", "bytes": [2]}],
        "received": [
            {"port": "Zed:Zed 20:0", "bytes": [240, 3, 247], "ms": 30},
            {"port": "Zed:Zed 20:0", "bytes": [240, 1, 247], "ms": 5},
        ],
    });
    let out: Value = serde_json::from_str(&Capture::parse(&v.to_string()).unwrap().to_json()).unwrap();
    for key in ["outputs", "inputs", "requests", "received"] {
        assert_eq!(out[key], v[key], "{key}");
    }
}

#[test]
fn a_capture_holds_no_pairing_and_no_usb_ids_read() {
    let c = Capture::parse(&minilab3_text()).unwrap().to_json();
    let v: Value = serde_json::from_str(&c).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["cards", "clients", "format_version", "inputs", "outputs", "received", "requests", "usbids"]);
}

// ---- the replay ---------------------------------------------------------------

#[test]
fn replaying_a_minilab3_capture_gives_its_identity_and_usb_ids() {
    let found = replay(&Capture::parse(&minilab3_text()).unwrap());
    assert_eq!(found.len(), 1);
    let d = &found[0];
    assert_eq!(d.label(), "Minilab3:Minilab3 MIDI 40:0");
    assert_eq!(d.usb, Some((0x1c75, 0x020b)));
    let id = d.identity.as_ref().expect("identity");
    assert_eq!(id.manufacturer, vec![0x00, 0x20, 0x6B]);
    assert_eq!((id.family, id.model, id.version), (2, 4, [1, 2, 3, 4]));
    assert_eq!(d.replies, vec![minilab_reply()]);
    assert_eq!((d.out_index, d.in_index), (Some(0), Some(0)));
}

#[test]
fn the_replay_pairs_inputs_with_outputs_and_gives_each_port_its_own_replies() {
    let other = vec![0xF0, 0x7D, 0x01, 0xF7];
    let v = json!({
        "format_version": FORMAT_VERSION,
        "outputs": ["Minilab3:Minilab3 MIDI 40:0", "Lonely:Lonely 50:0"],
        "inputs": ["Minilab3:Minilab3 MIDI 40:0", "Lonely:Lonely 50:0", "Only In:Only In 51:0"],
        "cards": " 6 [Minilab3       ]: USB-Audio - Minilab3\n",
        "usbids": [{"card": 6, "text": "1c75:020b\n"}], "clients": "",
        "requests": [],
        "received": [
            {"port": "Lonely:Lonely 50:0", "bytes": other, "ms": 3},
            {"port": "Minilab3:Minilab3 MIDI 40:0", "bytes": minilab_reply(), "ms": 9},
        ],
    });
    let found = replay(&Capture::parse(&v.to_string()).unwrap());
    assert_eq!(found.len(), 3, "two outputs, one input left over");
    assert_eq!(found[0].replies, vec![minilab_reply()]);
    assert_eq!(found[0].usb, Some((0x1c75, 0x020b)));
    assert_eq!(found[1].replies, vec![other]);
    assert_eq!(found[1].usb, None);
    assert_eq!(found[2].out_index, None);
    assert_eq!(found[2].in_name.as_deref(), Some("Only In:Only In 51:0"));
    assert!(found[2].replies.is_empty());
}

#[test]
fn a_listing_without_messages_replays_the_usb_ids_of_each_card_by_its_name() {
    let v = json!({
        "format_version": FORMAT_VERSION,
        "outputs": ["Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 24:0"],
        "inputs": ["Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 24:0"],
        "cards": " 2 [BCR32          ]: USB-Audio - Behringer BCR-32\n                      Behringer BCR-32 at usb-0000:00:14.0-1, full speed\n",
        "usbids": [{"card": 2, "text": "1397:12cf\n"}], "clients": "",
        "requests": [], "received": [],
    });
    let found = replay(&Capture::parse(&v.to_string()).unwrap());
    assert_eq!(found[0].usb, Some((0x1397, 0x12cf)));
    assert!(found[0].identity.is_none());
}

// ---- what a capture holds, and leaves out ---------------------------------------

#[test]
fn a_capture_of_a_machine_holds_no_host_user_or_serial_name() {
    let asound = temp("asound-root");
    files(asound.path(), &[
        ("cards", " 6 [Minilab3       ]: USB-Audio - Minilab3\n                      Arturia Minilab3 at usb-0000:00:14.0-2, full speed\n"),
        ("card6/usbid", "1c75:020b\n"),
        ("card6/id", "Minilab3\n"),
        ("card6/stream0", "Arturia Minilab3 at usb-0000:00:14.0-2\nSerial SERIAL-8f3a9c\n"),
        ("card6/usb-serial", "SERIAL-8f3a9c\n"),
        ("hostname", "HOST-romans-laptop\n"),
        ("card6/owner", "USER-romain\n"),
    ]);
    let asound = Asound::read(asound.path());
    let c = Capture::new(vec!["Minilab3:Minilab3 MIDI 40:0".into()], vec!["Minilab3:Minilab3 MIDI 40:0".into()], asound);
    let text = c.to_json();
    for secret in ["SERIAL-8f3a9c", "HOST-romans-laptop", "USER-romain"] {
        assert!(!text.contains(secret), "{secret} is in the capture:\n{text}");
    }
    let found = replay(&Capture::parse(&text).unwrap());
    assert_eq!(found[0].usb, Some((0x1c75, 0x020b)), "the listing and the usbid are kept");
}

#[test]
fn only_the_listing_and_the_usb_ids_are_read_from_the_system() {
    let asound = temp("asound-card-files");
    files(asound.path(), &[
        ("cards", " 1 [Foo            ]: USB-Audio - Foo\n 2 [Bar            ]: USB-Audio - Bar\n"),
        ("card1/usbid", "abcd:0001\n"),
        ("card2/usbid", "abcd:0002\n"),
        ("card2/pcm0p/info", "name: SOMEONE-PRIVATE\n"),
    ]);
    let c = Capture::new(vec![], vec![], Asound::read(asound.path()));
    let v: Value = serde_json::from_str(&c.to_json()).unwrap();
    assert_eq!(v["usbids"], json!([{"card": 1, "text": "abcd:0001\n"}, {"card": 2, "text": "abcd:0002\n"}]));
    assert!(!c.to_json().contains("SOMEONE-PRIVATE"));
}

// ---- the corpus ---------------------------------------------------------------

/// The catalogue of the corpus: each driver recognised by its USB ids.
fn corpus_catalogue() -> tempfile::TempDir {
    let usb = |v: u16, p: u16| format!(r#","identity":{{"usb":[{{"vendor_id":{v},"product_id":{p}}}]}}"#);
    let root = temp("corpus-catalogue");
    files(root.path(), &[
        ("behringer/bcr32.json", &device05("BCR-32", "/bcr32", &usb(0x1397, 0x12cf))),
        ("electra-one/electra-one.json", &device05("Electra One", "/electra", &usb(0x1fc9, 0x82cf))),
        ("arturia/minilab3.json", &device05("MiniLab 3", "/minilab3", &usb(0x1c75, 0x020b))),
    ]);
    root
}

/// A ticket id: a prefix of letters, a dash, a non-empty rest of letters, digits and dots.
/// Its existence is not checked.
fn is_ticket_id(s: &str) -> bool {
    let Some((prefix, rest)) = s.split_once('-') else { return false };
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_alphabetic())
        && !rest.is_empty()
        && !rest.starts_with('.')
        && !rest.ends_with('.')
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// Each capture of `dir` (`<name>.json`) is replayed over `catalogue` and compared with its
/// neighbour `<name>.expected.json`, a map port → truth, written by hand: a driver id of the
/// catalogue, null (no driver in this catalogue, or no device), or a `wrong_today` marker that
/// carries the expected truth and the ticket that corrects the detection (the marker is
/// refused when the detection already gives the truth). Keys starting with `_` are notes.
/// The errors name the capture and the port.
fn check_corpus(dir: &Path, catalogue: &Path) -> Result<usize, Vec<String>> {
    let mut errors = Vec::new();
    let mut checked = 0;
    let names = |suffix: &str| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().filter_map(|e| {
            let n = e.unwrap().file_name().to_string_lossy().to_string();
            n.strip_suffix(suffix).map(str::to_string)
        }).collect();
        v.sort();
        v
    };
    // `<name>.fingerprints.expected.json` is the neighbour of the learn mode, not of detection.
    let expected_names: Vec<String> =
        names(".expected.json").into_iter().filter(|n| !n.ends_with(".fingerprints")).collect();
    let captures: Vec<String> =
        names(".json").into_iter().filter(|n| !n.ends_with(".expected") && !n.ends_with(".fingerprints")).collect();
    for n in &expected_names {
        if !captures.contains(n) { errors.push(format!("{n}.expected.json has no capture {n}.json")); }
    }
    for n in &captures {
        if !expected_names.contains(n) { errors.push(format!("{n}.json has no neighbour file {n}.expected.json")); continue; }
        let capture = Capture::parse(&std::fs::read_to_string(dir.join(format!("{n}.json"))).unwrap())
            .unwrap_or_else(|e| panic!("{n}.json: {e}"));
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{n}.expected.json"))).unwrap()).unwrap();
        let devices = osc_bridge::mcp::detect_capture(catalogue, &capture).unwrap();
        // One answer per device: each of its ports is judged on it.
        for (d, port) in devices.iter().flat_map(|d| {
            d["ports"].as_array().unwrap_or_else(|| panic!("no ports: {d}")).iter().map(move |p| (d, p["port"].as_str().unwrap()))
        }) {
            let got = d["candidates"][0]["device"].as_str();
            let Some(truth) = expected.get(port) else {
                errors.push(format!("{n}.json: port {port}: not in {n}.expected.json"));
                continue;
            };
            // A truth is a driver id or null, or `{"wrong_today": {"truth": …, "ticket": …}}`.
            match truth.get("wrong_today") {
                None => {
                    let w = truth.as_str();
                    if w != got {
                        errors.push(format!("{n}.json: port {port}: detected {got:?}, expected {w:?}"));
                    } else {
                        checked += 1;
                    }
                }
                Some(marker) => {
                    let ticket = marker.get("ticket").and_then(Value::as_str).unwrap_or("");
                    if !is_ticket_id(ticket) {
                        errors.push(format!("{n}.json: port {port}: wrong_today ticket {ticket:?} is not a ticket id"));
                        continue;
                    }
                    let w = marker.get("truth").and_then(Value::as_str);
                    if w == got {
                        errors.push(format!("{n}.json: port {port}: marked wrong_today ({ticket}) but detection already gives {w:?}: remove the marker"));
                    } else {
                        checked += 1;
                    }
                }
            }
        }
    }
    if errors.is_empty() { Ok(checked) } else { Err(errors) }
}

#[test]
fn every_capture_of_the_corpus_detects_the_device_its_neighbour_file_declares() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/captures");
    let catalogue = corpus_catalogue();
    match check_corpus(&dir, catalogue.path()) {
        Ok(n) => assert!(n >= 28, "the corpus holds the three benches and the software ports, {n} ports checked"),
        Err(errors) => panic!("{}", errors.join("\n")),
    }
}

#[test]
fn the_corpus_holds_the_bench_of_the_21st_of_november_2026() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/captures");
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("2026-11-21.expected.json")).unwrap()).unwrap();
    assert_eq!(expected["Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 24:0"], "behringer/bcr32");
    assert_eq!(expected["Electra Controller:Electra Controller Electra Port 28:0"], "electra-one/electra-one");
    assert_eq!(expected["Minilab3:Minilab3 MIDI 40:0"], "arturia/minilab3");
}

fn small_corpus(expected: Value) -> tempfile::TempDir {
    let dir = temp("corpus");
    files(dir.path(), &[("2026-01-02.json", &minilab3_text()), ("2026-01-02.expected.json", &expected.to_string())]);
    dir
}

#[test]
fn the_corpus_check_passes_when_the_device_is_the_declared_one() {
    let dir = small_corpus(json!({"Minilab3:Minilab3 MIDI 40:0": "arturia/minilab3"}));
    assert_eq!(check_corpus(dir.path(), corpus_catalogue().path()), Ok(1));
}

#[test]
fn the_corpus_check_names_the_capture_and_the_port_of_a_device_that_differs() {
    let dir = small_corpus(json!({"Minilab3:Minilab3 MIDI 40:0": "electra-one/electra-one"}));
    let errors = check_corpus(dir.path(), corpus_catalogue().path()).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("2026-01-02.json") && errors[0].contains("Minilab3:Minilab3 MIDI 40:0"), "{errors:?}");
}

#[test]
fn the_corpus_check_refuses_a_capture_without_its_neighbour_file_and_the_reverse() {
    let dir = small_corpus(json!({}));
    files(dir.path(), &[("2026-02-03.json", &minilab3_text()), ("2026-03-04.expected.json", "{}")]);
    let errors = check_corpus(dir.path(), corpus_catalogue().path()).unwrap_err().join("\n");
    assert!(errors.contains("2026-02-03.json has no neighbour"), "{errors}");
    assert!(errors.contains("2026-03-04.expected.json has no capture"), "{errors}");
}

const MINILAB: &str = "Minilab3:Minilab3 MIDI 40:0";

fn wrong_today(truth: Value, ticket: &str) -> Value {
    json!({ MINILAB: { "wrong_today": { "truth": truth, "ticket": ticket } } })
}

#[test]
fn a_port_marked_wrong_today_passes_while_the_detection_differs_from_its_truth() {
    let dir = small_corpus(wrong_today(json!("electra-one/electra-one"), "OB-80g.17"));
    assert_eq!(check_corpus(dir.path(), corpus_catalogue().path()), Ok(1));
}

#[test]
fn a_port_marked_wrong_today_passes_whatever_the_wrong_detection_is() {
    // The detection gives arturia/minilab3: a truth of none, or of another driver, both differ.
    for truth in [json!(null), json!("behringer/bcr32")] {
        let dir = small_corpus(wrong_today(truth.clone(), "OB-80g.19"));
        assert_eq!(check_corpus(dir.path(), corpus_catalogue().path()), Ok(1), "{truth}");
    }
}

#[test]
fn a_port_marked_wrong_today_whose_detection_already_gives_the_truth_fails_saying_to_remove_the_marker() {
    let dir = small_corpus(wrong_today(json!("arturia/minilab3"), "OB-80g.17"));
    let errors = check_corpus(dir.path(), corpus_catalogue().path()).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("2026-01-02.json") && errors[0].contains(MINILAB), "{errors:?}");
    assert!(errors[0].contains("wrong_today") && errors[0].contains("remove the marker"), "{errors:?}");
    // The truth none, when the detection gives none, is the same case.
    let dir = temp("corpus-none");
    let lonely = json!({
        "format_version": FORMAT_VERSION, "outputs": ["Lonely:Lonely 50:0"], "inputs": [],
        "cards": "", "usbids": [], "clients": "", "requests": [], "received": [],
    });
    files(dir.path(), &[
        ("c.json", &lonely.to_string()),
        ("c.expected.json", &json!({"Lonely:Lonely 50:0": {"wrong_today": {"truth": null, "ticket": "OB-80g.18"}}}).to_string()),
    ]);
    let errors = check_corpus(dir.path(), corpus_catalogue().path()).unwrap_err().join("\n");
    assert!(errors.contains("remove the marker"), "{errors}");
}

#[test]
fn a_wrong_today_ticket_must_have_the_shape_of_a_ticket_id_and_need_not_exist() {
    for good in ["OB-80g.17", "OB-1", "SH-jmc.15", "AB-999999.9"] {
        let dir = small_corpus(wrong_today(json!("electra-one/electra-one"), good));
        assert_eq!(check_corpus(dir.path(), corpus_catalogue().path()), Ok(1), "{good}");
    }
    for bad in ["", "OB", "OB-", "-80g", "80g.17", "OB 80g", "OB-80g.", "OB-.17", "OB-80g 17", "#17"] {
        let dir = small_corpus(wrong_today(json!("electra-one/electra-one"), bad));
        let errors = check_corpus(dir.path(), corpus_catalogue().path()).unwrap_err();
        assert_eq!(errors.len(), 1, "{bad:?}: {errors:?}");
        assert!(errors[0].contains("not a ticket id") && errors[0].contains(MINILAB), "{bad:?}: {errors:?}");
    }
    let dir = small_corpus(json!({ MINILAB: { "wrong_today": { "truth": "electra-one/electra-one" } } }));
    assert!(check_corpus(dir.path(), corpus_catalogue().path()).is_err(), "a marker without a ticket");
}

#[test]
fn the_corpus_check_still_compares_a_plain_truth_when_a_neighbour_port_is_marked() {
    let v = json!({
        "format_version": FORMAT_VERSION,
        "outputs": ["Minilab3:Minilab3 MIDI 40:0", "Lonely:Lonely 50:0"],
        "inputs": [], "cards": " 6 [Minilab3       ]: USB-Audio - Minilab3\n",
        "usbids": [{"card": 6, "text": "1c75:020b\n"}], "clients": "", "requests": [], "received": [],
    });
    let dir = temp("corpus-mixed");
    files(dir.path(), &[
        ("c.json", &v.to_string()),
        ("c.expected.json", &json!({
            MINILAB: { "wrong_today": { "truth": null, "ticket": "OB-80g.17" } },
            "Lonely:Lonely 50:0": "behringer/bcr32",
        }).to_string()),
    ]);
    let errors = check_corpus(dir.path(), corpus_catalogue().path()).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("Lonely:Lonely 50:0"), "{errors:?}");
}

// ---- the corpus of the bench of seven devices and of the software ports -------------

fn captures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/captures")
}

fn read_json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(captures_dir().join(name)).unwrap()).unwrap()
}

#[test]
fn the_corpus_holds_the_bench_of_seven_devices_and_the_software_ports_each_with_its_truth_file() {
    for n in ["2026-11-21-banc-7-appareils", "2026-11-22-ports-logiciels"] {
        assert!(captures_dir().join(format!("{n}.json")).is_file(), "{n}.json");
        assert!(captures_dir().join(format!("{n}.expected.json")).is_file(), "{n}.expected.json");
    }
}

#[test]
fn the_bench_of_seven_devices_has_a_truth_for_each_of_its_sixteen_ports() {
    let capture = Capture::parse(&std::fs::read_to_string(captures_dir().join("2026-11-21-banc-7-appareils.json")).unwrap()).unwrap();
    let expected = read_json("2026-11-21-banc-7-appareils.expected.json");
    assert_eq!(capture.outputs.len(), 16);
    let truths: Vec<(&String, &Value)> = expected.as_object().unwrap().iter().filter(|(k, _)| !k.starts_with('_')).collect();
    assert_eq!(truths.len(), 16);
    for port in &capture.outputs {
        assert!(expected.get(port.as_str()).is_some(), "{port} has no truth");
    }
    let id = |client: &str| -> Vec<Value> {
        truths.iter().filter(|(k, _)| k.starts_with(client)).map(|(_, v)| (*v).clone()).collect()
    };
    assert_eq!(id("Behringer BCR-32"), vec![json!("behringer/bcr32"); 2]);
    assert_eq!(id("Electra Controller"), vec![json!("electra-one/electra-one"); 3]);
    assert_eq!(id("Minilab3"), vec![json!("arturia/minilab3"); 4]);
    for none in ["Midi Through", "Deluge", "MatrixBrute", "Moog Sub 37", "Osmose"] {
        assert!(id(none).iter().all(Value::is_null), "{none}");
    }
    let note = expected["_note"].as_str().expect("the truth file says what none means");
    assert!(note.contains("no driver in this catalogue"), "{note}");
}

#[test]
fn the_software_ports_capture_holds_the_seven_software_ports_none_of_them_a_device() {
    let capture = Capture::parse(&std::fs::read_to_string(captures_dir().join("2026-11-22-ports-logiciels.json")).unwrap()).unwrap();
    let expected = read_json("2026-11-22-ports-logiciels.expected.json");
    for client in ["FLUID Synth", "Microsoft GS Wavetable Synth", "Controller:", "Audio Interface MIDI", "TiMidity", "Pure Data", "Midi Through"] {
        assert!(capture.outputs.iter().any(|p| p.starts_with(client)), "{client} is not in the capture");
    }
    assert!(capture.outputs.iter().any(|p| p.contains("Synth input port")), "FLUID Synth names its port Synth input port");
    for (port, truth) in expected.as_object().unwrap().iter().filter(|(k, _)| !k.starts_with('_')) {
        assert!(truth.is_null(), "{port}");
    }
    assert_eq!(serde_json::from_str::<Value>(&capture.to_json()).unwrap()["usbids"], json!([]), "software ports have no USB id");
}

/// Every capture of the corpus: each file of the folder that parses as a capture, the truth files
/// and the README left aside.
fn corpus_captures() -> Vec<(String, Capture)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(captures_dir()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".json") || name.ends_with(".expected.json") || name.ends_with(".fingerprints.expected.json") || name == "README.md" {
            continue;
        }
        if let Ok(capture) = Capture::parse(&std::fs::read_to_string(&path).unwrap()) {
            found.push((name, capture));
        }
    }
    found
}

/// The replies' bytes as text, the bytes outside printable ASCII a space.
fn ascii_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| if (32..127).contains(b) { *b as char } else { ' ' }).collect()
}

/// The value of every `serial` field in a text, whatever the case.
fn serial_values(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut values = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("serial") {
        let rest = &text[from + at + 6..];
        let value: String = rest.trim_start_matches(|c: char| c == '"' || c == ':' || c.is_whitespace()).chars().take_while(|c| *c != '"').collect();
        values.push(value);
        from += at + 6;
    }
    values
}

/// Each `EO2-` followed by eight hexadecimal digits in a text, the placeholder left out.
fn electra_serials(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("EO2-") {
        let start = from + at;
        let digits: String = text[start + 4..].chars().take(8).collect();
        if digits.len() == 8 && digits.chars().all(|c| c.is_ascii_hexdigit()) && digits != "00000000" {
            found.push(format!("EO2-{digits}"));
        }
        from = start + 4;
    }
    found
}

#[test]
fn the_corpus_guard_reads_every_capture_of_the_folder() {
    let names: Vec<String> = corpus_captures().into_iter().map(|(n, _)| n).collect();
    for expected in ["2026-11-21.json", "2026-11-21-banc-7-appareils.json", "2026-11-21-ports.json", "2026-11-22-ports-logiciels.json"] {
        assert!(names.iter().any(|n| n == expected), "{expected} is not read: {names:?}");
    }
    assert!(names.iter().all(|n| !n.contains("expected")), "{names:?}");
}

#[test]
fn every_capture_of_the_corpus_has_the_serial_of_its_unit_replaced_by_the_placeholder() {
    for (name, capture) in corpus_captures() {
        for r in &capture.received {
            for value in serial_values(&ascii_of(&r.bytes)) {
                assert_eq!(value, "EO2-00000000", "{name}, port {}: serial value {value:?}", r.port);
            }
        }
    }
}

#[test]
fn every_capture_of_the_corpus_holds_no_host_name_in_its_listings_or_its_replies() {
    for (name, capture) in corpus_captures() {
        let v: Value = serde_json::from_str(&capture.to_json()).unwrap();
        let listings = format!("{} {} {}", v["outputs"], v["inputs"], v["cards"]).to_lowercase();
        for word in ["serial", "hostname", "host-", "laptop", "/home/", "romain", "romi"] {
            assert!(!listings.contains(word), "{name}: {word} in the listings");
        }
        for r in &capture.received {
            let lower = ascii_of(&r.bytes).to_lowercase();
            for word in ["hostname", "host-", "laptop", "/home/", "romain", "romi"] {
                assert!(!lower.contains(word), "{name}: {word} in a reply of {}", r.port);
            }
        }
    }
}

#[test]
fn the_serial_reader_finds_every_occurrence_whatever_the_case() {
    let text = r#"{"Serial":"EO2-00000000","x":1}{"serial": "other"}"#;
    assert_eq!(serial_values(text), vec!["EO2-00000000".to_string(), "other".to_string()]);
    assert!(serial_values("no field here").is_empty());
}

#[test]
fn the_electra_serial_pattern_refuses_a_number_and_accepts_the_placeholder() {
    let real = format!("EO2-{}", "ABCDEF12");
    assert_eq!(electra_serials(&format!("a {real} b")), vec![real.clone()]);
    assert_eq!(electra_serials(&format!("EO2-{}", "0a1b2c3d")).len(), 1);
    assert!(electra_serials("verified on EO2-00000000").is_empty());
    assert!(electra_serials("EO2-123 and EO2-GHIJKLMN").is_empty());
}

#[test]
fn no_tracked_file_of_the_repository_carries_an_electra_serial_other_than_the_placeholder() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let listed = std::process::Command::new("git").arg("-C").arg(&root).args(["ls-files", "-z"]).output().unwrap();
    assert!(listed.status.success(), "git ls-files failed");
    let mut read = 0;
    for rel in String::from_utf8_lossy(&listed.stdout).split('\0').filter(|s| !s.is_empty()) {
        let Ok(bytes) = std::fs::read(root.join(rel)) else { continue };
        read += 1;
        let text = String::from_utf8_lossy(&bytes);
        assert!(electra_serials(&text).is_empty(), "{rel} carries an Electra One serial number");
    }
    assert!(read > 100, "only {read} tracked files read");
}

#[test]
fn the_bench_capture_replays_the_identities_and_the_usb_ids_the_bench_gave() {
    let capture = Capture::parse(&std::fs::read_to_string(captures_dir().join("2026-11-21-banc-7-appareils.json")).unwrap()).unwrap();
    let found = replay(&capture);
    assert_eq!(found.len(), 16);
    let by = |label: &str| found.iter().find(|d| d.label() == label).unwrap_or_else(|| panic!("{label}"));
    // The identity replies (F0 7E … 06 02 …) keep their manufacturer, family, model and version bytes.
    let minilab = by("Minilab3:Minilab3 MIDI 40:0");
    assert_eq!(minilab.replies, vec![vec![0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x00, 0x20, 0x6B, 0x02, 0x00, 0x04, 0x04, 0x45, 0x00, 0x02, 0x01, 0xF7]]);
    assert_eq!(minilab.identity.as_ref().expect("identity").manufacturer, vec![0x00, 0x20, 0x6B]);
    let brute = by("MatrixBrute:MatrixBrute 36:0");
    assert_eq!(brute.replies, vec![vec![0xF0, 0x7E, 0x01, 0x06, 0x02, 0x00, 0x20, 0x6B, 0x04, 0x00, 0x03, 0x01, 0x02, 0x00, 0x03, 0x03, 0xF7]]);
    assert!(brute.identity.is_some());
    assert_eq!(by("MatrixBrute:MatrixBrute MIDI Interface 36:1").replies, vec![vec![0xF0, 0x7E, 0x01, 0x06, 0x02, 0x00, 0x20, 0x6B, 0x04, 0x00, 0x03, 0x02, 0x02, 0x00, 0x03, 0x03, 0xF7]]);
    let moog = by("Moog Sub 37:Moog Sub 37 MIDI 1 44:0");
    assert_eq!(moog.replies, vec![vec![0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x04, 0x00, 0x0C, 0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0xF7]]);
    assert!(moog.identity.is_some());
    // The USB ids of the seven cards.
    let usb = [
        ("Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 24:0", (0x1397, 0x12cf)),
        ("Electra Controller:Electra Controller Electra Port 28:0", (0x1fc9, 0x82cf)),
        ("Deluge:Deluge OUT 32:0", (0x16d0, 0x0ce2)),
        ("MatrixBrute:MatrixBrute 36:0", (0x1c75, 0x0403)),
        ("Minilab3:Minilab3 MIDI 40:0", (0x1c75, 0x020b)),
        ("Moog Sub 37:Moog Sub 37 MIDI 1 44:0", (0x2662, 0x0ffc)),
        ("Osmose:Osmose  48:0", (0x2e14, 0x0010)),
    ];
    for (label, ids) in usb {
        assert_eq!(by(label).usb, Some(ids), "{label}");
    }
    assert_eq!(by("Midi Through:Midi Through Port-0 14:0").usb, None);
}

// ---- the command ----------------------------------------------------------------

fn detect(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_osc-bridge")).arg("detect").args(args).output().unwrap()
}

#[test]
fn detect_read_replays_a_capture_against_the_catalogue_of_devices_dir() {
    let dir = temp("detect-read");
    files(dir.path(), &[("c.json", &minilab3_text())]);
    let catalogue = corpus_catalogue();
    let out = detect(&["--read", dir.path().join("c.json").to_str().unwrap(), "--devices-dir", catalogue.path().to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{} {}", stdout, String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("Minilab3:Minilab3 MIDI 40:0"), "{stdout}");
    assert!(stdout.contains("usb: 1c75:020b"), "{stdout}");
    assert!(stdout.contains("arturia/minilab3"), "{stdout}");
}

#[test]
fn detect_read_refuses_no_probe_and_write_because_a_replay_sends_nothing() {
    let dir = temp("detect-read-refused");
    files(dir.path(), &[("c.json", &minilab3_text())]);
    let c = dir.path().join("c.json");
    let c = c.to_str().unwrap();
    let w = dir.path().join("out.json");
    for args in [vec!["--read", c, "--no-probe"], vec!["--read", c, "--write", w.to_str().unwrap()]] {
        let out = detect(&args);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}");
        assert!(err.contains("--read") && (err.contains("--no-probe") || err.contains("--write")), "{args:?}: {err}");
    }
    assert!(!w.exists(), "nothing is written by a replay");
}

#[test]
fn detect_read_of_a_capture_of_an_unknown_version_names_the_version() {
    let dir = temp("detect-read-version");
    let mut v: Value = serde_json::from_str(&minilab3_text()).unwrap();
    v["format_version"] = json!(9999);
    files(dir.path(), &[("c.json", &v.to_string())]);
    let out = detect(&["--read", dir.path().join("c.json").to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("9999"));
}

#[test]
fn detect_publishes_write_beside_no_probe_and_devices_dir_without_scanning() {
    // The help is answered before any scan: no MIDI system of the machine is opened.
    let out = detect(&["--help"]);
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in ["--write <FILE>", "--no-probe", "--devices-dir <DEVICES_DIR>"] {
        assert!(help.contains(flag), "{flag} missing from: {help}");
    }
    // `--write` conflicts only with `--learn` and `--read`: refused by the arguments, before any scan.
    let dir = temp("detect-write");
    let w = dir.path().join("out.json");
    let out = detect(&["--write", w.to_str().unwrap(), "--read", w.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(!w.exists());
}

// ---- the fingerprints of the corpus -----------------------------------------------

/// A wire that plays back what a capture received.
struct Played(Vec<osc_bridge::detect::Heard>);

impl osc_bridge::detect::Wire for Played {
    fn send(&mut self, _port: &str, _msg: &[u8]) {}
    fn heard(&mut self, _listen: std::time::Duration) -> Vec<osc_bridge::detect::Heard> { self.0.clone() }
}

#[test]
fn the_bench_of_the_21st_of_november_2026_learns_one_fingerprint_per_device_with_the_words_of_its_ports() {
    use osc_bridge::detect::{learn, units, Heard};
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/captures");
    let capture = Capture::parse(&std::fs::read_to_string(dir.join("2026-11-21.json")).unwrap()).unwrap();
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("2026-11-21.fingerprints.expected.json")).unwrap()).unwrap();
    let all = units(&capture);
    assert_eq!(all.len(), expected.as_object().unwrap().len(), "one device per expected entry: {all:?}");
    for unit in &all {
        let client = unit.inputs.first().or(unit.outputs.first()).unwrap().split(':').next().unwrap().to_string();
        let want = expected.get(&client).unwrap_or_else(|| panic!("{client} is not in the expected file"));
        let heard: Vec<Heard> = capture.received.iter()
            .filter(|r| unit.inputs.contains(&r.port))
            .map(|r| Heard { port: r.port.clone(), at: std::time::Duration::from_millis(r.ms), msg: r.bytes.clone() })
            .collect();
        let l = learn(unit, &mut Played(heard), std::time::Duration::from_millis(5000), "2026-11-21");
        osd::read(&l.fingerprint.to_string(), osd::Kind::Fingerprint).unwrap_or_else(|e| panic!("{client}: {e}"));
        assert_eq!(l.fingerprint["ports"]["inputs"], want["inputs"], "{client}");
        assert_eq!(l.fingerprint["ports"]["outputs"], want["outputs"], "{client}");
    }
}

/// A device built in memory from the port names the system gave, in that order.
fn unit_of(inputs: &[&str], outputs: &[&str], usb: Option<(u16, u16)>) -> osc_bridge::detect::Unit {
    osc_bridge::detect::Unit {
        inputs: inputs.iter().map(|s| s.to_string()).collect(),
        outputs: outputs.iter().map(|s| s.to_string()).collect(),
        in_indexes: (0..inputs.len()).collect(),
        out_indexes: (0..outputs.len()).collect(),
        usb,
        client_kind: osc_bridge::detect::ClientKind::Unknown,
    }
}

fn learned_ports(unit: &osc_bridge::detect::Unit) -> Value {
    let l = osc_bridge::detect::learn(unit, &mut Played(vec![]), std::time::Duration::from_millis(1000), "2026-11-21");
    osd::read(&l.fingerprint.to_string(), osd::Kind::Fingerprint).unwrap();
    l.fingerprint["ports"].clone()
}

#[test]
fn a_fingerprint_lists_two_ports_of_one_name_as_two_entries() {
    let osmose = unit_of(&["Osmose:Osmose  48:0", "Osmose:Osmose  48:1"], &["Osmose:Osmose  48:0", "Osmose:Osmose  48:1"], Some((0x2e14, 0x0010)));
    let ports = learned_ports(&osmose);
    assert_eq!(ports["inputs"], serde_json::json!(["osmose", "osmose"]));
    assert_eq!(ports["outputs"], serde_json::json!(["osmose", "osmose"]));
}

#[test]
fn a_fingerprint_of_a_one_port_device_lists_one_entry() {
    let deluge = unit_of(&["Deluge:Deluge OUT 32:0"], &["Deluge:Deluge OUT 32:0"], Some((0x16d0, 0x0ce2)));
    let ports = learned_ports(&deluge);
    assert_eq!(ports["inputs"].as_array().unwrap().len(), 1, "{ports}");
    assert_eq!(ports["outputs"].as_array().unwrap().len(), 1, "{ports}");
}

#[test]
fn a_fingerprint_counts_the_ports_of_a_device_with_distinct_and_repeated_names() {
    let brute = unit_of(
        &["MatrixBrute:MatrixBrute 36:0", "MatrixBrute:MatrixBrute MIDI Interface 36:1"],
        &["MatrixBrute:MatrixBrute 36:0", "MatrixBrute:MatrixBrute MIDI Interface 36:1"], None);
    assert_eq!(learned_ports(&brute)["inputs"].as_array().unwrap().len(), 2);
    let three = unit_of(&["X:X A 1:0", "X:X A 1:1", "X:X A 1:2"], &["X:X A 1:0"], None);
    let ports = learned_ports(&three);
    assert_eq!(ports["inputs"].as_array().unwrap().len(), 3, "{ports}");
    assert_eq!(ports["outputs"].as_array().unwrap().len(), 1, "{ports}");
}

#[test]
fn a_fingerprint_keeps_the_ports_in_the_order_of_the_listing_not_sorted() {
    let unit = unit_of(&["Z:Z Zulu 5:0", "Z:Z Alpha 5:1", "Z:Z Mike 5:2"], &["Z:Z Mike 5:0", "Z:Z Zulu 5:1", "Z:Z Alpha 5:2"], None);
    let ports = learned_ports(&unit);
    assert_eq!(ports["inputs"], serde_json::json!(["z zulu", "z alpha", "z mike"]));
    assert_eq!(ports["outputs"], serde_json::json!(["z mike", "z zulu", "z alpha"]));
}

// ---- a port of a program is not a device ------------------------------------------

const FLUID: &str = "FLUID Synth (4242):Synth input port (4242:0) 128:0";

/// The catalogue of one hardware card whose name is the name of the software synth.
fn fluid_catalogue() -> tempfile::TempDir {
    let root = temp("fluid-catalogue");
    files(root.path(), &[("acme/fluid.json", &device05("FLUID Synth", "/fluid", ""))]);
    root
}

fn corpus_capture(name: &str) -> Capture {
    Capture::parse(&std::fs::read_to_string(captures_dir().join(name)).unwrap()).unwrap()
}

/// The capture of the software ports with its `clients` text changed.
fn software_capture_with_clients(clients: &str) -> Capture {
    let mut v: Value = serde_json::from_str(&corpus_capture("2026-11-22-ports-logiciels.json").to_json()).unwrap();
    v["clients"] = json!(clients);
    Capture::parse(&v.to_string()).unwrap()
}

fn device_of_port<'a>(devices: &'a [Value], port: &str) -> &'a Value {
    devices.iter().find(|d| d["port"] == port).unwrap_or_else(|| panic!("{port} is not in {devices:?}"))
}

#[test]
fn a_capture_is_written_in_version_two_with_the_raw_list_of_the_clients_of_the_system() {
    assert_eq!(FORMAT_VERSION, 2);
    let c = Capture::parse(&minilab3_text()).unwrap();
    let v: Value = serde_json::from_str(&c.to_json()).unwrap();
    assert_eq!(v["format_version"], json!(2));
    assert_eq!(v["clients"], json!(MINILAB3_CLIENTS));
}

#[test]
fn a_capture_of_a_machine_keeps_the_list_of_clients_the_system_gave_and_no_other_file_of_it() {
    let asound = temp("asound-clients");
    files(asound.path(), &[
        ("cards", ""),
        ("seq/clients", MINILAB3_CLIENTS),
        ("seq/timer", "SECRET-TIMER\n"),
    ]);
    let c = Capture::new(vec![], vec![], Asound::read(asound.path()));
    let v: Value = serde_json::from_str(&c.to_json()).unwrap();
    assert_eq!(v["clients"], json!(MINILAB3_CLIENTS));
    assert!(!c.to_json().contains("SECRET-TIMER"));
}

#[test]
fn a_system_with_no_list_of_clients_gives_an_empty_one() {
    let asound = temp("asound-no-clients");
    files(asound.path(), &[("cards", "")]);
    let c = Capture::new(vec![], vec![], Asound::read(asound.path()));
    assert_eq!(serde_json::from_str::<Value>(&c.to_json()).unwrap()["clients"], json!(""));
}

#[test]
fn a_capture_of_version_one_is_refused_naming_the_version_and_asking_to_capture_again() {
    let mut v: Value = serde_json::from_str(&minilab3_text()).unwrap();
    v["format_version"] = json!(1);
    v.as_object_mut().unwrap().remove("clients");
    let err = Capture::parse(&v.to_string()).expect_err("refused").to_string();
    assert!(err.contains("version 1"), "{err}");
    assert!(err.contains("capture again"), "{err}");
}

#[test]
fn every_capture_of_the_corpus_is_in_version_two_with_its_list_of_clients() {
    let mut seen = 0;
    for e in std::fs::read_dir(captures_dir()).unwrap() {
        let name = e.unwrap().file_name().to_string_lossy().to_string();
        if !name.ends_with(".json") || name.ends_with(".expected.json") || name.ends_with(".fingerprints.expected.json") { continue; }
        let raw: Value = read_json(&name);
        assert_eq!(raw["format_version"], json!(2), "{name}");
        assert!(raw["clients"].as_str().is_some_and(|c| c.contains("Client")), "{name} holds no list of clients");
        seen += 1;
    }
    assert!(seen >= 4, "{seen} captures");
}

#[test]
fn the_replay_reads_the_kind_of_each_port_from_the_clients_of_the_capture() {
    let devices = osc_bridge::mcp::detect_capture(fluid_catalogue().path(), &corpus_capture("2026-11-22-ports-logiciels.json")).unwrap();
    assert_eq!(device_of_port(&devices, FLUID)["client_kind"], "user");
    assert_eq!(device_of_port(&devices, "Pure Data:Pure Data Midi-In 1 133:0")["client_kind"], "user");
    assert_eq!(device_of_port(&devices, "Midi Through:Midi Through Port-0 14:0")["client_kind"], "kernel");
    let bench = osc_bridge::mcp::detect_capture(corpus_catalogue().path(), &corpus_capture("2026-11-21.json")).unwrap();
    assert_eq!(device_of_port(&bench, "Minilab3:Minilab3 MIDI 40:0")["client_kind"], "kernel");
    assert_eq!(device_of_port(&bench, "Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 24:0")["client_kind"], "kernel");
}

#[test]
fn a_port_whose_client_is_not_in_the_list_has_the_kind_unknown() {
    let devices = osc_bridge::mcp::detect_capture(fluid_catalogue().path(), &corpus_capture("2026-11-22-ports-logiciels.json")).unwrap();
    assert_eq!(device_of_port(&devices, "Microsoft GS Wavetable Synth:Microsoft GS Wavetable Synth 0:0")["client_kind"], "unknown");
}

#[test]
fn a_system_whose_clients_are_not_readable_gives_every_port_the_kind_unknown() {
    let capture = software_capture_with_clients("");
    let devices = osc_bridge::mcp::detect_capture(fluid_catalogue().path(), &capture).unwrap();
    assert!(!devices.is_empty());
    for d in &devices { assert_eq!(d["client_kind"], "unknown", "{d}"); }
}

#[test]
fn a_port_whose_link_to_a_client_is_ambiguous_has_the_kind_unknown() {
    let twice = "Client info\n\nClient 128 : \"FLUID Synth (4242)\" [User]\n  Port   0 : \"Synth input port (4242:0)\" (Rwe-)\n\
                 Client 128 : \"FLUID Synth (4242)\" [Kernel]\n  Port   0 : \"Synth input port (4242:0)\" (Rwe-)\n";
    let devices = osc_bridge::mcp::detect_capture(fluid_catalogue().path(), &software_capture_with_clients(twice)).unwrap();
    assert_eq!(device_of_port(&devices, FLUID)["client_kind"], "unknown");
}

#[test]
fn a_port_of_a_program_gets_no_hardware_candidate_whatever_its_name() {
    let devices = osc_bridge::mcp::detect_capture(fluid_catalogue().path(), &corpus_capture("2026-11-22-ports-logiciels.json")).unwrap();
    let fluid = device_of_port(&devices, FLUID);
    assert!(fluid["candidates"].as_array().unwrap().is_empty(), "{fluid}");
    assert!(fluid["run_command"].is_null(), "{fluid}");
}

#[test]
fn the_same_name_on_a_kernel_client_still_gets_its_candidate() {
    let kernel = software_capture_with_clients(
        "Client   0 : \"System\" [Kernel]\nClient 128 : \"FLUID Synth (4242)\" [Kernel]\n  Port   0 : \"Synth input port (4242:0)\" (RWe-)\n");
    let devices = osc_bridge::mcp::detect_capture(fluid_catalogue().path(), &kernel).unwrap();
    assert_eq!(device_of_port(&devices, FLUID)["candidates"][0]["device"], "acme/fluid");
}

#[test]
fn a_port_of_the_kind_unknown_is_matched_as_before() {
    let devices = osc_bridge::mcp::detect_capture(fluid_catalogue().path(), &software_capture_with_clients("")).unwrap();
    assert_eq!(device_of_port(&devices, FLUID)["candidates"][0]["device"], "acme/fluid");
}

#[test]
fn a_port_of_a_program_gets_no_candidate_even_with_the_usb_ids_of_a_card_declared() {
    let program = json!({
        "format_version": FORMAT_VERSION,
        "clients": "Client  40 : \"Minilab3\" [User]\n  Port   0 : \"Minilab3 MIDI\" (RWe-)\n",
        "outputs": ["Minilab3:Minilab3 MIDI 40:0"], "inputs": ["Minilab3:Minilab3 MIDI 40:0"],
        "cards": " 6 [Minilab3       ]: USB-Audio - Minilab3\n", "usbids": [{"card": 6, "text": "1c75:020b\n"}],
        "requests": [], "received": [{"port": "Minilab3:Minilab3 MIDI 40:0", "bytes": minilab_reply(), "ms": 4}],
    });
    let devices = osc_bridge::mcp::detect_capture(corpus_catalogue().path(), &Capture::parse(&program.to_string()).unwrap()).unwrap();
    assert_eq!(devices[0]["client_kind"], "user");
    assert!(devices[0]["candidates"].as_array().unwrap().is_empty(), "{}", devices[0]);
}

#[test]
fn the_minilab3_of_the_kernel_is_recognised_as_before() {
    let devices = osc_bridge::mcp::detect_capture(corpus_catalogue().path(), &corpus_capture("2026-11-21.json")).unwrap();
    let minilab = device_of_port(&devices, "Minilab3:Minilab3 MIDI 40:0");
    assert_eq!(minilab["client_kind"], "kernel");
    assert_eq!(minilab["candidates"][0]["device"], "arturia/minilab3");
    assert!(minilab["run_command"].is_string(), "{minilab}");
}
