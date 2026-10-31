//! OB-20q.4: a command's `{checksum:...}` is computed after the command's
//! `script`, over the bytes the script leaves (the checksum covers the bytes as
//! they leave, as the Electra One preset format computes its SysEx template
//! checksum over the final bytes,
//! https://docs.electra.one/developers/presetformat.html, SysEx template).
//!
//! The frame shape is that of the 88 Electra One commands SyntHub SH-cgi.7
//! reimports (roland/jv_1080.electra-ralph-mertingk-patch-editor, boss/boss_rv_70,
//! roland/sde_330): Roland DT1 header `F0 41 10 6A 12` (JV-1080), an address, a
//! value, a function byte the command's Lua writes, then `twos7` from the
//! address. Every test goes through `encode`, which runs `dispatch_command` and
//! then `maybe_run_cmd_script`.

mod common;

use osc_bridge::device::Device;
use osc_bridge::frame::{build_frame, Bindings};
use osc_bridge::runtime::encode;
use rosc::OscMessage;

fn load(frame: &str, script: Option<&str>) -> (tempfile::TempDir, Device) {
    let dir = common::temp("checksum_after_script");
    let script = script.map(|s| format!(r#","script":{}"#, serde_json::to_string(s).unwrap())).unwrap_or_default();
    let json = format!(
        r#"{{"device":{{"name":"Acme JV","osc_prefix":"/jv"}},"sysex":{{"header":[240,65,16,106,18],"footer":[247]}},
"commands":[{{"osc":"/fn","frame":{frame}{script}}}]}}"#
    );
    let path = dir.path().join("jv.json");
    std::fs::write(&path, json).unwrap();
    let dev = Device::load(&path).unwrap_or_else(|e| panic!("driver refused: {e}"));
    (dir, dev)
}

fn send(dev: &Device) -> Vec<Vec<u8>> {
    encode(dev.clone(), OscMessage { addr: dev.full_osc_path("/fn"), args: vec![] })
}

/// Roland checksum, written here apart from the engine.
fn twos7(bytes: &[u8]) -> u8 {
    let s: u32 = bytes.iter().map(|b| *b as u32).sum();
    ((128 - (s % 128)) & 0x7F) as u8
}

const HEADER: [u8; 5] = [0xF0, 0x41, 0x10, 0x6A, 0x12];
// address 03 00 20 00, value 0x45, function byte placeholder 00, checksum.
const JV_FRAME: &str = r#"[3,0,32,0,69,0,"{checksum:twos7@5}"]"#;
// Lua indexes from 1: header 1-5, address 6-9, value 10, function byte 11, checksum 12, F7 13.
const WRITE_FUNCTION_BYTE: &str = "ctx = ...\nctx.payload[11] = 0x25\nreturn ctx";

fn expected(function: u8) -> Vec<u8> {
    let mut v = HEADER.to_vec();
    v.extend_from_slice(&[3, 0, 32, 0, 69, function]);
    let cs = twos7(&v[5..]);
    v.push(cs);
    v.push(0xF7);
    v
}

#[test]
fn criterion_1_3_7_the_checksum_covers_the_function_byte_the_script_wrote() {
    let (_d, dev) = load(JV_FRAME, Some(WRITE_FUNCTION_BYTE));
    let sent = send(&dev);
    assert_eq!(sent, vec![expected(0x25)]);
    assert_ne!(sent[0][11], expected(0x00)[11], "not the checksum over the placeholder byte");
}

#[test]
fn criterion_1_3_another_function_byte_another_checksum() {
    let (_d, dev) = load(JV_FRAME, Some("ctx = ...\nctx.payload[11] = 0x7E\nreturn ctx"));
    assert_eq!(send(&dev), vec![expected(0x7E)]);
}

