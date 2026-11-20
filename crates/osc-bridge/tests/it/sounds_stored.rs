//! Sounds fetched and stored the way the device needs: the pause a sequence
//! states, the reply timeout, a bank that cannot be written, any channel
//! accepted on receive and the configured one on send (OSD 0.6).

use osc_bridge::programs::{dump, dump_paced, normalise_transfer, recall, recall_paced, recognise_transfer, store, Clock};
use std::time::Duration;

/// A 0.6 driver: `programs` is the block, `extra` more top-level members.
fn dev06(programs: &str, extra: &str) -> osd::Device {
    let text = format!(r#"{{"$schema":"https://w3id.org/osd/0.6/device.schema.json",
        "device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box"}},
        "midi_out":{{"default_channel":2}},"programs":{programs}{extra}}}"#);
    match osd::read(&text, osd::Kind::Device).unwrap_or_else(|e| panic!("{e}\n{text}")) {
        osd::Descriptor::Device(d) => *d,
        _ => unreachable!(),
    }
}

/// A clock that only records what it is asked to wait.
#[derive(Default)]
struct FakeClock { asked: Vec<Duration> }
impl Clock for FakeClock {
    fn sleep(&mut self, d: Duration) { self.asked.push(d); }
}

const PACED: &str = r#"{"delay_ms":30,"before":[[240,125,1,247]],
    "banks":[{"name":"A","msb":1,"lsb":2,"size":8}],
    "dump":{"kind":"request","request":[240,125,2,247],"reply_timeout_ms":500}}"#;

#[test]
fn a_recall_waits_the_declared_pause_between_its_messages() {
    let d = dev06(PACED, "");
    let mut clock = FakeClock::default();
    let sent = recall_paced(&d, "A", 1, None, &mut clock).unwrap();
    assert_eq!(sent, recall(&d, "A", 1, None).unwrap(), "same messages, same order");
    assert_eq!(sent.len(), 4);
    assert_eq!(clock.asked, vec![Duration::from_millis(30); 3], "between messages, not after the last");
}

#[test]
fn a_recall_of_one_message_waits_nothing() {
    let d = dev06(r#"{"delay_ms":30,"banks":[{"name":"A","size":8}]}"#, "");
    let mut clock = FakeClock::default();
    assert_eq!(recall_paced(&d, "A", 1, None, &mut clock).unwrap().len(), 1);
    assert!(clock.asked.is_empty());
}

#[test]
fn a_driver_that_declares_no_pause_plays_its_recall_as_before_without_waiting() {
    for programs in [
        r#"{"before":[[240,125,1,247]],"banks":[{"name":"A","msb":1,"lsb":2,"size":8}]}"#,
        r#"{"delay_ms":0,"before":[[240,125,1,247]],"banks":[{"name":"A","msb":1,"lsb":2,"size":8}]}"#,
    ] {
        let d = dev06(programs, "");
        let mut clock = FakeClock::default();
        assert_eq!(recall_paced(&d, "A", 3, Some(4), &mut clock).unwrap(), recall(&d, "A", 3, Some(4)).unwrap());
        assert!(clock.asked.is_empty(), "no wait added: {programs}");
    }
}

#[test]
fn a_recall_refuses_what_recall_refuses_without_waiting() {
    let d = dev06(PACED, "");
    let mut clock = FakeClock::default();
    assert!(recall_paced(&d, "Card", 1, None, &mut clock).unwrap_err().contains("no bank"));
    assert!(clock.asked.is_empty());
}

#[test]
fn a_dump_waits_for_the_answer_up_to_the_declared_reply_timeout() {
    let d = dev06(PACED, "");
    let mut clock = FakeClock::default();
    let sent = dump_paced(&d, None, None, &mut clock).unwrap();
    assert_eq!(sent, dump(&d, None, None).unwrap());
    assert_eq!(clock.asked, vec![Duration::from_millis(500)], "one request, then the reply bound");
}

#[test]
fn a_dump_with_no_declared_reply_timeout_adds_no_wait() {
    let d = dev06(r#"{"dump":{"kind":"request","request":[240,125,2,247]}}"#, "");
    let mut clock = FakeClock::default();
    assert_eq!(dump_paced(&d, None, None, &mut clock).unwrap(), dump(&d, None, None).unwrap());
    assert!(clock.asked.is_empty());
}

const BANKS: &str = r#"{"before":[[240,125,9,247]],"banks":[
    {"name":"Factory","msb":1,"size":8,"writable":false},
    {"name":"User","msb":2,"size":8,"writable":true},
    {"name":"Card","msb":3,"size":8}]}"#;

