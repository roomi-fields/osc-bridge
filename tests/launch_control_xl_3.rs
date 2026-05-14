//! Launch Control XL 3 frame & CC tests — verify bytes against Novation's
//! programmer's reference guide.

use osc_bridge::device::Device;
use osc_bridge::frame::{build_frame, Bindings};

fn load() -> Device {
    Device::load("devices/novation/launch-control-xl-3.json").expect("load lcxl3 json")
}

fn find_frame<'a>(dev: &'a Device, osc: &str) -> &'a [osc_bridge::device::FrameToken] {
    &dev.commands.iter().find(|c| c.osc == osc)
        .unwrap_or_else(|| panic!("no command {osc}"))
        .frame
}

#[test]
fn header_and_footer() {
    let d = load();
    assert_eq!(d.sysex.header, vec![0xF0, 0x00, 0x20, 0x29, 0x02, 0x15]);
    assert_eq!(d.sysex.footer, vec![0xF7]);
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

#[test]
fn cc_params_count_and_layout() {
    let d = load();
    let ccs = d.cc_params.as_ref().expect("cc_params");
    // 12 feature + 8 fader LEDs + 24 encoder LEDs + 8 solo + 8 mute + 9 misc = 69
    assert_eq!(ccs.entries.len(), 69);
    // Default channel for feature controls = 6 (MIDI ch 7)
    assert_eq!(ccs.channel, 6);
    // Feature: surface mode CC 30
    let sm = ccs.entries.iter().find(|e| e.osc == "/feature/surface_mode").unwrap();
    assert_eq!(sm.cc, Some(30));
    // LED palette for encoder row1/1 is CC 13 on channel 0 (MIDI ch 1)
    let e1 = ccs.entries.iter().find(|e| e.osc == "/led/palette/encoder/row1/1").unwrap();
    assert_eq!(e1.cc, Some(13));
    assert_eq!(e1.channel, Some(0));
}

#[test]
fn all_commands_build() {
    let d = load();
    for cmd in &d.commands {
        let mut b = Bindings::new();
        for spec in &cmd.args {
            match spec.ty.as_str() {
                "u7" | "bool" | "enum" => b.set_u7(&spec.name, 1),
                "u14" => b.set_u14(&spec.name, 1),
                "string" => b.set_bytes(&spec.name, b"x".to_vec()),
                _ => panic!("unknown arg type {} in {}", spec.ty, cmd.osc),
            }
        }
        let f = build_frame(&d, &cmd.frame, &b)
            .unwrap_or_else(|e| panic!("build_frame {}: {e}", cmd.osc));
        assert_eq!(f.first(), Some(&0xF0), "{} missing SOX", cmd.osc);
        assert_eq!(f.last(),  Some(&0xF7), "{} missing EOX", cmd.osc);
        assert_eq!(&f[1..6], &[0x00, 0x20, 0x29, 0x02, 0x15], "{} wrong header", cmd.osc);
    }
}
