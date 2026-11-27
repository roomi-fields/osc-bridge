//! A candidate carries its proofs, each with its nature (strong, medium, weak): the name alone
//! is a weak proof, never a recognition, and a name made of common words is no proof at all.
//! A proof is `{"nature": "strong" | "medium" | "weak", "kind": "identity reply" | "usb id" |
//! "maker" | "name"}`; a candidate with no strong proof is `state_label: "possible, to confirm"`.

use crate::common::{device05, files, temp};
use osc_bridge::capture::Capture;
use osc_bridge::synthub::learn::{learn_report, Learned, ServiceMatches, Unit};
use osc_bridge::recognition::{Candidate, Identity};
use osc_bridge::mcp::DeviceIndex;
use osc_bridge::synthub::catalogue::tier_rank;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// `match_port` with the catalogue's order of tiers, the order of every caller of the bridge.
fn match_port(cat: &[Candidate], port: &str, usb: Option<(u16, u16)>, replies: &[Vec<u8>]) -> Vec<Value> {
    osc_bridge::mcp::match_port(cat, port, usb, replies, tier_rank)
}

/// `detect_capture` with the catalogue's order of tiers.
fn detect_capture(dir: &Path, capture: &Capture) -> Result<Vec<Value>, String> {
    osc_bridge::mcp::detect_capture(dir, capture, tier_rank)
}

const POSSIBLE: &str = "possible, to confirm";

fn captures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/captures")
}

fn capture(name: &str) -> Capture {
    Capture::parse(&std::fs::read_to_string(captures_dir().join(name)).unwrap()).unwrap()
}

