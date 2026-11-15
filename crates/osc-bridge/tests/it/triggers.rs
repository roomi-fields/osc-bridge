//! Named note triggers (`triggers` block): drum machines and samplers.

use osc_bridge::device::Device;
use osc_bridge::runtime::trigger_note_on;
use rosc::{OscMessage, OscType};

fn drum_machine() -> Device {
    serde_json::from_str(r#"{
        "device": { "name": "Kit", "osc_prefix": "/kit" },
        "midi_out": { "default_channel": 3 },
        "triggers": { "entries": [
            { "osc": "/drum/kick", "sound": "Kick", "channel": 9,
              "note_range": [36, 36], "note": 36 },
            { "osc": "/drum/snare", "sound": "Snare", "channel_mode": "basic",
              "note_range": [38, 38], "note": 38 },
            { "osc": "/drum/chromatic", "sound": "Chromatic", "channel": 0,
              "note_range": [12, 59] }
        ]}
    }"#).unwrap()
}

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage { addr: addr.into(), args }
}

#[test]
fn fixed_trigger_uses_its_note_channel_and_default_velocity() {
    let bytes = trigger_note_on(&drum_machine(), &msg("/kit/drum/kick", vec![])).unwrap().unwrap();
    assert_eq!(bytes, vec![0x99, 36, 100]);
}

#[test]
fn velocity_arg_is_clamped_to_a_playable_note_on() {
    let d = drum_machine();
    let loud = trigger_note_on(&d, &msg("/kit/drum/kick", vec![OscType::Int(300)])).unwrap().unwrap();
    assert_eq!(loud[2], 127);
    // Velocity 0 would read as Note Off on most gear.
    let zero = trigger_note_on(&d, &msg("/kit/drum/kick", vec![OscType::Int(0)])).unwrap().unwrap();
    assert_eq!(zero[2], 1);
}

#[test]
fn basic_channel_trigger_falls_back_to_midi_out_default_channel() {
    let bytes = trigger_note_on(&drum_machine(), &msg("/kit/drum/snare", vec![OscType::Int(90)])).unwrap().unwrap();
    assert_eq!(bytes, vec![0x93, 38, 90]);
}

#[test]
fn explicit_channel_arg_overrides() {
    let args = vec![OscType::Int(90), OscType::Int(38), OscType::Int(5)];
    let bytes = trigger_note_on(&drum_machine(), &msg("/kit/drum/snare", args)).unwrap().unwrap();
    assert_eq!(bytes[0], 0x95);
}

#[test]
fn range_without_natural_note_requires_a_note_inside_the_range() {
    let d = drum_machine();
    assert!(trigger_note_on(&d, &msg("/kit/drum/chromatic", vec![OscType::Int(80)])).unwrap().is_err());
    let out = vec![OscType::Int(80), OscType::Int(70)];
    assert!(trigger_note_on(&d, &msg("/kit/drum/chromatic", out)).unwrap().is_err());
    let ok = vec![OscType::Int(80), OscType::Int(40)];
    assert_eq!(trigger_note_on(&d, &msg("/kit/drum/chromatic", ok)).unwrap().unwrap(), vec![0x90, 40, 80]);
}

#[test]
fn non_trigger_address_is_not_claimed() {
    assert!(trigger_note_on(&drum_machine(), &msg("/kit/cutoff", vec![])).is_none());
}

#[test]
fn imported_midi_guide_drum_map_fires_the_documented_note() {
    let d = Device::load("tests/fixtures/devices/vermona/drm1_mkiv.pencilresearch.fw-unknown.json").unwrap();
    let bytes = trigger_note_on(&d, &msg("/drm1_mkiv/drum/kick", vec![OscType::Int(110)])).unwrap().unwrap();
    assert_eq!(bytes, vec![0x99, 36, 110], "DRM1 MKIV kick: note 36 on channel 10");
}
