//! One generic gesture puts the export of a device on a recognised target device: the format
//! and the way come from the target's descriptor (what it receives, `resources`), the export is
//! built in that format, and the messages go by the declared way (a file written by SysEx, step
//! by step). Nothing is sent without the person's consent; nothing names a device.
//!
//! The gesture is `osc_bridge::install::install(source, target, confirm_overwrite, midi, clock)`
//! over the `midi_port::Midi` ports and the `programs::Clock` of the other gestures: small tests,
//! the ports are a recorded exchange. The tool and the command refuse before touching any port
//! when the target is not plugged.

use crate::common::{files, temp, tool_call, FakeHardware};
use osc_bridge::install::install;
use osc_bridge::programs::Clock;
use osc_bridge::midi_port::Midi;
use serde_json::{json, Value};
use std::time::Duration;

// ---- the descriptors, read by osd ----------------------------------------------------------

const SCHEMA: &str = "https://w3id.org/osd/0.6/device.schema.json";

fn read(doc: Value) -> osd::Device {
    match osd::read(&doc.to_string(), osd::Kind::Device).unwrap_or_else(|e| panic!("{e}\n{doc}")) {
        osd::Descriptor::Device(d) => *d,
        _ => unreachable!(),
    }
}

fn source_doc(vendor: &str, name: &str, with_sources: bool) -> Value {
    let mut doc = json!({
        "$schema": SCHEMA,
        "device": {"name": name, "vendor": vendor, "osc_prefix": "/src"},
        "cc_params": {"entries": [{"osc": "/cutoff", "cc": 74}, {"osc": "/resonance", "cc": 71}, {"osc": "/attack", "cc": 73},
            {"osc": "/decay", "cc": 75}, {"osc": "/release", "cc": 72}, {"osc": "/volume", "cc": 7}]},
    });
    if with_sources {
        doc["_sources"] = json!([{"type": "community-re", "url": "https://example.org/box", "author": "A maker", "license": "CC-BY-4.0"}]);
    }
    doc
}

fn source() -> osd::Device {
    read(source_doc("Acme", "Box", true))
}

const HEAD: [u8; 7] = [0xF0, 0x00, 0x21, 0x7B, 0x01, 0x04, 0x00];
const REPLY_HEAD: [u8; 6] = [0xF0, 0x00, 0x21, 0x7B, 0x01, 0x05];

fn with(bytes: &[u8], text: &str, tail: &[u8]) -> Vec<u8> {
    [bytes, text.as_bytes(), tail].concat()
}

/// A device that receives its definition file by a file written in SysEx: the steps of the OSD
/// conformance suite for a Deluge, with a small chunk so that a file takes several writes.
fn deluge() -> osd::Device {
    let head: Vec<Value> = HEAD.iter().map(|b| json!(b)).collect();
    let reply: Vec<Value> = REPLY_HEAD.iter().map(|b| json!(b)).chain([json!("any")]).collect();
    let cat = |a: &[Value], b: Vec<Value>| -> Vec<Value> { a.iter().cloned().chain(b).collect() };
    read(json!({
        "$schema": SCHEMA,
        "device": {"name": "Deluge", "vendor": "Synthstrom Audible", "osc_prefix": "/deluge"},
        "resources": [{
            "resource": "X-DeviceDefinition", "canSet": "full", "mediaTypes": ["application/xml"], "export": "deluge",
            "via": {"file_write": {
                "path": ["MIDI_DEVICES", "DEFINITION", {"variable": "vendor"}, {"variable": "device", "suffix": ".xml"}],
                "carried_by": "sysex",
                "steps": [
                    {"step": "open",
                     "send": cat(&head, vec![json!({"text": "{\"open\":{\"path\":\""}), json!({"file": "path"}), json!({"text": "\",\"write\":1}}"}), json!(247)]),
                     "reply": cat(&reply, vec![json!({"capture_json": "fid"})]), "reply_timeout_ms": 1000},
                    {"step": "write", "chunk_bytes": 64, "coding": "msb_first_7in8",
                     "send": cat(&head, vec![json!({"text": "{\"write\":{\"fid\":"}), json!({"captured": "fid", "as": "decimal"}),
                        json!({"text": ",\"addr\":"}), json!({"file": "offset", "as": "decimal"}),
                        json!({"text": ",\"size\":"}), json!({"file": "chunk_size", "as": "decimal"}),
                        json!({"text": "}}"}), json!(0), json!({"file": "chunk"}), json!(247)]),
                     "reply": reply, "reply_timeout_ms": 1000},
                    {"step": "close",
                     "send": cat(&head, vec![json!({"text": "{\"close\":{\"fid\":"}), json!({"captured": "fid", "as": "decimal"}), json!({"text": "}}"}), json!(247)]),
                     "reply": reply, "reply_timeout_ms": 1000}
                ]}}
        }],
        "_sources": [{"type": "community-re", "url": "https://example.org/target", "license": "GPL-3.0-only"}]
    }))
}

