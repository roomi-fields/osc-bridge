//! `restore_presets` and `backup_presets` always name a device: the driver says which banks
//! can be written, the pauses, the reply bound and how a received sound is recognised. The
//! logic is `programs::restore` and `programs::backup_filter` (small tests, an injected
//! clock and sender); the two tools refuse before touching any port (small tests through
//! the MCP session); the CLI words need `--device` too.

use crate::common::{files, temp};
use osc_bridge::programs::{backup_filter, restore, Clock};
use serde_json::{json, Value};
use std::time::Duration;

fn driver(programs: &str, extra: &str) -> String {
    format!(r#"{{"$schema":"https://w3id.org/osd/0.6/device.schema.json",
        "device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box"}},
        "midi_out":{{"default_channel":2}},"programs":{programs}{extra}}}"#)
}

fn dev06(programs: &str, extra: &str) -> osd::Device {
    match osd::read(&driver(programs, extra), osd::Kind::Device).unwrap() {
        osd::Descriptor::Device(d) => *d,
        _ => unreachable!(),
    }
}

#[derive(Default)]
struct FakeClock { asked: Vec<Duration> }
impl Clock for FakeClock {
    fn sleep(&mut self, d: Duration) { self.asked.push(d); }
}

const PROGRAMS: &str = r#"{"delay_ms":30,"before":[[240,125,1,247]],
    "banks":[{"name":"Factory","msb":1,"size":8,"writable":false},
             {"name":"User","msb":2,"lsb":3,"size":8,"writable":true}]}"#;
const DX7: &str = r#","sound_transfers":[
    {"id":"voice","carries":"sound","match_frame":[240,67,"any",0,1,27,"any","any","any",247],
     "elements":{"data":{"from":6,"length":3},"channel":{"from":2,"length":1}}},
    {"id":"patch","carries":"sound","match_frame":[240,65,"any",0,6,"any","any",247],
     "elements":{"data":{"from":5,"length":2},"device_number":{"from":2,"length":1}}}]"#;

const VOICE: [u8; 10] = [0xF0, 0x43, 0x09, 0, 1, 0x1B, 10, 20, 30, 0xF7];
const PATCH: [u8; 8] = [0xF0, 0x41, 0x63, 0, 6, 5, 6, 0xF7];

/// What `restore` sends, and the waits it asked.
fn run(d: &osd::Device, msgs: &[Vec<u8>], target: Option<(&str, i64)>, channel: u8, number: Option<u8>)
    -> (Result<usize, String>, Vec<Vec<u8>>, Vec<Duration>) {
    let mut clock = FakeClock::default();
    let mut sent: Vec<Vec<u8>> = Vec::new();
    let r = restore(d, msgs, target, channel, number, &mut clock, &mut |m: &[u8]| { sent.push(m.to_vec()); Ok(()) });
    (r, sent, clock.asked)
}

#[test]
fn a_restore_into_a_bank_that_cannot_be_written_sends_nothing_and_names_the_bank() {
    let d = dev06(PROGRAMS, DX7);
    let (r, sent, asked) = run(&d, &[VOICE.to_vec()], Some(("Factory", 1)), 5, None);
    let e = r.unwrap_err();
    assert!(e.contains("Factory") && e.contains("writable"), "{e}");
    assert!(sent.is_empty(), "no message, the one before and the bank selection included");
    assert!(asked.is_empty());
}

#[test]
fn a_restore_selects_the_writable_bank_then_sends_the_file_with_the_declared_pause_between_messages() {
    let d = dev06(PROGRAMS, DX7);
    let (r, sent, asked) = run(&d, &[VOICE.to_vec(), PATCH.to_vec()], Some(("User", 2)), 5, Some(1));
    assert_eq!(r, Ok(sent.len()));
    let selection = osc_bridge::programs::store(&d, "User", 2, Some(5)).unwrap();
    assert_eq!(&sent[..selection.len()], selection.as_slice(), "the bank selection comes first");
    assert_eq!(sent.len(), selection.len() + 2);
    assert_eq!(asked, vec![Duration::from_millis(30); sent.len() - 1], "between messages, not after the last");
}

#[test]
fn a_restore_writes_the_configured_channel_and_device_number_in_each_sound() {
    let d = dev06(PROGRAMS, DX7);
    let (r, sent, _) = run(&d, &[VOICE.to_vec(), PATCH.to_vec()], None, 5, Some(16));
    assert!(r.is_ok());
    assert_eq!(sent, vec![
        vec![0xF0, 0x43, 5, 0, 1, 0x1B, 10, 20, 30, 0xF7],
        vec![0xF0, 0x41, 16, 0, 6, 5, 6, 0xF7],
    ]);
}

