//! Performance MIDI OSC routes (note on/off, pitchbend, CC, program change).

use osc_bridge::device::{Device, DeviceMeta, MidiOut, Sysex};
use rosc::{OscMessage, OscType};

// `try_midi_out` is private; we drive it through a local copy of the routing
// logic by constructing a Device and calling the public dispatch through a
// loopback UDP isn't worth it for a unit test. Instead: expose just enough via
// a tiny re-exported helper in the runtime. For now, we test the behaviour by
// driving Runtime's OSC path with a mocked socket would be heavy, so: test the
// device loading, and test the byte assembly by an internal helper.
//
// Strategy: parse a minimal device JSON with midi_out, then call the exposed
// helper via a public shim added in runtime.rs for testing.

fn bare_device(mo: Option<MidiOut>) -> Device {
    Device {
        device: DeviceMeta {
            name: "t".into(), vendor: "t".into(), revision: "1".into(),
            osc_prefix: "/t".into(),
            manufacturer_id: vec![0x7F],
            device_id: vec![],
            rate_limit_hz: None,
            kind: None,
            transport: None,
        },
        sysex: Sysex::default(),
        commands: vec![], params: None, cc_params: None,
        midi_out: mo,
        midi_in: Default::default(),
        replies: vec![],
        subscriptions: vec![],
        custom_commands: vec![], docs: None,
    }
}

/// Public test shim — see `runtime::midi_out_for_msg`.
fn build(dev: &Device, addr: &str, args: Vec<OscType>) -> Option<Vec<u8>> {
    osc_bridge::runtime::midi_out_for_msg(
        dev,
        &OscMessage { addr: addr.into(), args },
    )
}

#[test]
fn note_on_default_channel() {
    let d = bare_device(Some(MidiOut { default_channel: 0, note_offset: 0 }));
    let r = build(&d, "/t/note/on", vec![OscType::Int(60), OscType::Int(100)]).unwrap();
    assert_eq!(r, vec![0x90, 60, 100]);
}

#[test]
fn note_on_override_channel() {
    let d = bare_device(Some(MidiOut { default_channel: 0, note_offset: 0 }));
    let r = build(&d, "/t/note/on",
        vec![OscType::Int(60), OscType::Int(100), OscType::Int(9)]).unwrap();
    assert_eq!(r, vec![0x90 | 9, 60, 100]);
}

#[test]
fn note_on_with_note_offset_for_drums() {
    let d = bare_device(Some(MidiOut { default_channel: 9, note_offset: 36 }));
    // OSC sends note 0 ("kick" in Kanopi-speak), actual MIDI note = 36
    let r = build(&d, "/t/note/on", vec![OscType::Int(0), OscType::Int(120)]).unwrap();
    assert_eq!(r, vec![0x90 | 9, 36, 120]);
}

#[test]
fn note_off_default_velocity_zero() {
    let d = bare_device(Some(MidiOut::default()));
    let r = build(&d, "/t/note/off", vec![OscType::Int(60)]).unwrap();
    assert_eq!(r, vec![0x80, 60, 0]);
}

#[test]
fn pitchbend_u14_split() {
    let d = bare_device(Some(MidiOut::default()));
    // 8192 = centre = 0x2000 → lsb=0x00, msb=0x40
    let r = build(&d, "/t/pitchbend", vec![OscType::Int(8192)]).unwrap();
    assert_eq!(r, vec![0xE0, 0x00, 0x40]);
    // 12345 = 0x3039 → lsb=0x39, msb=0x60
    let r = build(&d, "/t/pitchbend", vec![OscType::Int(12345)]).unwrap();
    assert_eq!(r, vec![0xE0, 0x39, 0x60]);
}

#[test]
fn aftertouch_channel_pressure() {
    let d = bare_device(Some(MidiOut::default()));
    let r = build(&d, "/t/aftertouch", vec![OscType::Int(64)]).unwrap();
    assert_eq!(r, vec![0xD0, 64]);
}

#[test]
fn poly_aftertouch() {
    let d = bare_device(Some(MidiOut { default_channel: 3, note_offset: 0 }));
    let r = build(&d, "/t/poly_aftertouch",
        vec![OscType::Int(60), OscType::Int(80)]).unwrap();
    assert_eq!(r, vec![0xA0 | 3, 60, 80]);
}

#[test]
fn cc_via_path_number() {
    let d = bare_device(Some(MidiOut::default()));
    let r = build(&d, "/t/cc/74", vec![OscType::Int(80)]).unwrap();
    assert_eq!(r, vec![0xB0, 74, 80]);
}

#[test]
fn program_change() {
    let d = bare_device(Some(MidiOut::default()));
    let r = build(&d, "/t/program_change", vec![OscType::Int(5)]).unwrap();
    assert_eq!(r, vec![0xC0, 5]);
}

#[test]
fn absent_midi_out_leaves_routing_untouched() {
    let d = bare_device(None);
    let r = build(&d, "/t/note/on", vec![OscType::Int(60), OscType::Int(100)]);
    assert!(r.is_none(), "no midi_out => no performance route");
}

#[test]
fn values_clamp_to_u7() {
    let d = bare_device(Some(MidiOut::default()));
    // vel 200 clamps to 127
    let r = build(&d, "/t/note/on", vec![OscType::Int(60), OscType::Int(200)]).unwrap();
    assert_eq!(r, vec![0x90, 60, 127]);
}
