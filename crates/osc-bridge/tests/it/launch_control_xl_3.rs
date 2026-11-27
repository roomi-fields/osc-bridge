//! Launch Control XL 3 frame & CC tests — verify bytes against Novation's
//! programmer's reference guide.

use crate::common::address_text;
use osc_bridge::device::load as load_driver;
use osc_bridge::frame::{build_frame, Bindings};

fn load() -> osd::Device {
    load_driver("tests/fixtures/devices/novation/launch-control-xl-3.json").expect("load lcxl3 json")
}

fn find_frame<'a>(dev: &'a osd::Device, osc: &str) -> &'a [osd::FrameToken] {
    dev.commands.iter().flatten().find(|c| address_text(&c.osc) == osc)
        .unwrap_or_else(|| panic!("no command {osc}"))
        .frame.as_deref().unwrap_or_default()
}

#[test]
fn the_header_is_the_one_of_the_driver_and_0_7_holds_no_footer() {
    let d = load();
    let sysex = d.sysex.as_ref().unwrap();
    assert_eq!(sysex.header, Some(vec![0xF0, 0x00, 0x20, 0x29, 0x02, 0x15]));
    assert_eq!(sysex.footer, None);
}

#[test]
fn daw_mode_enable() {
    let d = load();
    let f = build_frame(&d, find_frame(&d, "/daw_mode/enable"), &Bindings::new()).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x20, 0x29, 0x02, 0x15, 0x02, 0x7F, 0xF7]);
}

#[test]
fn daw_mode_disable() {
    let d = load();
    let f = build_frame(&d, find_frame(&d, "/daw_mode/disable"), &Bindings::new()).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x20, 0x29, 0x02, 0x15, 0x02, 0x00, 0xF7]);
}

#[test]
fn led_rgb() {
    // Per docs: F0 00 20 29 02 15 01 53 <ctrl> <R> <G> <B> F7
    let d = load();
    let mut b = Bindings::new();
    b.set_u7("control", 13);
    b.set_u7("r", 127);
    b.set_u7("g", 0);
    b.set_u7("b", 64);
    let f = build_frame(&d, find_frame(&d, "/led/rgb"), &b).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x20, 0x29, 0x02, 0x15, 0x01, 0x53, 13, 127, 0, 64, 0xF7]);
}

#[test]
fn display_configure() {
    let d = load();
    let mut b = Bindings::new();
    b.set_u7("target", 0x35);
    b.set_u7("config", 0x04);
    let f = build_frame(&d, find_frame(&d, "/display/configure"), &b).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x20, 0x29, 0x02, 0x15, 0x04, 0x35, 0x04, 0xF7]);
}

#[test]
fn display_text() {
    let d = load();
    let mut b = Bindings::new();
    b.set_u7("target", 0x35);
    b.set_u7("field", 0);
    b.set_bytes("text", b"Hello".to_vec());
    let f = build_frame(&d, find_frame(&d, "/display/text"), &b).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x20, 0x29, 0x02, 0x15, 0x06, 0x35, 0x00,
                       b'H', b'e', b'l', b'l', b'o', 0xF7]);
}

/// The CC number of a control reached by one CC way.
fn only_cc(e: &osd::CcEntry) -> i64 {
    match e.ways.as_slice() {
        [w] if w.ctrl_type == "cc" => w.ctrl_index.as_ref().unwrap()[0],
        other => panic!("{}: not one cc way: {other:?}", e.osc),
    }
}

#[test]
fn cc_params_count_and_layout() {
    let d = load();
    let ccs = d.cc_params.as_ref().expect("cc_params");
    // 12 feature + 8 fader LEDs + 24 encoder LEDs + 8 solo + 8 mute + 9 misc = 69
    assert_eq!(ccs.entries.len(), 69);
    // Default channel for feature controls = 6 (MIDI ch 7)
    assert_eq!(ccs.channel, Some(6));
    // Feature: surface mode CC 30
    let sm = ccs.entries.iter().find(|e| e.osc == "/feature/surface_mode").unwrap();
    assert_eq!(only_cc(sm), 30);
    // LED palette for encoder row1/1 is CC 13 on channel 0 (MIDI ch 1)
    let e1 = ccs.entries.iter().find(|e| e.osc == "/led/palette/encoder/row1/1").unwrap();
    assert_eq!(only_cc(e1), 13);
    assert_eq!(e1.channel, Some(0));
}

#[test]
fn all_commands_build() {
    let d = load();
    for cmd in d.commands.iter().flatten() {
        let osc = address_text(&cmd.osc);
        let mut b = Bindings::new();
        for spec in cmd.args.iter().flatten() {
            match spec.ty {
                osd::ArgType::U7 | osd::ArgType::Bool | osd::ArgType::Enum => b.set_u7(&spec.name, 1),
                osd::ArgType::U14 => b.set_u14(&spec.name, 1),
                osd::ArgType::String => b.set_bytes(&spec.name, b"x".to_vec()),
                other => panic!("unknown arg type {other:?} in {osc}"),
            }
        }
        let f = build_frame(&d, cmd.frame.as_deref().unwrap_or_default(), &b)
            .unwrap_or_else(|e| panic!("build_frame {osc}: {e}"));
        assert_eq!(f.first(), Some(&0xF0), "{osc} missing SOX");
        assert_eq!(f.last(),  Some(&0xF7), "{osc} missing EOX");
        assert_eq!(&f[1..6], &[0x00, 0x20, 0x29, 0x02, 0x15], "{osc} wrong header");
    }
}
