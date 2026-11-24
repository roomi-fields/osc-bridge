//! The MIDI the detector, the learn mode and the tools reach is the one their opener gives: a fake
//! device answers the Identity Request without the machine's MIDI being touched, an opener that
//! fails gives the error that names the MIDI system and its cause, and the binary takes the opener
//! that fails by construction when the test variable asks for it.

use crate::common::{failing_opener, FakeHardware, MIDI_ENV};
use osc_bridge::detect::{scan, ClientKind};
use osc_bridge::recognition::{Probe, IDENTITY_REQUEST};
use osc_bridge::synthub::learn::{learn_all, learn_units, units};
use serde_json::{json, Value};
use std::time::Duration;

const PORT: &str = "Acme Box:Acme Box MIDI 1 20:0";
/// An Identity Reply: Arturia (00 20 6B), family 0x0002, member 0x0004, version 1.0.4.0.
const REPLY: [u8; 17] = [0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x00, 0x20, 0x6B, 0x02, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0xF7];
const WAIT: Duration = Duration::from_millis(100);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn maker_probe(request: u8) -> Probe {
    Probe { request: vec![0xF0, 0x7D, request, 0xF7], ports: vec![], usb: vec![], match_ports: vec![] }
}

// ---- the detector ----

#[test]
fn a_scan_through_a_fake_device_recognises_the_device_that_answers_the_identity_request() {
    let hardware = FakeHardware::device(PORT, &REPLY);
    let found = scan(&hardware.opener(), true, WAIT, &[]).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    let identity = found[0].identity.as_ref().expect("the fake device answered");
    assert_eq!(identity.manufacturer, vec![0x00, 0x20, 0x6B]);
    assert_eq!(identity.version, [1, 0, 4, 0]);
    assert_eq!(found[0].replies, vec![REPLY.to_vec()]);
    assert_eq!(hardware.sent_messages(), vec![(PORT.to_string(), IDENTITY_REQUEST.to_vec())], "the universal request and nothing else");
}

#[test]
fn a_scan_without_probe_sends_nothing_and_hears_nothing_from_the_fake_device() {
    let hardware = FakeHardware::device(PORT, &REPLY);
    let found = scan(&hardware.opener(), false, WAIT, &[]).unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].identity.is_none() && found[0].replies.is_empty(), "{found:?}");
    assert!(hardware.sent_messages().is_empty());
}

#[test]
fn a_scan_through_a_fake_with_no_device_finds_no_port() {
    assert!(scan(&FakeHardware::none().opener(), true, WAIT, &[]).unwrap().is_empty());
}

#[test]
fn a_fake_device_with_no_listing_of_its_own_gives_no_usb_ids_and_no_client_kind_whatever_the_machine_holds() {
    let hardware = FakeHardware::device(PORT, &REPLY);
    let found = scan(&hardware.opener(), true, WAIT, &[]).unwrap();
    assert_eq!(found[0].usb, None, "{found:?}");
    assert_eq!(found[0].client_kind, ClientKind::Unknown);
}

#[test]
fn a_fake_device_carries_its_own_cards_usb_ids_and_clients() {
    let port = "Minilab3:Minilab3 MIDI 40:0";
    let hardware = FakeHardware::device(port, &REPLY).with_asound(
        " 3 [Minilab3       ]: USB-Audio - Minilab3\n                      Arturia Minilab3 at usb-0000:00:14.0-2\n",
        &[(3, "1c75:020b\n")],
        "Client  40 : \"Minilab3\" [Kernel]\n",
    );
    let found = scan(&hardware.opener(), true, WAIT, &[]).unwrap();
    assert_eq!(found[0].usb, Some((0x1c75, 0x020b)), "{found:?}");
    assert_eq!(found[0].client_kind, ClientKind::Kernel);
}

#[test]
fn a_scan_whose_opener_fails_says_the_midi_system_cannot_be_opened_and_the_cause() {
    let error = scan(&failing_opener("no sequencer here"), true, WAIT, &[]).unwrap_err().to_string();
    assert!(error.contains("MIDI system cannot be opened"), "{error}");
    assert!(error.contains("no sequencer here"), "{error}");
}

#[test]
fn an_observation_whose_opener_fails_says_the_midi_system_cannot_be_opened_and_the_cause() {
    let error = osc_bridge::detect::observe(&failing_opener("no sequencer here"), false, WAIT, &[], &[]).unwrap_err().to_string();
    assert!(error.contains("MIDI system cannot be opened") && error.contains("no sequencer here"), "{error}");
}