/// A target that is not a Deluge: another format, another path, another messages and another
/// coding, another pause.
fn rack() -> osd::Device {
    read(json!({
        "$schema": SCHEMA,
        "device": {"name": "Rack One", "vendor": "Zed", "osc_prefix": "/rack"},
        "resources": [{
            "resource": "X-Names", "canSet": "full", "mediaTypes": ["application/xml"], "export": "midnam",
            "via": {"file_write": {
                "path": ["Names", {"variable": "device", "suffix": ".midnam"}],
                "carried_by": "sysex",
                "steps": [
                    {"step": "open", "send": [240, 0, 32, 9, 2, {"text": "PUT "}, {"file": "path"}, {"text": " "}, {"file": "size", "as": "decimal"}, 247],
                     "reply": [240, 0, 32, 9, 3, {"capture_json": "fid"}], "reply_timeout_ms": 500},
                    {"step": "write", "chunk_bytes": 200, "coding": "mcoded7", "delay_ms": 5,
                     "send": [240, 0, 32, 9, 4, {"text": "DATA "}, {"captured": "fid", "as": "decimal"}, {"text": " "}, {"file": "offset", "as": "decimal"}, 0, {"file": "chunk"}, 247],
                     "reply": [240, 0, 32, 9, 3], "reply_timeout_ms": 500},
                    {"step": "close", "send": [240, 0, 32, 9, 5, {"captured": "fid", "as": "decimal"}, 247]}
                ]}}
        }],
        "_sources": [{"type": "community-re", "url": "https://example.org/rack", "license": "CC0-1.0"}]
    }))
}

fn silent_target() -> osd::Device {
    read(json!({"$schema": SCHEMA, "device": {"name": "Mute", "vendor": "Zed", "osc_prefix": "/mute"}}))
}

// ---- the ports: a recorded exchange --------------------------------------------------------

struct Bench {
    ports: Vec<String>,
    waiting: Vec<(String, Vec<u8>)>,
    sent: Vec<(String, Vec<u8>)>,
    attempts: usize,
    fail_at: Option<usize>,
    deaf: bool,
    answer: fn(&[u8]) -> Option<Vec<u8>>,
}

impl Bench {
    fn new(ports: &[&str], answer: fn(&[u8]) -> Option<Vec<u8>>) -> Self {
        Bench { ports: ports.iter().map(|p| p.to_string()).collect(), waiting: vec![], sent: vec![], attempts: 0, fail_at: None, deaf: false, answer }
    }
    fn messages(&self) -> Vec<Vec<u8>> {
        self.sent.iter().map(|(_, m)| m.clone()).collect()
    }
}

impl Midi for Bench {
    fn outputs(&self) -> Vec<String> {
        self.ports.clone()
    }
    fn send(&mut self, port: &str, messages: &[Vec<u8>], _why: &str) -> Result<(), String> {
        for m in messages {
            self.attempts += 1;
            if self.fail_at == Some(self.attempts) {
                return Err("port closed".into());
            }
            self.sent.push((port.to_string(), m.clone()));
            if !self.deaf {
                if let Some(r) = (self.answer)(m) {
                    self.waiting.push((port.to_string(), r));
                }
            }
        }
        Ok(())
    }
    fn received(&mut self, _wait: Duration) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut self.waiting)
    }
}

