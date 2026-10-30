//! OB-20q.2: `{checksum:<algorithm>@<from>[+<length>]}` in every frame place.
//!
//! Definitions (Electra One preset format, "SysEx template", checksum;
//! https://docs.electra.one/developers/presetformat.html):
//! - `twos7` = roland: `(128 - (sum of bytes mod 128)) & 0x7F`
//! - `sum7`  = waldorf: `sum of bytes & 0x7F`
//! - `xor7`  = fractal: all bytes XORed together, `& 0x7F`
//!
//! `<from>` counts from the start of the rendered message, F0 = 0.
//!
//! Known messages:
//! - twos7: Roland DT1 `F0 41 10 42 12 40 00 7F 00 41 F7` (GS Reset, Roland
//!   SC-88 manual; the Electra doc's DT1 example has the same layout, start 5).
//! - xor7: Fractal Audio Axe-Fx II "get firmware version"
//!   `F0 00 01 74 03 08 0E F7` (checksum = XOR of every byte from F0, & 0x7F).
//! - sum7: a real Waldorf Blofeld Sound Dump (392 bytes, checksum 0x3E at byte 390), the
//!   first message of KnobKraft Orm's `Waldorf_Blofeld_Blo Factory_2008.syx` (Christof Ruch,
//!   AGPL-3.0); see `tests/fixtures/blofeld_sound_dump.NOTICE.md`. The maker's checksum is
//!   the sum of bytes 7..389 & 0x7F (Blofeld SysEx documentation v1.04, "sum of SDATA").

mod common;

use osc_bridge::detect::probe_of;
use osc_bridge::device::{Device, FrameToken};
use osc_bridge::frame::{build_message, Bindings};
use osc_bridge::programs;
use osc_bridge::runtime::{encode, handle_midi_in, ScriptCell};
use osc_bridge::routing::DynamicRoutes;
use rosc::{OscMessage, OscType};
use std::sync::{Arc, Mutex, OnceLock};

/// Load a driver named "Acme Box" (file `box.json`) whose body after the
/// device block is `rest` (JSON members, leading comma included).
fn load(header: &str, rest: &str) -> (tempfile::TempDir, anyhow::Result<Device>) {
    let dir = common::temp("checksum");
    let json = format!(
        r#"{{"device":{{"name":"Acme Box","osc_prefix":"/b"}},"sysex":{{"header":{header},"footer":[247]}}{rest}}}"#
    );
    let path = dir.path().join("box.json");
    std::fs::write(&path, json).unwrap();
    let dev = Device::load(&path);
    (dir, dev)
}

fn ok(header: &str, rest: &str) -> Device {
    let (_d, dev) = load(header, rest);
    dev.unwrap_or_else(|e| panic!("driver refused: {e}"))
}

/// The load error, which names the driver and the token.
fn refused(header: &str, rest: &str, token: &str) -> String {
    let (_d, dev) = load(header, rest);
    let err = dev.expect_err("the driver should be refused at load").to_string();
    assert!(err.contains(token), "the error names the token {token}: {err}");
    assert!(err.contains("Acme Box") || err.contains("box.json"), "the error names the driver: {err}");
    err
}

fn command(dev: &Device, osc: &str) -> Vec<Vec<u8>> {
    encode(dev.clone(), OscMessage { addr: dev.full_osc_path(osc), args: vec![] })
}

fn tokens(json: &str) -> Vec<FrameToken> {
    serde_json::from_str(json).unwrap()
}

const ROLAND: &str = "[240,65,16,66,18]";

// ---- criterion 1, 5: one test per algorithm on a known message ----

#[test]
fn criterion_1_5_twos7_roland_dt1_gs_reset() {
    let d = ok(ROLAND, r#","commands":[{"osc":"/gs","frame":[64,0,127,0,"{checksum:twos7@5}"]}]"#);
    assert_eq!(command(&d, "/gs"), vec![vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7]]);
}