#[test]
fn an_observation_through_the_fake_sends_a_maker_request_only_to_the_port_the_person_consented_to() {
    let hardware = FakeHardware::device(PORT, &REPLY).with_device("Other Box:Other Box MIDI 1 24:0", &REPLY);
    let probes = [maker_probe(0x21)];
    let held = osc_bridge::detect::observe(&hardware.opener(), true, WAIT, &probes, &[]).unwrap();
    assert!(held.requests.iter().all(|r| r.bytes == IDENTITY_REQUEST), "{:?}", held.requests);
    let consented = osc_bridge::detect::observe(&hardware.opener(), true, WAIT, &probes, &["acme".to_string()]).unwrap();
    let maker: Vec<_> = consented.requests.iter().filter(|r| r.bytes == probes[0].request).collect();
    assert_eq!(maker.len(), 1, "{:?}", consented.requests);
    assert_eq!(maker[0].port, PORT);
}

// ---- the learn mode ----

#[test]
fn the_learn_mode_through_a_fake_device_gives_the_fingerprint_it_heard() {
    let hardware = FakeHardware::device(PORT, &REPLY).with_chatter(PORT, &[0xB0, 74, 64]);
    let learned = learn_all(&hardware.opener(), Duration::from_millis(1000)).unwrap();
    assert_eq!(learned.len(), 1);
    let fp = &learned[0].1.fingerprint;
    assert_eq!(fp["identity_reply"], hex(&REPLY), "{fp}");
    assert!(fp["controls"].as_array().unwrap().iter().any(|c| c["cc"] == 74), "{fp}");
    assert!(fp.get("usb").is_none(), "no card listed by the fake: {fp}");
    assert_eq!(hardware.sent_messages(), vec![(PORT.to_string(), IDENTITY_REQUEST.to_vec())]);
}

#[test]
fn the_learn_mode_on_the_units_given_asks_only_their_ports() {
    let other = "Other Box:Other Box MIDI 1 24:0";
    let hardware = FakeHardware::device(PORT, &REPLY).with_device(other, &REPLY);
    let seen = units(&osc_bridge::detect::observe(&hardware.opener(), false, WAIT, &[], &[]).unwrap());
    assert_eq!(seen.len(), 2, "{seen:?}");
    let mine: Vec<_> = seen.into_iter().filter(|u| u.outputs == [PORT]).collect();
    let learned = learn_units(&hardware.opener(), &mine, Duration::from_millis(1000)).unwrap();
    assert_eq!(learned.len(), 1);
    assert_eq!(hardware.sent_messages().iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), vec![PORT]);
}

#[test]
fn the_learn_mode_whose_opener_fails_says_the_midi_system_cannot_be_opened_and_the_cause() {
    let error = learn_all(&failing_opener("no sequencer here"), Duration::from_millis(1000)).unwrap_err().to_string();
    assert!(error.contains("MIDI system cannot be opened") && error.contains("no sequencer here"), "{error}");
    let hardware = FakeHardware::device(PORT, &REPLY);
    let one = units(&osc_bridge::detect::observe(&hardware.opener(), false, WAIT, &[], &[]).unwrap());
    let error = learn_units(&failing_opener("no sequencer here"), &one, Duration::from_millis(1000)).unwrap_err().to_string();
    assert!(error.contains("MIDI system cannot be opened") && error.contains("no sequencer here"), "{error}");
}

// ---- the tools ----

fn call(open: osc_bridge::midi_port::Opener, arguments: Value) -> Value {
    let dir = crate::common::temp("midi-through-opener");
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(dir.path().to_path_buf()),
        default_target: "127.0.0.1:9000".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: Some(osc_bridge::synthub::learn::detect_offline),
    };
    let input = format!("{}\n", crate::common::tool_call(1, "detect_hardware", arguments));
    let mut out = Vec::new();
    osc_bridge::mcp::serve_with_midi(opts, input.as_bytes(), &mut out, open).unwrap();
    serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap()
}

fn said(reply: &Value) -> String {
    format!("{} {}", reply["error"]["message"], crate::common::texts(reply).join(" "))
}