#[derive(Default)]
struct FakeClock {
    asked: Vec<Duration>,
}
impl Clock for FakeClock {
    fn sleep(&mut self, d: Duration) {
        self.asked.push(d);
    }
}

fn has(m: &[u8], text: &str) -> bool {
    m.windows(text.len()).any(|w| w == text.as_bytes())
}

/// The Deluge's answers: the file id after an open, an acknowledgement after the rest.
fn deluge_answers(m: &[u8]) -> Option<Vec<u8>> {
    if !m.starts_with(&HEAD) {
        return None;
    }
    if has(m, "\"open\"") {
        Some(with(&[REPLY_HEAD.as_slice(), &[0]].concat(), "{\"fid\":7}", &[0xF7]))
    } else {
        Some(with(&[REPLY_HEAD.as_slice(), &[0]].concat(), "{\"err\":0}", &[0xF7]))
    }
}

fn rack_answers(m: &[u8]) -> Option<Vec<u8>> {
    if m.starts_with(&[240, 0, 32, 9, 2]) {
        Some(with(&[240, 0, 32, 9, 3], "{\"fid\":5}", &[0xF7]))
    } else if m.starts_with(&[240, 0, 32, 9, 4]) {
        Some(with(&[240, 0, 32, 9, 3], "{}", &[0xF7]))
    } else {
        None
    }
}

/// The file the export of `source` in `format` gives, as osd-formats writes it.
fn exported(source: &osd::Device, format: osd_formats::Format) -> osd_formats::Translated<String> {
    let resolved = osd::Resolved { device: source.clone(), files: Vec::new(), capabilities: Vec::new(), inherited: Vec::new() };
    osd_formats::export(&resolved, format).unwrap_or_else(|e| panic!("{e}"))
}

/// The bytes of a chunk in 7 bits, `msb_first_7in8` (a byte of the high bits, the first byte's at
/// bit 6, then the 7 low bits of each byte), for every group of up to 7 bytes.
fn pack(chunk: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for group in chunk.chunks(7) {
        let high = group.iter().enumerate().fold(0u8, |acc, (i, b)| acc | ((b >> 7) << (6 - i)));
        out.push(high);
        out.extend(group.iter().map(|b| b & 0x7F));
    }
    out
}

fn run(src: &osd::Device, target: &osd::Device, confirm: bool, bench: &mut Bench) -> (Result<osc_bridge::install::Installed, String>, Vec<Duration>) {
    let mut clock = FakeClock::default();
    let r = install(src, target, confirm, bench, &mut clock);
    (r, clock.asked)
}

// ---- the gesture on a Deluge described by its descriptor ------------------------------------

#[test]
fn a_deluge_described_by_its_descriptor_receives_the_definition_file_as_a_file_written_by_sysex() {
    let (src, target) = (source(), deluge());
    let file = exported(&src, osd_formats::Format::Deluge).value.into_bytes();
    assert!(file.is_ascii() && file.len() > 128, "a file of several chunks: {}", file.len());
    let mut bench = Bench::new(&["Deluge MIDI 1"], deluge_answers);
    let (r, _) = run(&src, &target, true, &mut bench);
    let done = r.unwrap_or_else(|e| panic!("{e}"));

    let mut expected = vec![with(&HEAD, "{\"open\":{\"path\":\"MIDI_DEVICES/DEFINITION/Acme/Box.xml\",\"write\":1}}", &[0xF7])];
    for (n, chunk) in file.chunks(64).enumerate() {
        let text = format!("{{\"write\":{{\"fid\":7,\"addr\":{},\"size\":{}}}}}", n * 64, chunk.len());
        expected.push([with(&HEAD, &text, &[0]), pack(chunk), vec![0xF7]].concat());
    }
    expected.push(with(&HEAD, "{\"close\":{\"fid\":7}}", &[0xF7]));
    assert_eq!(bench.messages(), expected);
    assert!(bench.sent.iter().all(|(p, _)| p == "Deluge MIDI 1"), "the port of the target");
    assert!(done.written);
    assert_eq!(done.messages, expected.len());
}