#[test]
fn criterion_1_twos7_length_limits_the_covered_bytes() {
    // 40 + 00 = 0x40 -> 128 - 64 = 0x40; the bytes after are not summed.
    let d = ok(ROLAND, r#","commands":[{"osc":"/gs","frame":[64,0,127,0,"{checksum:twos7@5+2}"]}]"#);
    assert_eq!(command(&d, "/gs"), vec![vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x40, 0xF7]]);
    // Without length, from F0's neighbour (offset 1) to the checksum:
    // 41+10+42+12+40+00+7F+00 = 0x164 -> 0x64 -> 128-100 = 0x1C.
    let d = ok(ROLAND, r#","commands":[{"osc":"/gs","frame":[64,0,127,0,"{checksum:twos7@1}"]}]"#);
    assert_eq!(command(&d, "/gs")[0][9], 0x1C);
}

#[test]
fn criterion_1_5_twos7_is_zero_when_the_sum_is_a_multiple_of_128() {
    let d = ok(ROLAND, r#","commands":[{"osc":"/z","frame":[64,64,"{checksum:twos7@5}"]}]"#);
    assert_eq!(command(&d, "/z")[0][7], 0x00);
}

#[test]
fn criterion_1_5_sum7_waldorf_masks_the_sum() {
    // A real Blofeld Sound Dump: the byte produced from offset 7 is the maker's 0x3E.
    let hex = include_str!("fixtures/blofeld_sound_dump.hex").trim();
    let msg: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
    assert_eq!((msg.len(), msg[390], msg[391]), (392, 0x3E, 0xF7));
    let body: Vec<String> = msg[4..390].iter().map(|b| b.to_string()).collect();
    let frame = format!("[{},\"{{checksum:sum7@7}}\"]", body.join(","));
    let rest = format!(r#","commands":[{{"osc":"/w","frame":{frame}}}]"#);
    let d = ok("[240,62,19,0]", &rest);
    assert_eq!(command(&d, "/w"), vec![msg.clone()]);
    // Bytes 5 and 6 are 0 here, so a start at 5 gives the same value.
    let rest5 = rest.replace("sum7@7", "sum7@5");
    let d = ok("[240,62,19,0]", &rest5);
    assert_eq!(command(&d, "/w")[0][390], 0x3E);
}

#[test]
fn criterion_1_5_xor7_fractal_axe_fx_firmware_request() {
    let d = ok("[240,0,1,116]", r#","commands":[{"osc":"/fw","frame":[3,8,"{checksum:xor7@0}"]}]"#);
    assert_eq!(command(&d, "/fw"), vec![vec![0xF0, 0x00, 0x01, 0x74, 0x03, 0x08, 0x0E, 0xF7]]);
}

#[test]
fn criterion_1_xor7_length_and_mask() {
    // 03 ^ 08 = 0x0B over two bytes from offset 4.
    let d = ok("[240,0,1,116]", r#","commands":[{"osc":"/fw","frame":[3,8,"{checksum:xor7@4+2}"]}]"#);
    assert_eq!(command(&d, "/fw")[0][6], 0x0B);
    // F0 is masked: F0 ^ 7F = 0x8F -> & 0x7F = 0x0F.
    let d = ok("[240]", r#","commands":[{"osc":"/m","frame":[127,"{checksum:xor7@0}"]}]"#);
    assert_eq!(command(&d, "/m")[0][2], 0x0F);
}

// ---- criterion 2: every frame place ----

#[test]
fn criterion_2_param_set_and_get_frames_compute_it() {
    let d = ok(ROLAND, r#","params":{"value_type":"u7",
        "set_frame":["{p}","{val}","{checksum:twos7@5}"],
        "get_frame":["{p}","{c}","{checksum:xor7@5}"],
        "entries":[{"osc":"/cutoff","pr":0,"p":16,"c":2,"r":0,"range":[0,127]}]}"#);
    // 0x10 + 5 = 0x15 -> 0x6B.
    let set = encode(d.clone(), OscMessage { addr: d.full_osc_path("/cutoff"), args: vec![OscType::Int(5)] });
    assert_eq!(set, vec![vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x10, 0x05, 0x6B, 0xF7]]);
    // 0x10 ^ 0x02 = 0x12.
    let get = encode(d.clone(), OscMessage {
        addr: d.full_osc_path("/param/get"),
        args: vec![OscType::Int(0), OscType::Int(16), OscType::Int(2), OscType::Int(0)],
    });
    assert_eq!(get, vec![vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x10, 0x02, 0x12, 0xF7]]);
}

#[test]
fn criterion_2_programs_frames_compute_all_three_algorithms() {
    let d = ok("[]", r#","programs":{"banks":[{"name":"A","size":64}],
        "before":[[240,65,"{channel}",64,"{checksum:twos7@1}",247]],
        "dump":{"kind":"request","request":[240,62,"{index}","{checksum:sum7@1}",247]}}"#);
    // recall: channel 2 -> 41+02+40 = 0x83 -> 0x03 -> 0x7D; then the program change.
    let r = programs::recall(&d, "A", 1, Some(2)).unwrap();
    assert_eq!(r[0], vec![0xF0, 0x41, 0x02, 0x40, 0x7D, 0xF7]);
    // dump: 3E + 05 = 0x43.
    assert_eq!(programs::dump(&d, Some(("A", 6)), None).unwrap(), vec![vec![0xF0, 0x3E, 0x05, 0x43, 0xF7]]);
    let x = ok("[]", r#","programs":{"banks":[{"name":"A","size":64}],
        "dump":{"kind":"request","request":[240,0,1,116,3,8,"{checksum:xor7@0}",247]}}"#);
    assert_eq!(programs::dump(&x, Some(("A", 1)), None).unwrap(), vec![vec![0xF0, 0, 1, 0x74, 3, 8, 0x0E, 0xF7]]);
}

#[test]
fn criterion_2_build_message_computes_it_with_the_shared_function() {
    let t = tokens(r#"[240,65,16,66,18,64,0,127,0,"{checksum:twos7@5}",247]"#);
    assert_eq!(build_message(&t, &Bindings::new()).unwrap(), vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0, 0x7F, 0, 0x41, 0xF7]);
    let t = tokens(r#"[240,0,1,116,3,8,"{checksum:xor7@0}",247]"#);
    assert_eq!(build_message(&t, &Bindings::new()).unwrap(), vec![0xF0, 0, 1, 0x74, 3, 8, 0x0E, 0xF7]);
}

// ---- criterion 9: the identity request ----

#[test]
fn criterion_9_identity_request_checksums_the_replaced_dev_and_family_bytes() {
    let d = ok("[]", r#","identity":{"request":[240,65,"{dev}",6,"{family}","{checksum:twos7@1}",247],
        "device_id":{"default":16},"family":[89,1],"reply":[240,65]}"#);
    // 41+10+06+59+01 = 177 -> 49 -> 0x4F.
    let want = vec![0xF0, 0x41, 0x10, 0x06, 0x59, 0x01, 0x4F, 0xF7];
    assert_eq!(programs::identity_request(&d), want);
    assert_eq!(probe_of(d.identity.as_ref().unwrap()).unwrap().unwrap().request, want);
}

// ---- criterion 10: replies.match_frame ----

#[test]
fn criterion_10_match_frame_checksum_matches_any_byte() {
    let d = ok(ROLAND, r#","replies":[{"match_frame":[64,"{v}","{checksum:twos7@5}"],"emit_osc":"/b/got"}]"#);
    let dev = Arc::new(d);
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = seen.clone();
    let send: Arc<dyn Fn(OscMessage) + Send + Sync> = Arc::new(move |m| sink.lock().unwrap().push(m.addr));
    let routes = Arc::new(Mutex::new(DynamicRoutes::new()));
    let scripts: ScriptCell = Arc::new(OnceLock::new());
    for chk in [0x00u8, 0x55, 0x7F] {
        handle_midi_in(&[0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x09, chk, 0xF7], &dev, &send, &routes, &scripts);
    }
    assert_eq!(*seen.lock().unwrap(), vec!["/b/got"; 3], "any checksum byte matches, none falls back to raw");
}

// ---- criterion 4, 7, 11: load errors ----

#[test]
fn criterion_4_11_an_unknown_algorithm_is_refused_in_every_frame_place() {
    let bad = "{checksum:crc16@5}";
    for rest in [
        format!(r#","commands":[{{"osc":"/c","frame":[1,"{bad}"]}}]"#),
        format!(r#","params":{{"value_type":"u7","set_frame":[1,"{bad}"],"get_frame":[1],"entries":[]}}"#),
        format!(r#","params":{{"value_type":"u7","set_frame":[1],"get_frame":[1,"{bad}"],"entries":[]}}"#),
        format!(r#","identity":{{"request":[240,65,1,"{bad}",247],"reply":[240]}}"#),
        format!(r#","programs":{{"program_change":false,"before":[[240,1,"{bad}",247]],"banks":[{{"name":"A","size":4}}]}}"#),
        format!(r#","programs":{{"banks":[{{"name":"A","size":4}}],"dump":{{"kind":"request","request":[240,1,"{bad}",247]}}}}"#),
    ] {
        refused(ROLAND, &rest, bad);
    }
}

#[test]
fn criterion_3_the_former_twos_form_is_refused() {
    refused(ROLAND, r#","commands":[{"osc":"/c","frame":[1,2,"{checksum:twos@5}"]}]"#, "{checksum:twos@5}");
    refused("[]", r#","programs":{"banks":[{"name":"A","size":4}],"dump":{"kind":"request","request":[240,1,"{checksum:twos@1}",247]}}"#, "{checksum:twos@1}");
}

#[test]
fn criterion_4_a_malformed_checksum_is_refused() {
    for bad in ["{checksum:twos7}", "{checksum:twos7@}", "{checksum:twos7@x}", "{checksum:twos7@5+}", "{checksum:@5}", "{checksum:sum7@5+y}"] {
        refused(ROLAND, &format!(r#","commands":[{{"osc":"/c","frame":[1,"{bad}"]}}]"#), bad);
    }
}

#[test]
fn criterion_4_7_from_or_length_beyond_the_message_is_refused_at_load() {
    // Header is 5 bytes, one literal: the checksum sits at offset 6.
    for bad in ["{checksum:twos7@6}", "{checksum:twos7@7}", "{checksum:sum7@5+2}", "{checksum:xor7@0+99}"] {
        refused(ROLAND, &format!(r#","commands":[{{"osc":"/c","frame":[1,"{bad}"]}}]"#), bad);
    }
    // The edges are fine: the whole preceding message, or the last byte only.
    ok(ROLAND, r#","commands":[{"osc":"/c","frame":[1,"{checksum:twos7@5}"]}]"#);
    ok(ROLAND, r#","commands":[{"osc":"/c","frame":[1,"{checksum:xor7@0+6}"]}]"#);
    ok(ROLAND, r#","commands":[{"osc":"/c","frame":[1,"{checksum:sum7@5+1}"]}]"#);
}

#[test]
fn criterion_7_checksum_position_counts_the_earlier_checksum_byte() {
    // A first checksum is one byte: the second sits at offset 7, may cover it.
    let d = ok(ROLAND, r#","commands":[{"osc":"/c","frame":[1,"{checksum:sum7@5}","{checksum:sum7@5+2}"]}]"#);
    assert_eq!(command(&d, "/c")[0][5..8], [0x01, 0x01, 0x02]);
    refused(ROLAND, r#","commands":[{"osc":"/c","frame":[1,"{checksum:sum7@5}","{checksum:sum7@5+3}"]}]"#, "{checksum:sum7@5+3}");
}

#[test]
fn criterion_7_in_programs_frames_the_offset_counts_from_the_first_literal() {
    refused("[]", r#","programs":{"banks":[{"name":"A","size":4}],"dump":{"kind":"request","request":[240,1,"{checksum:twos7@2}",247]}}"#, "{checksum:twos7@2}");
    refused("[]", r#","programs":{"banks":[{"name":"A","size":4}],"dump":{"kind":"request","request":[240,1,"{checksum:twos7@1+5}",247]}}"#, "{checksum:twos7@1+5}");
}

#[test]
fn criterion_7_after_a_variable_token_the_error_comes_at_build_never_a_wrong_byte() {
    // `{family}` has no index: its size is the model's, known only at build.
    let short = r#","programs":{"banks":[{"name":"A","size":4}],"dump":{"kind":"request","request":[240,"{family}","{checksum:twos7@1+9}",247]}},
        "identity":{"device_id":{"default":16},"family":[1,2],"reply":[240]}"#;
    let d = ok("[]", short);
    let err = programs::dump(&d, Some(("A", 1)), None).expect_err("1+9 runs past the 3 bytes written");
    assert!(err.contains("{checksum:twos7@1+9}"), "{err}");
    let long = short.replace("[1,2]", "[1,2,3,4,5,6,7,8,9]");
    let d = ok("[]", &long);
    // 1..=9 are the family bytes: 45 -> 128 - 45 = 83 = 0x53.
    assert_eq!(programs::dump(&d, Some(("A", 1)), None).unwrap(),
        vec![vec![0xF0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x53, 0xF7]]);
}

// ---- criterion 8: never the footer, nor what is written after ----

#[test]
fn criterion_8_a_checksum_covers_neither_the_footer_nor_the_bytes_after_it() {
    let d = ok("[240,0,1,116]", r#","commands":[{"osc":"/a","frame":[3,"{checksum:xor7@0}",126]}]"#);
    // F0^00^01^74^03 = 0x86 -> 0x06; the trailing 7E and the F7 footer are not XORed.
    assert_eq!(command(&d, "/a"), vec![vec![0xF0, 0, 1, 0x74, 3, 0x06, 0x7E, 0xF7]]);
    // A length reaching past the checksum itself is refused at load.
    refused("[240,0,1,116]", r#","commands":[{"osc":"/a","frame":[3,"{checksum:xor7@0+7}",126]}]"#, "{checksum:xor7@0+7}");
}

// ---- criterion 6: the documents describe the form ----

#[test]
fn criterion_6_the_schema_and_the_changelog_describe_the_form() {
    let schema = std::fs::read_to_string("docs/DEVICE_JSON_SCHEMA.md").unwrap();
    let changelog = std::fs::read_to_string("CHANGELOG.md").unwrap();
    for (name, text) in [("docs/DEVICE_JSON_SCHEMA.md", &schema), ("CHANGELOG.md", &changelog)] {
        for w in ["twos7", "sum7", "xor7", "{checksum:"] {
            assert!(text.contains(w), "{name} should describe {w}");
        }
    }
    assert!(!schema.contains("checksum:twos@"), "the former form is gone from the schema");
}
