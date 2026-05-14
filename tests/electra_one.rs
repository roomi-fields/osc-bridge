//! Electra One frame assembly tests — verify bytes match the published SysEx
//! implementation reference verbatim.

use osc_bridge::device::Device;
use osc_bridge::frame::{build_frame, Bindings};

fn load() -> Device {
    Device::load("devices/electra-one/electra-one.json").expect("load electra-one.json")
}

fn find_frame<'a>(dev: &'a Device, osc: &str) -> &'a [osc_bridge::device::FrameToken] {
    &dev.commands.iter().find(|c| c.osc == osc)
        .unwrap_or_else(|| panic!("no command {osc}"))
        .frame
}

#[test]
fn header_and_footer() {
    let d = load();
    assert_eq!(d.sysex.header, vec![0xF0, 0x00, 0x21, 0x45]);
    assert_eq!(d.sysex.footer, vec![0xF7]);
}

#[test]
fn get_info_command() {
    let d = load();
    let f = build_frame(&d, find_frame(&d, "/info/get"), &Bindings::new()).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x21, 0x45, 0x02, 0x7F, 0xF7]);
}

#[test]
fn preset_switch_bank_slot() {
    let d = load();
    let mut b = Bindings::new();
    b.set_u7("bank", 2);
    b.set_u7("slot", 7);
    let f = build_frame(&d, find_frame(&d, "/preset/switch"), &b).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x21, 0x45, 0x09, 0x08, 2, 7, 0xF7]);
}

#[test]
fn page_switch() {
    let d = load();
    let mut b = Bindings::new();
    b.set_u7("page", 3);
    let f = build_frame(&d, find_frame(&d, "/page/switch"), &b).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x21, 0x45, 0x09, 0x0A, 3, 0xF7]);
}

#[test]
fn bottom_bar_text() {
    let d = load();
    let mut b = Bindings::new();
    b.set_bytes("text", b"Hello".to_vec());
    let f = build_frame(&d, find_frame(&d, "/bottom_bar/text"), &b).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x21, 0x45, 0x14, 0x77, b'H', b'e', b'l', b'l', b'o', 0xF7]);
}

#[test]
fn value_text_with_u14_control_id() {
    let d = load();
    let mut b = Bindings::new();
    b.set_u14("control_id", 0x2345); // 9029 = 0b 01000110 1000101 -> msb=0x46, lsb=0x45
    b.set_u7("value_id", 1);
    b.set_bytes("text", b"A".to_vec());
    let f = build_frame(&d, find_frame(&d, "/value_text"), &b).unwrap();
    // F0 00 21 45 14 0E <lsb=0x45> <msb=0x46> 01 'A' F7
    assert_eq!(f, vec![0xF0, 0x00, 0x21, 0x45, 0x14, 0x0E, 0x45, 0x46, 0x01, b'A', 0xF7]);
}

#[test]
fn reboot() {
    let d = load();
    let f = build_frame(&d, find_frame(&d, "/reboot"), &Bindings::new()).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x21, 0x45, 0x7F, 0x78, 0xF7]);
}

#[test]
fn preset_remove() {
    let d = load();
    let mut b = Bindings::new();
    b.set_u7("bank", 0);
    b.set_u7("slot", 5);
    let f = build_frame(&d, find_frame(&d, "/preset/remove"), &b).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x21, 0x45, 0x05, 0x01, 0, 5, 0xF7]);
}

#[test]
fn all_commands_build_with_filled_bindings() {
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
            .unwrap_or_else(|e| panic!("build_frame failed for {}: {e}", cmd.osc));
        assert_eq!(f.first(), Some(&0xF0), "cmd {} missing SOX", cmd.osc);
        assert_eq!(f.last(),  Some(&0xF7), "cmd {} missing EOX", cmd.osc);
        assert_eq!(&f[1..4], &[0x00, 0x21, 0x45], "cmd {} wrong MID", cmd.osc);
    }
}