#[test]
fn the_path_is_filled_from_the_vendor_and_the_name_of_the_device_placed() {
    let other = read(source_doc("Maker Inc", "Model 2", true));
    let mut bench = Bench::new(&["Deluge MIDI 1"], deluge_answers);
    let (r, _) = run(&other, &deluge(), true, &mut bench);
    let done = r.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(done.path, "MIDI_DEVICES/DEFINITION/Maker Inc/Model 2.xml");
    assert!(has(&bench.messages()[0], "MIDI_DEVICES/DEFINITION/Maker Inc/Model 2.xml"));
}

// ---- no consent, nothing sent ---------------------------------------------------------------

#[test]
fn without_consent_nothing_is_sent_and_the_preview_gives_format_way_path_size_and_losses() {
    let (src, target) = (source(), deluge());
    let out = exported(&src, osd_formats::Format::Deluge);
    let mut bench = Bench::new(&["Deluge MIDI 1"], deluge_answers);
    let (r, asked) = run(&src, &target, false, &mut bench);
    let preview = r.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(bench.attempts, 0, "no message, the device is not even read");
    assert!(asked.is_empty());
    assert!(!preview.written);
    assert_eq!(preview.messages, 0);
    assert_eq!(preview.format, "deluge");
    assert_eq!(preview.way, "sysex");
    assert_eq!(preview.path, "MIDI_DEVICES/DEFINITION/Acme/Box.xml");
    assert_eq!(preview.size, out.value.len());
    assert_eq!(preview.lost, out.lost);
}

#[test]
fn a_write_on_the_target_is_always_taken_for_one_that_may_overwrite() {
    // the target says canSet full and the gesture never reads the device first: no path
    // sends without the consent
    for target in [deluge(), rack()] {
        let mut bench = Bench::new(&["Deluge MIDI 1", "Rack One"], deluge_answers);
        let (r, _) = run(&source(), &target, false, &mut bench);
        assert!(r.is_ok());
        assert_eq!(bench.attempts, 0);
    }
}

// ---- the gesture on a target that is not a Deluge ------------------------------------------

#[test]
fn a_target_that_is_not_a_deluge_receives_the_format_by_the_way_its_descriptor_says() {
    let (src, target) = (source(), rack());
    let file = exported(&src, osd_formats::Format::Midnam).value.into_bytes();
    assert!(file.is_ascii(), "the file stays in 7 bits");
    let mut bench = Bench::new(&["Rack One"], rack_answers);
    let (r, asked) = run(&src, &target, true, &mut bench);
    let done = r.unwrap_or_else(|e| panic!("{e}"));

    let mut expected = vec![[
        b"\xF0\x00\x20\x09\x02PUT Names/Box.midnam ".as_slice(),
        file.len().to_string().as_bytes(),
        &[0xF7],
    ]
    .concat()];
    for (n, chunk) in file.chunks(200).enumerate() {
        expected.push([with(&[240, 0, 32, 9, 4], &format!("DATA 5 {}", n * 200), &[0]), pack(chunk), vec![0xF7]].concat());
    }
    expected.push(vec![240, 0, 32, 9, 5, b'5', 0xF7]);
    assert_eq!(bench.messages(), expected);
    assert_eq!(done.format, "midnam");
    assert_eq!(done.path, "Names/Box.midnam");
    assert!(asked.contains(&Duration::from_millis(5)), "the pause the descriptor declares: {asked:?}");
}

// ---- refusals, before any message -----------------------------------------------------------

#[test]
fn a_target_that_declares_nothing_it_receives_is_refused_with_a_message_that_says_so() {
    for confirm in [false, true] {
        let mut bench = Bench::new(&["Mute"], deluge_answers);
        let (r, _) = run(&source(), &silent_target(), confirm, &mut bench);
        let e = r.unwrap_err();
        assert!(e.contains("Mute") && e.contains("receive"), "{e}");
        assert_eq!(bench.attempts, 0);
    }
}