#[test]
fn detect_hardware_in_learn_mode_reports_the_fake_device_it_was_given() {
    let hardware = FakeHardware::device(PORT, &REPLY);
    let reply = call(&hardware.opener(), json!({ "learn": true, "listen_ms": 1000 }));
    let devices = reply["result"]["structuredContent"]["devices"].as_array().unwrap_or_else(|| panic!("{reply}"));
    assert_eq!(devices.len(), 1, "{reply}");
    assert_eq!(devices[0]["fingerprint"]["identity_reply"], hex(&REPLY), "{reply}");
}

#[test]
fn detect_hardware_in_learn_mode_returns_the_error_of_an_opener_that_fails() {
    let reply = call(&failing_opener("no sequencer here"), json!({ "learn": true, "listen_ms": 1000 }));
    assert!(reply.get("error").is_some() || reply["result"]["isError"] == true, "{reply}");
    let text = said(&reply);
    assert!(text.contains("MIDI system cannot be opened") && text.contains("no sequencer here"), "{text}");
}

#[test]
fn detect_hardware_reports_the_fake_device_and_the_error_of_an_opener_that_fails() {
    let hardware = FakeHardware::device(PORT, &REPLY);
    let reply = call(&hardware.opener(), json!({ "timeout_ms": 100 }));
    let devices = reply["result"]["structuredContent"]["devices"].as_array().unwrap_or_else(|| panic!("{reply}"));
    assert_eq!(devices.len(), 1, "{reply}");
    let reply = call(&failing_opener("no sequencer here"), json!({ "timeout_ms": 100 }));
    let text = said(&reply);
    assert!(text.contains("MIDI system cannot be opened") && text.contains("no sequencer here"), "{text}");
}

fn take(open: osc_bridge::midi_port::Opener) -> Value {
    let dir = crate::common::temp("midi-through-take");
    let server = crate::common::FakeSynthHub::start(dir.path(), None);
    let home = crate::common::ConfigHome::new();
    let replies = crate::common::mcp_session_midi(&home, open, &server.base, &dir.path().join("cache"),
        &[crate::common::initialize(), crate::common::tool_call(1, "take_sound", json!({}))]);
    replies.into_iter().find(|r| r["id"] == 1).unwrap()
}

#[test]
fn take_sound_looks_for_the_synth_on_the_ports_of_the_opener_it_was_given() {
    let hardware = FakeHardware::with_ports(&["Acme Box 20:0"]);
    let reply = take(&hardware.opener());
    let text = said(&reply);
    assert!(text.contains("no synth the librarian knows answered"), "{text}");
    assert!(hardware.sent_messages().iter().any(|(port, m)| port == "Acme Box 20:0" && *m == IDENTITY_REQUEST), "{:?}", hardware.sent_messages());
    let text = said(&take(&FakeHardware::none().opener()));
    assert!(text.contains("no MIDI output"), "{text}");
}

#[test]
fn take_sound_returns_the_error_of_an_opener_that_fails() {
    let text = said(&take(&failing_opener("no sequencer here")));
    assert!(text.contains("no sequencer here"), "{text}");
}

// ---- the binary ----

fn command(args: &[&str]) -> std::process::Output {
    let home = crate::common::temp("midi-through-binary");
    std::process::Command::new(env!("CARGO_BIN_EXE_osc-bridge"))
        .args(args)
        .env(MIDI_ENV, "failing")
        .env("XDG_CONFIG_HOME", home.path())
        .env("APPDATA", home.path())
        .output()
        .unwrap()
}

fn shown(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout)
}

#[test]
fn the_detect_command_fails_with_the_midi_system_message_when_the_opener_the_test_variable_names_fails() {
    let devices = crate::common::temp("midi-through-binary-devices");
    let out = command(&["detect", "--devices-dir", devices.path().to_str().unwrap()]);
    assert!(!out.status.success(), "{}", shown(&out));
    let text = shown(&out);
    assert!(text.contains("MIDI system cannot be opened") && text.contains(MIDI_ENV), "{text}");
}

#[test]
fn the_detect_learn_command_fails_with_the_midi_system_message_when_the_opener_the_test_variable_names_fails() {
    let out = command(&["detect", "--learn"]);
    assert!(!out.status.success(), "{}", shown(&out));
    let text = shown(&out);
    assert!(text.contains("MIDI system cannot be opened") && text.contains(MIDI_ENV), "{text}");
}
