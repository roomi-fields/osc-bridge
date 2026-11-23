//! The reference frames of the relay protocol (`tests/fixtures/relay_frames.json`): the contract
//! the other doors read. What `osc-bridge connect` builds equals them field for field; what it
//! reads is recognised from them.

use osc_bridge::synthub::relay_protocol::{
    confirm_reply_frame, is_identity_request, midi_in_frame, needs_confirmation, parse_door_frame,
    ports_frame, share_frame, ServerFrame,
};
use serde_json::{json, Value};

fn reference() -> Value {
    serde_json::from_str(include_str!("../fixtures/relay_frames.json")).expect("reference frames are JSON")
}

fn frame_text(name: &str) -> String {
    reference()[name].to_string()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn messages_of(name: &str) -> Vec<Vec<u8>> {
    reference()[name]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_array().unwrap().iter().map(|b| b.as_u64().unwrap() as u8).collect())
        .collect()
}

#[test]
fn ports_frame_equals_its_reference_frame() {
    let frame = ports_frame(&strings(&["Out A", "Out B"]), &strings(&["In A"]));
    assert_eq!(frame, reference()["ports"]);
}

#[test]
fn ports_frame_with_no_ports_keeps_both_lists() {
    let frame = ports_frame(&[], &[]);
    assert_eq!(frame, json!({ "type": "ports", "outputs": [], "inputs": [] }));
}

#[test]
fn share_frame_equals_its_reference_frame() {
    assert_eq!(share_frame(true, &strings(&["Out B"])), reference()["share"]);
}

#[test]
fn share_frame_disabled_without_private_ports() {
    assert_eq!(share_frame(false, &[]), json!({ "type": "share", "enabled": false, "private": [] }));
}

#[test]
fn midi_in_frame_equals_its_reference_frame() {
    assert_eq!(midi_in_frame("In A", &[144, 60, 100]), reference()["midi_in"]);
}

#[test]
fn confirm_reply_frame_equals_its_reference_frame() {
    assert_eq!(confirm_reply_frame(&json!("c1"), true), reference()["confirm_reply"]);
}

#[test]
fn confirm_reply_frame_refusal_keeps_the_request_id() {
    assert_eq!(
        confirm_reply_frame(&json!("c1"), false),
        json!({ "type": "confirm_reply", "id": "c1", "accepted": false })
    );
}

#[test]
fn hello_frame_is_read_with_its_code_and_user() {
    assert_eq!(
        parse_door_frame(&frame_text("hello")),
        Some(ServerFrame::Hello { code: "ABC-123".into(), user: "u_42".into() })
    );
}

#[test]
fn midi_out_frame_is_read_with_its_port_and_bytes() {
    assert_eq!(
        parse_door_frame(&frame_text("midi_out")),
        Some(ServerFrame::MidiOut { port: "Out A".into(), data: vec![176, 74, 64] })
    );
}

#[test]
fn confirm_frame_is_read_with_every_field() {
    assert_eq!(
        parse_door_frame(&frame_text("confirm")),
        Some(ServerFrame::Confirm {
            id: json!("c1"),
            port: "Out A".into(),
            messages: messages_of("confirm"),
            bytes: 15,
            gap_ms: 50,
            why: Some("stores a program".into()),
        })
    );
}

#[test]
fn confirm_cancel_frame_is_read_with_its_id() {
    assert_eq!(parse_door_frame(&frame_text("confirm_cancel")), Some(ServerFrame::ConfirmCancel { id: json!("c1") }));
}

#[test]
fn paired_frame_is_recognised() {
    assert_eq!(parse_door_frame(&frame_text("paired")), Some(ServerFrame::Paired));
}

#[test]
fn text_that_is_not_a_known_frame_is_not_read() {
    assert_eq!(parse_door_frame("not json"), None);
    assert_eq!(parse_door_frame(r#"{"type":"unheard_of"}"#), None);
    assert_eq!(parse_door_frame("{}"), None);
}

#[test]
fn confirmation_rule_holds_on_the_reference_messages() {
    for m in messages_of("confirm") {
        let identity = m[..] == [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];
        assert_eq!(needs_confirmation(&m), !identity);
    }
    assert!(needs_confirmation(&messages_of("confirm")[0]));

    let midi_out: Vec<u8> = reference()["midi_out"]["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap() as u8)
        .collect();
    assert!(!needs_confirmation(&midi_out));
    assert!(!is_identity_request(&midi_out));
    assert!(is_identity_request(&[0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]));
    assert!(!is_identity_request(&messages_of("confirm")[0]));
}