fn usb_body(v: u16, p: u16) -> String {
    format!(r#","identity":{{"usb":[{{"vendor_id":{v},"product_id":{p}}}]}}"#)
}

/// The catalogue of the corpus: each driver declares its USB ids.
fn corpus_catalogue() -> tempfile::TempDir {
    let root = temp("proofs-corpus");
    files(root.path(), &[
        ("behringer/bcr32.json", &device05("BCR-32", "/bcr32", &usb_body(0x1397, 0x12cf))),
        ("electra-one/electra-one.json", &device05("Electra One", "/electra", &usb_body(0x1fc9, 0x82cf))),
        ("arturia/minilab3.json", &device05("MiniLab 3", "/minilab3", &usb_body(0x1c75, 0x020b))),
    ]);
    root
}

fn natures(candidate: &Value) -> Vec<String> {
    let mut v: Vec<String> = candidate["proofs"].as_array().unwrap_or_else(|| panic!("no proofs: {candidate}"))
        .iter().map(|p| p["nature"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

fn kinds(candidate: &Value) -> Vec<String> {
    let mut v: Vec<String> = candidate["proofs"].as_array().unwrap_or_else(|| panic!("no proofs: {candidate}"))
        .iter().map(|p| p["kind"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

fn driver(id: &str, name: &str, vendor: &str, maker: Vec<u8>, identity: Option<osd::Identity>) -> Candidate {
    Candidate { id: id.into(), slug: id.rsplit('/').next().unwrap().into(), name: name.into(), vendor: vendor.into(),
        file: format!("{id}.json"), tier: "official".into(), manufacturer_id: maker, identity, firmware: None, midi_of: None }
}

fn usb_identity(v: u16, p: u16) -> osd::Identity {
    crate::common::device_of(&device05("X", "/x", &usb_body(v, p))).identity.unwrap()
}

// ---- a candidate carries its proofs ----------------------------------------------

#[test]
fn a_usb_id_the_card_declares_is_a_strong_proof() {
    let cat = vec![driver("acme/box", "Box", "Acme", vec![], Some(usb_identity(0x1234, 0x5678)))];
    let got = match_port(&cat, "USB MIDI Interface", Some((0x1234, 0x5678)), &[]);
    assert_eq!(got[0]["id"], "acme/box", "{got:?}");
    assert_eq!(natures(&got[0]), ["strong"], "{got:?}");
    assert_eq!(kinds(&got[0]), ["usb id"], "{got:?}");
    assert!(got[0].get("recognised_by").is_none(), "proofs replace recognised_by: {got:?}");
    assert!(got[0].get("state_label").is_none(), "a strong proof is no 'to confirm': {got:?}");
}

#[test]
fn the_identity_reply_the_card_recognises_is_a_strong_proof() {
    let cat = vec![driver("arturia/minilab3", "MiniLab 3", "Arturia", vec![0x00, 0x20, 0x6B], None)];
    let reply = Identity { manufacturer: vec![0x00, 0x20, 0x6B], family: 2, model: 4, version: [1, 2, 3, 4] }.reply();
    // The card names the reply it recognises through the fixture driver of the catalogue.
    let index = DeviceIndex::build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/devices"), osc_bridge::synthub::catalogue::tier_rank);
    assert!(index.find("acme/box.vendor.fw-2").is_some());
    let acme = Identity { manufacturer: vec![0, 33, 9], family: 1, model: 1, version: [2, 0, 0, 0] }.reply();
    let fixtures = osc_bridge::mcp::hardware_candidates(&index);
    let got = match_port(&fixtures, "Box", None, &[acme]);
    assert_eq!(got[0]["id"], "acme/box.vendor.fw-2", "{got:?}");
    assert!(kinds(&got[0]).contains(&"identity reply".to_string()), "{got:?}");
    assert!(natures(&got[0]).contains(&"strong".to_string()), "{got:?}");
    // The maker of a reply alone, with no card recognising it, is a medium proof, never a strong one.
    let maker_only = match_port(&cat, "MiniLab 3", None, &[reply]);
    assert_eq!(natures(&maker_only[0]), ["medium", "weak"], "{maker_only:?}");
    assert_eq!(kinds(&maker_only[0]), ["maker", "name"], "{maker_only:?}");
}

#[test]
fn the_name_of_a_device_alone_is_a_weak_proof_and_the_candidate_is_possible_to_confirm() {
    let cat = vec![driver("elektron/osmose", "Osmose", "Expressive E", vec![], None)];
    let got = match_port(&cat, "Osmose:Osmose MIDI 1 24:0", None, &[]);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(natures(&got[0]), ["weak"], "{got:?}");
    assert_eq!(kinds(&got[0]), ["name"], "{got:?}");
    assert_eq!(got[0]["state_label"], POSSIBLE, "{got:?}");
}

#[test]
fn a_name_found_in_the_port_gives_the_weak_proof_of_each_of_the_cases_of_a_driver() {
    let cat = vec![driver("korg/minilogue", "minilogue", "Korg", vec![], None),
                   driver("novation/bass_station_ii", "Bass Station II", "Novation", vec![], None)];
    for (port, want) in [("minilogue MIDI 1", "korg/minilogue"), ("Bass Station II", "novation/bass_station_ii")] {
        let got = match_port(&cat, port, None, &[]);
        assert_eq!(got[0]["id"], want, "{port}: {got:?}");
        assert_eq!(natures(&got[0]), ["weak"], "{port}: {got:?}");
        assert_eq!(got[0]["state_label"], POSSIBLE, "{port}: {got:?}");
    }
}

#[test]
fn a_usb_id_with_a_name_keeps_both_proofs_and_is_not_possible_to_confirm() {
    let cat = vec![driver("acme/box", "Box", "Acme", vec![], Some(usb_identity(0x1234, 0x5678)))];
    let got = match_port(&cat, "Box MIDI 1", Some((0x1234, 0x5678)), &[]);
    assert_eq!(natures(&got[0]), ["strong", "weak"], "{got:?}");
    assert!(got[0].get("state_label").is_none(), "{got:?}");
}

#[test]
fn another_usb_id_than_the_one_declared_is_no_proof() {
    let cat = vec![driver("acme/box", "Box", "Acme", vec![], Some(usb_identity(0x1234, 0x5678)))];
    assert!(match_port(&cat, "USB MIDI Interface", Some((1, 2)), &[]).is_empty());
    assert!(match_port(&cat, "USB MIDI Interface", None, &[]).is_empty());
}

// ---- a name of common words is no proof -------------------------------------------

#[test]
fn a_port_name_made_of_common_words_gets_no_candidate_whatever_the_catalogue_names() {
    let cat = vec![
        driver("polyend/synth", "Synth", "Polyend", vec![], None),
        driver("acme/controller", "Controller", "Acme", vec![], None),
        driver("acme/audio-interface", "Audio Interface", "Acme", vec![], None),
        driver("acme/mini", "Mini", "Acme", vec![], None),
        driver("acme/midi-controller", "MIDI Controller", "Acme", vec![], None),
        driver("acme/port", "Port", "Acme", vec![], None),
    ];
    for port in ["FLUID Synth (4242):Synth input port (4242:0) 128:0", "Microsoft GS Wavetable Synth:Microsoft GS Wavetable Synth 0:0",
                 "Synth input port", "Controller:Controller 130:0", "Audio Interface MIDI:Audio Interface MIDI 131:0",
                 "Mini Output Port", "Midi Through:Midi Through Port-0 14:0"] {
        let got = match_port(&cat, port, None, &[]);
        assert!(got.is_empty(), "{port}: {got:?}");
    }
}

#[test]
fn a_common_word_next_to_a_word_that_names_the_device_still_gives_the_weak_proof() {
    let cat = vec![driver("elektron/osmose", "Osmose Synth", "Expressive E", vec![], None)];
    let got = match_port(&cat, "Osmose:Osmose MIDI 1 24:0", None, &[]);
    assert_eq!(natures(&got[0]), ["weak"], "{got:?}");
}

#[test]
fn the_software_ports_of_the_corpus_get_no_candidate_from_a_catalogue_of_common_names() {
    let root = temp("proofs-common");
    files(root.path(), &[
        ("polyend/synth.json", &device05("Synth", "/synth", "")),
        ("acme/controller.json", &device05("Controller", "/controller", "")),
        ("acme/audio-interface.json", &device05("Audio Interface", "/ai", "")),
    ]);
    let devices = detect_capture(root.path(), &capture("2026-11-22-ports-logiciels.json")).unwrap();
    assert!(!devices.is_empty());
    for d in &devices {
        assert!(d["candidates"].as_array().unwrap().is_empty(), "{d}");
        assert!(d["run_command"].is_null(), "{d}");
    }
}

// ---- the run command ---------------------------------------------------------------

#[test]
fn a_name_alone_proposes_no_run_command_and_is_never_the_answer_of_the_port() {
    let root = temp("proofs-name");
    files(root.path(), &[("acme/osmose.json", &device05("Osmose", "/osmose", ""))]);
    let cap = Capture::parse(&json!({
        "format_version": osc_bridge::capture::FORMAT_VERSION,
        "outputs": ["Osmose:Osmose MIDI 1 24:0"], "inputs": ["Osmose:Osmose MIDI 1 24:0"],
        "cards": "", "usbids": [], "clients": "", "requests": [], "received": [],
    }).to_string()).unwrap();
    let devices = detect_capture(root.path(), &cap).unwrap();
    let c = &devices[0]["candidates"][0];
    assert_eq!(c["device"], "acme/osmose", "{devices:?}");
    assert_eq!(natures(c), ["weak"], "{devices:?}");
    assert_eq!(c["state_label"], POSSIBLE, "{devices:?}");
    assert!(devices[0]["run_command"].is_null(), "{devices:?}");
}

#[test]
fn a_strong_proof_without_a_rival_proposes_the_run_command() {
    let devices = detect_capture(corpus_catalogue().path(), &capture("2026-11-21.json")).unwrap();
    let minilab = devices.iter().find(|d| d["port"] == "Minilab3:Minilab3 MIDI 40:0").expect("the MiniLab 3 port");
    assert_eq!(minilab["candidates"][0]["device"], "arturia/minilab3", "{minilab}");
    assert!(natures(&minilab["candidates"][0]).contains(&"strong".to_string()), "{minilab}");
    assert!(minilab["run_command"].as_str().unwrap().contains("arturia"), "{minilab}");
}

#[test]
fn two_candidates_with_a_strong_proof_on_one_port_propose_no_run_command_and_show_both_proofs() {
    let root = temp("proofs-rival");
    files(root.path(), &[
        ("acme/one.json", &device05("One", "/one", &usb_body(0x1c75, 0x020b))),
        ("acme/two.json", &device05("Two", "/two", &usb_body(0x1c75, 0x020b))),
    ]);
    let devices = detect_capture(root.path(), &capture("2026-11-21.json")).unwrap();
    let port = devices.iter().find(|d| d["port"] == "Minilab3:Minilab3 MIDI 40:0").unwrap();
    let cands = port["candidates"].as_array().unwrap();
    assert_eq!(cands.len(), 2, "{port}");
    for c in cands { assert_eq!(natures(c), ["strong"], "{port}"); }
    assert!(port["run_command"].is_null(), "{port}");
}

// ---- the corpus ----------------------------------------------------------------------

#[test]
fn the_bench_is_recognised_on_the_ports_that_answer_and_midi_through_gets_nothing() {
    let devices = detect_capture(corpus_catalogue().path(), &capture("2026-11-21-banc-7-appareils.json")).unwrap();
    let on = |client: &str| -> Vec<&Value> { devices.iter().filter(|d| d["port"].as_str().unwrap().starts_with(client)).collect() };
    for (client, id) in [("Minilab3", "arturia/minilab3"), ("Electra Controller", "electra-one/electra-one")] {
        assert!(!on(client).is_empty(), "{client}");
        for d in on(client) {
            assert_eq!(d["candidates"][0]["device"], id, "{d}");
            assert!(natures(&d["candidates"][0]).contains(&"strong".to_string()), "{d}");
        }
    }
    for d in on("Midi Through") {
        assert!(d["candidates"].as_array().unwrap().is_empty(), "{d}");
    }
}

// ---- the learn mode ---------------------------------------------------------------------

fn unit(port: &str, usb: Option<(u16, u16)>) -> Unit {
    Unit { inputs: vec![port.into()], outputs: vec![port.into()], in_indexes: vec![0], out_indexes: vec![0], usb, client_kind: osc_bridge::detect::ClientKind::Unknown }
}

fn learned(port: &str) -> Learned {
    let words = osc_bridge::synthub::learn::port_words(port);
    Learned {
        fingerprint: json!({ "$schema": "https://w3id.org/osd/0.7/fingerprint.schema.json", "observed": "2026-11-21",
            "via": "native", "ports": { "inputs": [words], "outputs": [words] },
            "controls": [{ "type": "cc", "channel": 1, "number": 74 }] }),
        measured: json!({}),
    }
}

fn learn(root: &Path, port: &str, usb: Option<(u16, u16)>) -> Value {
    let index = DeviceIndex::build(root, osc_bridge::synthub::catalogue::tier_rank);
    learn_report(&[(unit(port, usb), learned(port))], &index, Duration::from_millis(5000), &ServiceMatches::NotLinked,
                 osc_bridge::synthub::catalogue::tier_rank)
}

#[test]
fn the_learn_mode_takes_over_when_the_only_proof_of_a_driver_is_the_name() {
    let root = temp("proofs-learn-name");
    files(root.path(), &[("acme/osmose.json", &device05("Osmose", "/osmose", ""))]);
    let r = learn(root.path(), "Osmose:Osmose MIDI 1 24:0", None);
    let d = &r["structuredContent"]["devices"][0];
    assert!(d.get("driver").is_none(), "{r}");
    assert!(d["next_step"].as_str().is_some(), "{r}");
}

#[test]
fn the_learn_mode_names_the_driver_of_a_strong_proof_with_its_proofs() {
    let root = temp("proofs-learn-usb");
    files(root.path(), &[("arturia/minilab3.json", &device05("MiniLab 3", "/minilab3", &usb_body(0x1c75, 0x020b)))]);
    let r = learn(root.path(), "Minilab3:Minilab3 MIDI 40:0", Some((0x1c75, 0x020b)));
    let d = &r["structuredContent"]["devices"][0];
    assert_eq!(d["driver"]["id"], "arturia/minilab3", "{r}");
    assert!(natures(&d["driver"]).contains(&"strong".to_string()), "{r}");
    assert!(d.get("next_step").is_none(), "{r}");
}
