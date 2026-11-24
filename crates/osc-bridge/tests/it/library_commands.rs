//! The library carries the bodies of the six commands that name a device, with plain values and the
//! writers the caller gives: a client outside the binary calls them without a process.

use osc_bridge::commands::{self, RunOptions};
use std::path::Path;

const MIDI_DRIVER: &str = "tests/fixtures/devices/arturia/microfreak.json";
const FIRMWARE: &str = "tests/fixtures/devices/arturia/microfreak.vendor.fw-microfreak-tools-0.1.0.json";
const OSC_ONLY: &str = "tests/fixtures/devices/bitwig/bitwig-studio.third-party-osc.fw-5.json";

fn options(device: &str) -> RunOptions {
    RunOptions {
        device: device.into(),
        out_port: None,
        in_port: None,
        bind: "127.0.0.1:0".into(),
        osc_clients: vec![],
        ws_bind: None,
        ws_allow_origins: vec![],
        oscquery_bind: None,
    }
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap()
}

#[test]
fn inspect_writes_the_summary_on_the_given_output() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    commands::inspect(Path::new(MIDI_DRIVER), &mut out, &mut err).unwrap();
    let out = text(out);
    assert!(out.contains("Device:"), "{out}");
    assert!(out.contains("Commands:"), "{out}");
    assert!(out.contains("Replies:"), "{out}");
}

#[test]
fn inspect_writes_the_checksum_alerts_on_the_error_writer_not_on_the_output() {
    let device = osc_bridge::device::load(FIRMWARE).unwrap();
    let alerts = commands::checksum_warnings(&device);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    commands::inspect(Path::new(FIRMWARE), &mut out, &mut err).unwrap();
    let (out, err) = (text(out), text(err));
    for line in &alerts {
        assert!(err.contains(line.as_str()), "{line} missing from {err}");
        assert!(!out.contains(line.as_str()), "{line} leaked on the output");
    }
}

#[test]
fn checksum_warnings_is_published_for_a_loaded_device() {
    let device = osc_bridge::device::load(MIDI_DRIVER).unwrap();
    let lines = commands::checksum_warnings(&device);
    assert!(lines.iter().all(|l| l.starts_with("  WARN: ")), "{lines:?}");
}

#[test]
fn inspect_of_a_missing_driver_fails_naming_the_path() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let e = commands::inspect(Path::new("/no/such/driver.json"), &mut out, &mut err).unwrap_err();
    assert!(format!("{e:#}").contains("/no/such/driver.json"), "{e:#}");
}

#[test]
fn export_writes_the_document_on_the_output_and_the_losses_on_the_error_writer() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    commands::export(Path::new(FIRMWARE), Some("midnam"), None, &mut out, &mut err).unwrap();
    let (out, err) = (text(out), text(err));
    assert!(out.contains("<MIDINameDocument>"), "{out}");
    assert!(err.contains("not carried: "), "{err}");
    assert!(err.contains("/_sources/0/firmware"), "{err}");
    assert!(!out.contains("not carried"), "the document holds no list of losses");
}

#[test]
fn export_without_format_nor_target_fails_saying_the_format_is_required() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let e = commands::export(Path::new(FIRMWARE), None, None, &mut out, &mut err).unwrap_err();
    assert!(format!("{e:#}").contains("--format is required without --on"), "{e:#}");
    assert!(out.is_empty());
}

#[test]
fn export_of_an_osc_only_device_to_midnam_fails_with_its_message() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let e = commands::export(Path::new(OSC_ONLY), Some("midnam"), None, &mut out, &mut err).unwrap_err();
    assert!(format!("{e:#}").contains("speaks OSC, not MIDI"), "{e:#}");
    assert!(out.is_empty());
}

#[test]
fn export_of_a_missing_driver_fails_naming_the_path() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let e = commands::export(Path::new("/no/such/driver.json"), Some("midnam"), None, &mut out, &mut err).unwrap_err();
    assert!(format!("{e:#}").contains("/no/such/driver.json"), "{e:#}");
}