#[test]
fn criterion_1_a_length_limited_checksum_is_computed_after_the_script_too() {
    let (_d, dev) = load(r#"[3,0,32,0,69,0,"{checksum:twos7@5+6}"]"#, Some(WRITE_FUNCTION_BYTE));
    assert_eq!(send(&dev), vec![expected(0x25)]);
}

#[test]
fn criterion_1_sum7_and_xor7_follow_the_script_as_well() {
    let (_d, dev) = load(r#"[3,0,32,0,69,0,"{checksum:sum7@5}"]"#, Some(WRITE_FUNCTION_BYTE));
    assert_eq!(send(&dev)[0][11], ((3u32 + 32 + 69 + 0x25) & 0x7F) as u8);
    let (_d, dev) = load(r#"[3,0,32,0,69,0,"{checksum:xor7@5}"]"#, Some(WRITE_FUNCTION_BYTE));
    assert_eq!(send(&dev)[0][11], (3 ^ 32 ^ 69 ^ 0x25) & 0x7F);
}

#[test]
fn criterion_2_a_command_without_script_builds_as_build_frame_does() {
    let (_d, dev) = load(JV_FRAME, None);
    let direct = build_frame(&dev, &dev.commands[0].frame, &Bindings::new()).unwrap();
    assert_eq!(send(&dev), vec![direct]);
    assert_eq!(send(&dev), vec![expected(0x00)]);
}

#[test]
fn criterion_8_build_frame_alone_still_computes_the_checksum_while_it_builds() {
    let (_d, dev) = load(JV_FRAME, Some(WRITE_FUNCTION_BYTE));
    let direct = build_frame(&dev, &dev.commands[0].frame, &Bindings::new()).unwrap();
    assert_eq!(direct, expected(0x00));
}

#[test]
fn criterion_10_the_script_sees_a_zero_byte_where_a_checksum_goes() {
    // The script copies what it sees at the checksum place (Lua index 12) into the function byte, plus 0x40.
    let (_d, dev) = load(JV_FRAME, Some("ctx = ...\nctx.payload[11] = 0x40 + ctx.payload[12]\nreturn ctx"));
    assert_eq!(send(&dev), vec![expected(0x40)]);
    // The frame it receives already has the checksum's slot: 13 bytes.
    let (_d, dev) = load(JV_FRAME, Some("ctx = ...\nctx.payload[11] = #ctx.payload\nreturn ctx"));
    assert_eq!(send(&dev), vec![expected(13)]);
}

#[test]
fn criterion_11_checksums_go_left_to_right_and_the_second_sees_the_final_first() {
    let frame = r#"[3,0,32,0,69,0,"{checksum:twos7@5}","{checksum:sum7@5}"]"#;
    let (_d, dev) = load(frame, Some(WRITE_FUNCTION_BYTE));
    let mut want = expected(0x25);
    want.pop(); // F7
    let cs2 = (want[5..].iter().map(|b| *b as u32).sum::<u32>() & 0x7F) as u8;
    want.push(cs2);
    want.push(0xF7);
    assert_eq!(send(&dev), vec![want]);
}

#[test]
fn criterion_4_6_a_script_that_inserts_a_byte_before_a_checksum_sends_nothing() {
    let (_d, dev) = load(JV_FRAME, Some("ctx = ...\ntable.insert(ctx.payload, 11, 0x33)\nreturn ctx"));
    assert!(send(&dev).is_empty());
}

#[test]
fn criterion_6_a_script_that_removes_or_appends_bytes_sends_nothing() {
    let (_d, dev) = load(JV_FRAME, Some("ctx = ...\ntable.remove(ctx.payload, 11)\nreturn ctx"));
    assert!(send(&dev).is_empty(), "shorter");
    let (_d, dev) = load(JV_FRAME, Some("ctx = ...\nctx.payload[14] = 0x01\nreturn ctx"));
    assert!(send(&dev).is_empty(), "longer");
}

#[test]
fn criterion_6_a_script_may_change_the_length_of_a_frame_without_checksum() {
    let (_d, dev) = load("[1,2]", Some("ctx = ...\nctx.payload[#ctx.payload+1] = 0x09\nreturn ctx"));
    assert_eq!(send(&dev).len(), 1);
}

#[test]
fn criterion_9_an_unreadable_checksum_is_refused_with_or_without_script() {
    for frame in [r#"[1,"{checksum:crc9@5}"]"#, r#"[1,"{checksum:twos7@50}"]"#, r#"[1,"{checksum}"]"#] {
        let (_d, dev) = load(frame, Some(WRITE_FUNCTION_BYTE));
        assert!(send(&dev).is_empty(), "{frame} with script");
        let (_d, dev) = load(frame, None);
        assert!(send(&dev).is_empty(), "{frame} without script");
    }
}

#[test]
fn criterion_12_a_lua_that_writes_the_right_checksum_sends_the_same_bytes() {
    let lua = "ctx = ...\nctx.payload[11] = 0x25\n\
        local s = 0\nfor i = 6, 11 do s = s + ctx.payload[i] end\n\
        ctx.payload[12] = (128 - (s % 128)) % 128\nreturn ctx";
    let (_d, dev) = load(JV_FRAME, Some(lua));
    assert_eq!(send(&dev), vec![expected(0x25)]);
}

#[test]
fn criterion_12_a_lua_that_writes_another_value_is_overwritten_by_the_engine() {
    let (_d, dev) = load(JV_FRAME, Some("ctx = ...\nctx.payload[11] = 0x25\nctx.payload[12] = 0x01\nreturn ctx"));
    assert_eq!(send(&dev), vec![expected(0x25)]);
}

#[test]
fn criterion_5_the_changelog_says_checksums_follow_the_script() {
    common::changelog::section_with_words(&["checksum", "script"]);
}