#[test]
fn a_target_that_is_not_plugged_is_refused_naming_it_before_any_message() {
    let mut bench = Bench::new(&["Midi Through", "Other Synth"], deluge_answers);
    let (r, _) = run(&source(), &deluge(), true, &mut bench);
    let e = r.unwrap_err();
    assert!(e.contains("Deluge"), "{e}");
    assert_eq!(bench.attempts, 0);
}

#[test]
fn a_source_whose_export_cannot_be_built_is_refused_with_the_reason_and_nothing_is_sent() {
    let uncredited = read(source_doc("Acme", "Box", false));
    let mut bench = Bench::new(&["Deluge MIDI 1"], deluge_answers);
    let (r, _) = run(&uncredited, &deluge(), true, &mut bench);
    let e = r.unwrap_err();
    assert!(e.contains("sources"), "{e}");
    assert_eq!(bench.attempts, 0);
}

// ---- a failure on the way -------------------------------------------------------------------

#[test]
fn a_send_that_fails_on_the_way_says_the_step_reached_and_the_messages_already_sent_without_retry() {
    let mut bench = Bench::new(&["Deluge MIDI 1"], deluge_answers);
    bench.fail_at = Some(3); // the open and the first write went, the second write fails
    let (r, _) = run(&source(), &deluge(), true, &mut bench);
    let e = r.unwrap_err();
    assert!(e.contains("write"), "the step: {e}");
    assert!(e.contains('2'), "the two messages sent: {e}");
    assert!(e.contains("port closed"), "{e}");
    assert_eq!(bench.sent.len(), 2);
    assert_eq!(bench.attempts, 3, "no retry, no rollback message");
}

#[test]
fn a_device_that_does_not_answer_the_open_step_stops_the_gesture_at_that_step() {
    let mut bench = Bench::new(&["Deluge MIDI 1"], deluge_answers);
    bench.deaf = true;
    let (r, _) = run(&source(), &deluge(), true, &mut bench);
    let e = r.unwrap_err();
    assert!(e.contains("open"), "{e}");
    assert_eq!(bench.sent.len(), 1, "only the open went");
}

// ---- the tool and the command ---------------------------------------------------------------

fn catalogue() -> tempfile::TempDir {
    let root = temp("install-export");
    files(
        root.path(),
        &[
            ("acme/box.json", &source_doc("Acme", "Box", true).to_string()),
            ("synthstrom/deluge.json", &deluge_doc().to_string()),
        ],
    );
    root
}

fn deluge_doc() -> Value {
    json!({
        "$schema": SCHEMA,
        "device": {"name": "Deluge", "vendor": "Synthstrom Audible", "osc_prefix": "/deluge"},
        "resources": [{
            "resource": "X-DeviceDefinition", "canSet": "full", "mediaTypes": ["application/xml"], "export": "deluge",
            "via": {"file_write": {
                "path": ["MIDI_DEVICES", "DEFINITION", {"variable": "vendor"}, {"variable": "device", "suffix": ".xml"}],
                "carried_by": "sysex",
                "steps": [
                    {"step": "open", "send": [240, 0, 33, 123, 1, 4, 0, {"text": "{\"open\":{\"path\":\""}, {"file": "path"}, {"text": "\",\"write\":1}}"}, 247],
                     "reply": [240, 0, 33, 123, 1, 5, "any", {"capture_json": "fid"}], "reply_timeout_ms": 1000},
                    {"step": "write", "chunk_bytes": 1024, "coding": "msb_first_7in8",
                     "send": [240, 0, 33, 123, 1, 4, 0, {"text": "{\"write\":{\"fid\":"}, {"captured": "fid", "as": "decimal"}, {"text": "}}"}, 0, {"file": "chunk"}, 247],
                     "reply": [240, 0, 33, 123, 1, 5, "any"], "reply_timeout_ms": 1000},
                    {"step": "close", "send": [240, 0, 33, 123, 1, 4, 0, {"text": "{\"close\":{\"fid\":"}, {"captured": "fid", "as": "decimal"}, {"text": "}}"}, 247],
                     "reply": [240, 0, 33, 123, 1, 5, "any"], "reply_timeout_ms": 1000}
                ]}}
        }],
        "_sources": [{"type": "community-re", "url": "https://example.org/target", "license": "GPL-3.0-only"}]
    })
}