#[test]
fn a_bank_the_descriptor_says_cannot_be_written_is_refused_naming_it_and_why() {
    let d = dev06(BANKS, "");
    let e = store(&d, "Factory", 1, None).unwrap_err();
    assert!(e.contains("Factory"), "{e}");
    assert!(e.contains("writable"), "{e}");
    let by_place = store(&d, "0", 1, None).unwrap_err();
    assert!(by_place.contains("Factory"), "a bank by its place is named too: {by_place}");
}

#[test]
fn a_bank_that_can_be_written_or_does_not_say_is_accepted_as_a_target() {
    let d = dev06(BANKS, "");
    assert!(!store(&d, "User", 1, None).unwrap().is_empty());
    assert!(!store(&d, "card", 8, Some(1)).unwrap().is_empty(), "absent means writable");
}

#[test]
fn a_target_outside_its_bank_is_refused() {
    let d = dev06(BANKS, "");
    assert!(store(&d, "User", 9, None).unwrap_err().contains("1..=8"));
    assert!(store(&d, "Nowhere", 1, None).unwrap_err().contains("no bank"));
}

const DX7: &str = r#","sound_transfers":[
    {"id":"voice","carries":"sound","match_frame":[240,67,"any",0,1,27,"any","any","any",247],
     "elements":{"data":{"from":6,"length":3},"channel":{"from":2,"length":1}}},
    {"id":"patch","carries":"sound","match_frame":[240,65,"any",0,6,"any","any",247],
     "elements":{"data":{"from":5,"length":2},"device_number":{"from":2,"length":1}}}]"#;

fn dx7() -> osd::Device {
    dev06(r#"{"banks":[{"name":"A","size":8}]}"#, DX7)
}

#[test]
fn a_received_sound_is_recognised_on_any_channel() {
    let d = dx7();
    for n in 0..16u8 {
        let frame = [0xF0, 0x43, n, 0, 1, 0x1B, 10, 20, 30, 0xF7];
        assert_eq!(recognise_transfer(&d, &frame).as_deref(), Some("voice"), "channel {n}");
    }
}

#[test]
fn a_doubtful_channel_byte_found_in_a_file_is_recognised() {
    let d = dx7();
    assert_eq!(recognise_transfer(&d, &[0xF0, 0x43, 0x1A, 0, 1, 0x1B, 1, 2, 3, 0xF7]).as_deref(), Some("voice"));
}

#[test]
fn a_received_sound_is_recognised_whatever_its_device_number() {
    let d = dx7();
    for n in [0u8, 1, 16, 127] {
        assert_eq!(recognise_transfer(&d, &[0xF0, 0x41, n, 0, 6, 5, 6, 0xF7]).as_deref(), Some("patch"), "device {n}");
    }
}

#[test]
fn a_frame_that_is_not_a_declared_transfer_is_not_recognised() {
    let d = dx7();
    assert_eq!(recognise_transfer(&d, &[0xF0, 0x43, 0, 0, 1, 0x1C, 1, 2, 3, 0xF7]), None, "another fixed byte");
    assert_eq!(recognise_transfer(&d, &[0xF0, 0x43, 0, 0, 1, 0x1B, 1, 2, 0xF7]), None, "another length");
    let none = dev06(r#"{"banks":[{"name":"A","size":8}]}"#, "");
    assert_eq!(recognise_transfer(&none, &[0xF0, 0x43, 0, 0, 1, 0x1B, 1, 2, 3, 0xF7]), None, "no transfer declared");
}

#[test]
fn a_sound_is_sent_on_the_configured_channel_with_the_data_unchanged() {
    let d = dx7();
    for (received, channel) in [(0x00u8, 5u8), (0x0F, 0), (0x1A, 9)] {
        let frame = [0xF0, 0x43, received, 0, 1, 0x1B, 10, 20, 30, 0xF7];
        let sent = normalise_transfer(&d, &frame, channel, None).unwrap();
        assert_eq!(sent, vec![0xF0, 0x43, channel, 0, 1, 0x1B, 10, 20, 30, 0xF7], "from {received:#04x}");
    }
}

#[test]
fn a_sound_is_sent_with_the_configured_device_number() {
    let d = dx7();
    let sent = normalise_transfer(&d, &[0xF0, 0x41, 99, 0, 6, 5, 6, 0xF7], 3, Some(16)).unwrap();
    assert_eq!(sent, vec![0xF0, 0x41, 16, 0, 6, 5, 6, 0xF7]);
    let kept = normalise_transfer(&d, &[0xF0, 0x41, 99, 0, 6, 5, 6, 0xF7], 3, None).unwrap();
    assert_eq!(kept, vec![0xF0, 0x41, 99, 0, 6, 5, 6, 0xF7], "no device number set: left as received");
}

#[test]
fn a_frame_no_transfer_recognises_is_refused_at_sending() {
    let d = dx7();
    assert!(normalise_transfer(&d, &[0xF0, 0x7D, 1, 0xF7], 0, None).is_err());
}