#[test]
fn a_restore_without_a_place_sends_only_the_file_and_a_driver_with_no_pause_waits_nothing() {
    let d = dev06(r#"{"banks":[{"name":"User","size":8}]}"#, DX7);
    let (r, sent, asked) = run(&d, &[VOICE.to_vec(), VOICE.to_vec(), VOICE.to_vec()], None, 0, None);
    assert_eq!(r, Ok(3));
    assert_eq!(sent.len(), 3);
    assert!(asked.is_empty());
}

#[test]
fn a_restore_stops_at_the_first_message_the_port_refuses() {
    let d = dev06(PROGRAMS, DX7);
    let mut clock = FakeClock::default();
    let mut n = 0;
    let r = restore(&d, &[VOICE.to_vec(), VOICE.to_vec()], None, 0, None, &mut clock, &mut |_: &[u8]| {
        n += 1;
        if n == 2 { Err("port closed".to_string()) } else { Ok(()) }
    });
    assert!(r.unwrap_err().contains("port closed"));
    assert_eq!(n, 2);
}

#[test]
fn a_backup_takes_a_sound_received_on_another_channel_or_from_another_device_number() {
    let d = dev06(PROGRAMS, DX7);
    let other_channel = vec![0xF0, 0x43, 0x0F, 0, 1, 0x1B, 1, 2, 3, 0xF7];
    let other_device = vec![0xF0, 0x41, 99, 0, 6, 5, 6, 0xF7];
    let stranger = vec![0xF0, 0x7D, 1, 0xF7];
    let kept = backup_filter(&d, &[other_channel.clone(), stranger, other_device.clone()]);
    assert_eq!(kept, vec![other_channel, other_device]);
}

#[test]
fn a_backup_from_a_driver_declaring_no_sound_transfer_keeps_every_message() {
    let d = dev06(PROGRAMS, "");
    let all = vec![VOICE.to_vec(), vec![0xF0, 0x7D, 1, 0xF7]];
    assert_eq!(backup_filter(&d, &all), all);
}

// ---- the two tools -------------------------------------------------------------

fn call(root: &tempfile::TempDir, tool: &str, args: Value) -> Value {
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(root.path().to_path_buf()),
        default_target: "127.0.0.1:9".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
    };
    let input = format!("{}\n", crate::common::tool_call(1, tool, args));
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, input.as_bytes(), &mut out).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    reply["result"].clone()
}

fn text(r: &Value) -> String {
    r["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")
}

fn catalogue() -> tempfile::TempDir {
    let root = temp("presets-with-device");
    files(root.path(), &[("acme/box.json", &driver(PROGRAMS, DX7))]);
    std::fs::write(root.path().join("sound.syx"), VOICE).unwrap();
    root
}

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

#[test]
fn restore_presets_refuses_a_call_without_a_device_naming_the_argument() {
    let root = catalogue();
    let file = root.path().join("sound.syx").display().to_string();
    let r = call(&root, "restore_presets", json!({"out_port": "no-such-port", "file": file, "confirm_overwrite": true}));
    assert!(is_error(&r), "{r}");
    assert!(text(&r).contains("device"), "{}", text(&r));
}

#[test]
fn backup_presets_refuses_a_call_without_a_device_naming_the_argument() {
    let root = catalogue();
    let file = root.path().join("out.syx").display().to_string();
    let r = call(&root, "backup_presets", json!({"in_port": "no-such-port", "file": file}));
    assert!(is_error(&r), "{r}");
    assert!(text(&r).contains("device"), "{}", text(&r));
    assert!(!root.path().join("out.syx").exists());
}

#[test]
fn both_tools_refuse_a_device_the_catalogue_does_not_know_and_invite_to_describe_it() {
    let root = catalogue();
    let file = root.path().join("sound.syx").display().to_string();
    for (tool, args) in [
        ("restore_presets", json!({"device": "acme/ghost", "out_port": "x", "file": file, "confirm_overwrite": true})),
        ("backup_presets", json!({"device": "acme/ghost", "in_port": "x", "file": file})),
    ] {
        let r = call(&root, tool, args);
        assert!(is_error(&r), "{tool}: {r}");
        let t = text(&r);
        assert!(t.contains("acme/ghost") && t.to_lowercase().contains("describe"), "{tool}: {t}");
    }
}

#[test]
fn restore_presets_refuses_a_bank_that_cannot_be_written_before_opening_any_port() {
    let root = catalogue();
    let file = root.path().join("sound.syx").display().to_string();
    let r = call(&root, "restore_presets", json!({"device": "acme/box", "out_port": "no-such-port", "file": file,
        "confirm_overwrite": true, "bank": "Factory", "number": 1}));
    assert!(is_error(&r), "{r}");
    let t = text(&r);
    assert!(t.contains("Factory") && t.contains("writable"), "the refusal comes from the driver, not the port: {t}");
}

#[test]
fn restore_presets_still_asks_for_the_persons_consent() {
    let root = catalogue();
    let file = root.path().join("sound.syx").display().to_string();
    let r = call(&root, "restore_presets", json!({"device": "acme/box", "out_port": "x", "file": file}));
    assert!(is_error(&r) && text(&r).contains("confirm_overwrite"), "{r}");
}

// ---- the CLI --------------------------------------------------------------------

fn cli(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_osc-bridge")).args(args).output().unwrap()
}

#[test]
fn the_cli_restore_and_backup_words_require_a_device() {
    for args in [["restore", "--out-port", "0", "x.syx"].as_slice(), ["backup", "--in-port", "0", "--out", "x.syx"].as_slice()] {
        let o = cli(args);
        assert!(!o.status.success(), "{args:?}");
        assert!(String::from_utf8_lossy(&o.stderr).contains("--device"), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    }
}

#[test]
fn the_cli_restore_and_backup_help_offers_no_pause_nor_request_option() {
    let restore = String::from_utf8_lossy(&cli(&["restore", "--help"]).stdout).into_owned();
    assert!(restore.contains("--device") && !restore.contains("--gap-ms"), "{restore}");
    let backup = String::from_utf8_lossy(&cli(&["backup", "--help"]).stdout).into_owned();
    assert!(backup.contains("--device") && !backup.contains("--request"), "{backup}");
}
