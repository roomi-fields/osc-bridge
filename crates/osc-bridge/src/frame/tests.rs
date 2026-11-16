use super::*;
use osd::{Encoding, FramePart, FrameToken};

fn test_device() -> osd::Device {
    let text = r#"{"$schema":"https://w3id.org/osd/0.5/device.schema.json",
        "device":{"name":"test","vendor":"t","revision":"1","osc_prefix":"/t","manufacturer_id":[0,32,107],"device_id":[127,66]},
        "sysex":{"header":[240,0,32,107,127,66],"footer":[247]}}"#;
    match osd::read(text, osd::Kind::Device).unwrap() {
        osd::Descriptor::Device(d) => *d,
        _ => unreachable!(),
    }
}

fn arg(name: &str, encoding: Option<Encoding>) -> FrameToken {
    FrameToken::Part(FramePart::Arg { name: name.into(), encoding, add: None })
}

#[test]
fn frame_with_literals_only() {
    let d = test_device();
    let tokens = vec![FrameToken::Byte(0x02), FrameToken::Byte(0x40)];
    let f = build_frame(&d, &tokens, &Bindings::new()).unwrap();
    assert_eq!(f, vec![0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x02, 0x40, 0xF7]);
}

#[test]
fn frame_with_u7_placeholder() {
    let d = test_device();
    let tokens = vec![FrameToken::Byte(0x21), arg("val", None)];
    let mut b = Bindings::new(); b.set_u7("val", 100);
    let f = build_frame(&d, &tokens, &b).unwrap();
    assert_eq!(&f[6..=7], &[0x21, 100]);
}

#[test]
fn an_argument_with_an_offset_adds_it_on_seven_bits() {
    let d = test_device();
    let tokens = vec![FrameToken::Part(FramePart::Arg { name: "channel".into(), encoding: None, add: Some(0x30) })];
    let mut b = Bindings::new(); b.set_u7("channel", 2);
    assert_eq!(build_frame(&d, &tokens, &b).unwrap()[6], 0x32);
}

#[test]
fn frame_with_u14_split() {
    let d = test_device();
    let tokens = vec![arg("val", Some(Encoding::U14Msb)), arg("val", Some(Encoding::U14Lsb))];
    let mut b = Bindings::new(); b.set_u14("val", 12345);  // 0x3039
    let f = build_frame(&d, &tokens, &b).unwrap();
    // 12345 = 0b01100000 0111001 -> msb = 0x60, lsb = 0x39
    assert_eq!(&f[6..=7], &[0x60, 0x39]);
}

#[test]
fn s32_sends_five_7bit_groups_twos_complement() {
    let d = test_device();
    let tokens = vec![arg("val", Some(Encoding::S32))];
    let mut b = Bindings::new(); b.set_s32("val", -1);
    assert_eq!(&build_frame(&d, &tokens, &b).unwrap()[6..11], &[0x0F, 0x7F, 0x7F, 0x7F, 0x7F]);
    b.set_s32("val", -36);
    assert_eq!(&build_frame(&d, &tokens, &b).unwrap()[6..11], &[0x0F, 0x7F, 0x7F, 0x7F, 0x5C]);
    b.set_s32("val", 300);
    assert_eq!(&build_frame(&d, &tokens, &b).unwrap()[6..11], &[0x00, 0x00, 0x00, 0x02, 0x2C]);
}

#[test]
fn ascii_placeholder_filters_nonprintable() {
    let d = test_device();
    let tokens = vec![arg("text", Some(Encoding::Ascii))];
    let mut b = Bindings::new();
    b.set_bytes("text", b"Hi\x00\x80!".to_vec());
    let f = build_frame(&d, &tokens, &b).unwrap();
    assert_eq!(&f[6..=8], b"Hi!");
}