#[test]
fn export_to_a_target_without_yes_sends_nothing_and_says_so_on_the_output() {
    // The target declares a way to receive a file written by SysEx, so the gesture gets past the
    // declaration; the machine's MIDI hardware decides the rest, and no message goes without `yes`.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("zed/rack.json");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    let doc = serde_json::json!({
        "$schema": "https://w3id.org/osd/0.6/device.schema.json",
        "device": {"name": "Rack One", "vendor": "Zed", "osc_prefix": "/rack"},
        "resources": [{
            "resource": "X-Names", "canSet": "full", "mediaTypes": ["application/xml"], "export": "midnam",
            "via": {"file_write": {
                "path": ["Names", {"variable": "device", "suffix": ".midnam"}],
                "carried_by": "sysex",
                "steps": [
                    {"step": "open", "send": [240, 0, 32, 9, 2, {"text": "PUT "}, {"file": "path"}, 247],
                     "reply": [240, 0, 32, 9, 3, {"capture_json": "fid"}], "reply_timeout_ms": 500},
                    {"step": "write", "chunk_bytes": 200, "coding": "mcoded7",
                     "send": [240, 0, 32, 9, 4, {"captured": "fid", "as": "decimal"}, 0, {"file": "chunk"}, 247],
                     "reply": [240, 0, 32, 9, 3], "reply_timeout_ms": 500},
                    {"step": "close", "send": [240, 0, 32, 9, 5, {"captured": "fid", "as": "decimal"}, 247]}
                ]}}
        }],
        "_sources": [{"type": "community-re", "url": "https://example.org/rack", "license": "CC0-1.0"}]
    });
    std::fs::write(&target, doc.to_string()).unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = commands::export(Path::new(MIDI_DRIVER), None, Some((&target, false)), &mut out, &mut err);
    match result {
        Ok(()) => {
            let out = text(out);
            assert!(out.starts_with("nothing sent: it would write Names/"), "{out}");
            assert!(out.contains("add --yes to send"), "{out}");
        }
        Err(e) => {
            // Only the step after the declared way may refuse here (no MIDI system, or the target
            // not plugged): the target was read and its way accepted, and the output stays empty.
            let e = format!("{e:#}");
            assert!(e.contains("cannot be reached (MIDI system)") || e.contains("is not plugged"), "{e}");
            assert!(!e.contains("declares no"), "{e}");
            assert!(out.is_empty(), "{e}");
        }
    }
}

#[test]
fn run_of_a_midi_driver_without_out_port_fails_asking_for_it() {
    let mut err = Vec::new();
    let e = commands::run(options(MIDI_DRIVER), &mut err).unwrap_err();
    assert!(format!("{e:#}").contains("--out-port is required"), "{e:#}");
}

#[test]
fn run_with_an_invalid_osc_client_fails_naming_the_option() {
    let mut err = Vec::new();
    let mut opts = options(OSC_ONLY);
    opts.osc_clients = vec!["not an address".into()];
    let e = commands::run(opts, &mut err).unwrap_err();
    assert!(format!("{e:#}").contains("--osc-client"), "{e:#}");
}

#[test]
fn run_notes_on_the_error_writer_that_in_port_is_ignored_for_an_osc_driver() {
    let mut err = Vec::new();
    let mut opts = options(OSC_ONLY);
    opts.in_port = Some(1);
    opts.osc_clients = vec!["not an address".into()];
    let _ = commands::run(opts, &mut err);
    assert_eq!(text(err), "note: --in-port is ignored for OSC-transport drivers\n");
}

#[test]
fn run_of_a_missing_driver_fails_naming_the_path() {
    let mut err = Vec::new();
    let e = commands::run(options("/no/such/driver.json"), &mut err).unwrap_err();
    assert!(format!("{e:#}").contains("/no/such/driver.json"), "{e:#}");
}

#[test]
fn backup_of_a_missing_driver_fails_naming_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut err = Vec::new();
    let e = commands::backup(Path::new("/no/such/driver.json"), "0", &dir.path().join("a.syx"), None, 1, 1, &mut err).unwrap_err();
    assert!(format!("{e:#}").contains("/no/such/driver.json"), "{e:#}");
    assert!(err.is_empty(), "nothing is said before the driver is read");
}

#[test]
fn restore_of_a_missing_driver_fails_naming_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut err = Vec::new();
    let e = commands::restore(Path::new("/no/such/driver.json"), "0", None, &dir.path().join("a.syx"), &mut err).unwrap_err();
    assert!(format!("{e:#}").contains("/no/such/driver.json"), "{e:#}");
}

#[test]
fn orchestrate_of_a_missing_configuration_fails_naming_the_path() {
    let e = commands::orchestrate(Path::new("/no/such/bridge.toml")).unwrap_err();
    assert!(format!("{e:#}").contains("/no/such/bridge.toml"), "{e:#}");
}
