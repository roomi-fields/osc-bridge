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

/// The capture of the bench's MiniLab 3, written by hand from the listing the system gave.
fn minilab3_text() -> String {
    json!({
        "format_version": FORMAT_VERSION,
        "outputs": ["Minilab3:Minilab3 MIDI 40:0"],
        "inputs": ["Minilab3:Minilab3 MIDI 40:0"],
        "cards": " 6 [Minilab3       ]: USB-Audio - Minilab3\n                      Arturia Minilab3 at usb-0000:00:14.0-2, full speed\n",
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
        "cards": "", "usbids": [],
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
    assert_eq!(keys, ["cards", "format_version", "inputs", "outputs", "received", "requests", "usbids"]);
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
        "usbids": [{"card": 6, "text": "1c75:020b\n"}],
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
        "usbids": [{"card": 2, "text": "1397:12cf\n"}],
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

/// Each capture of `dir` (`<name>.json`) is replayed over `catalogue` and compared with its
/// neighbour `<name>.expected.json`, a map port → device id (null: none). The errors name the
/// capture and the port.
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
        for d in &devices {
            let port = d["port"].as_str().unwrap();
            let got = d["candidates"][0]["device"].as_str();
            let want = expected.get(port).map(|w| w.as_str());
            match want {
                None => errors.push(format!("{n}.json: port {port}: not in {n}.expected.json")),
                Some(w) if w != got => errors.push(format!("{n}.json: port {port}: detected {got:?}, expected {w:?}")),
                Some(_) => checked += 1,
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
        Ok(n) => assert!(n >= 3, "the corpus holds the bench of 2026-11-21 (3 devices), {n} checked"),
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
fn detect_accepts_write_with_no_probe_and_devices_dir() {
    let dir = temp("detect-write");
    let w = dir.path().join("out.json");
    let out = detect(&["--write", w.to_str().unwrap(), "--no-probe", "--devices-dir", dir.path().to_str().unwrap()]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("unexpected argument") && !err.contains("cannot be used"), "{err}");
    if out.status.success() {
        let c = Capture::parse(&std::fs::read_to_string(&w).unwrap()).unwrap();
        let v: Value = serde_json::from_str(&c.to_json()).unwrap();
        assert_eq!(v["requests"], json!([]), "no request without a probe");
    }
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