fn call(root: &tempfile::TempDir, args: Value) -> Value {
    call_on(root, args, &crate::common::FakeHardware::none().opener())
}

fn call_on(
    root: &tempfile::TempDir,
    args: Value,
    open: &dyn Fn() -> anyhow::Result<Box<dyn Midi>>,
) -> Value {
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(root.path().to_path_buf()),
        default_target: "127.0.0.1:9".parse().unwrap(),
        online: false,
        private: vec![],
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
    };
    let input = format!("{}\n", tool_call(1, "install_export", args));
    let mut out = Vec::new();
    osc_bridge::mcp::serve_with_midi(opts, "http://127.0.0.1:1", None, input.as_bytes(), &mut out, open).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    reply["result"].clone()
}

fn text(r: &Value) -> String {
    r["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")
}

#[test]
fn install_export_refuses_a_target_that_is_not_plugged_naming_it_before_any_send() {
    let root = catalogue();
    let r = call(&root, json!({"device": "acme/box", "on": "synthstrom/deluge", "confirm_overwrite": true}));
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text(&r).contains("Deluge"), "{}", text(&r));
}

#[test]
fn install_export_refuses_a_target_the_catalogue_does_not_know() {
    let root = catalogue();
    let r = call(&root, json!({"device": "acme/box", "on": "nobody/nothing", "confirm_overwrite": true}));
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text(&r).contains("nobody/nothing"), "{}", text(&r));
}

#[test]
fn install_export_refuses_a_call_without_a_target_naming_the_argument() {
    let root = catalogue();
    let r = call(&root, json!({"device": "acme/box"}));
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text(&r).contains("on"), "{}", text(&r));
}

#[test]
fn the_export_command_function_refuses_a_target_that_is_not_plugged_naming_it_whatever_else_is_plugged() {
    let root = catalogue();
    let src = root.path().join("acme/box.json");
    let target = root.path().join("synthstrom/deluge.json");
    for hardware in [FakeHardware::none(), FakeHardware::with_ports(&["Some Keyboard", "Midi Through"])] {
        let e = osc_bridge::install::install_command(&src, &target, true, &hardware.opener()).unwrap_err().to_string();
        assert!(e.contains("Deluge"), "{e}");
        assert!(hardware.sent.lock().unwrap().is_empty(), "nothing is sent to a port that is not the target's");
    }
}

#[test]
fn the_export_command_function_names_the_midi_system_and_the_cause_when_it_cannot_be_opened() {
    let root = catalogue();
    let src = root.path().join("acme/box.json");
    let target = root.path().join("synthstrom/deluge.json");
    let e = osc_bridge::install::install_command(&src, &target, true, &crate::common::failing_opener("no sequencer"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("MIDI"), "{e}");
    assert!(e.contains("no sequencer"), "{e}");
}

#[test]
fn install_export_names_the_midi_system_and_the_cause_when_it_cannot_be_opened() {
    let root = catalogue();
    let r = call_on(
        &root,
        json!({"device": "acme/box", "on": "synthstrom/deluge", "confirm_overwrite": true}),
        &crate::common::failing_opener("no sequencer"),
    );
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text(&r).contains("MIDI") && text(&r).contains("no sequencer"), "{}", text(&r));
}

#[test]
fn install_export_takes_its_midi_from_the_opener_it_is_given() {
    let root = catalogue();
    let hardware = FakeHardware::with_ports(&["Some Keyboard"]);
    let r = call_on(&root, json!({"device": "acme/box", "on": "synthstrom/deluge", "confirm_overwrite": true}), &hardware.opener());
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text(&r).contains("Deluge"), "{}", text(&r));
    assert!(hardware.sent.lock().unwrap().is_empty());
}

#[test]
fn the_export_command_without_a_target_still_writes_the_document() {
    let root = catalogue();
    let src = root.path().join("acme/box.json");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_osc-bridge"))
        .args(["export", "--format", "deluge", "--device"])
        .arg(&src)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8(out.stdout).unwrap().contains("<midiDevice>"));
}
