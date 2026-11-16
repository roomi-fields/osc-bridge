//! A checksum `osd` cannot read (an unknown algorithm, no offset) refuses the driver at
//! load, in every place a frame stands, with `osd`'s fault. A checksum `osd` reads but no
//! frame could compute (an offset on its own place, a length past the bytes written) still
//! loads: it fails where it is used, naming the driver and the block, and `lint` and
//! `inspect` warn about it.
//!
//! Checksum definitions: see `checksum.rs` (Electra One preset format).

use crate::common::{self, D05};

use osc_bridge::device::{full_osc_path, load};
use osc_bridge::frame::checksum_problems;
use osc_bridge::programs;
use osc_bridge::runtime::encode;
use rosc::{OscMessage, OscType};
use std::path::PathBuf;

const ROLAND: &str = "[240,65,16,66,18]";

/// An OSD 0.5 driver named "Acme Box" whose body after the sysex block is `rest`.
fn write05(dir: &tempfile::TempDir, header: &str, rest: &str) -> PathBuf {
    let json = format!(
        r#"{{"$schema":"{D05}","device":{{"name":"Acme Box","osc_prefix":"/b"}},"sysex":{{"header":{header},"footer":[247]}}{rest}}}"#
    );
    let path = dir.path().join("box.json");
    std::fs::write(&path, json).unwrap();
    path
}

/// The same, written in 0.4 with the tests' checksum names and converted by `osd::convert`.
fn write(dir: &tempfile::TempDir, header: &str, rest: &str) -> PathBuf {
    let json = format!(
        r#"{{"device":{{"name":"Acme Box","osc_prefix":"/b"}},"sysex":{{"header":{header},"footer":[247]}}{rest}}}"#
    );
    let path = dir.path().join("box.json");
    std::fs::write(&path, common::to05(&json)).unwrap();
    path
}

fn ok(header: &str, rest: &str) -> (tempfile::TempDir, osd::Device) {
    let dir = common::temp("checksum-lenient");
    let path = write(&dir, header, rest);
    let dev = load(&path).unwrap_or_else(|e| panic!("driver refused at load: {e:#}"));
    (dir, dev)
}

fn tokens(json: &str) -> Vec<osd::FrameToken> {
    common::frame(json)
}

// ---- a checksum osd cannot read refuses the load, in every place ----

#[test]
fn a_checksum_osd_cannot_read_refuses_the_load_in_every_frame_place_with_osds_fault() {
    let forms = [
        r#"{"checksum":"crc16","from":5}"#, r#"{"checksum":"twos","from":5}"#, r#"{"checksum":"twos7"}"#,
        r#"{"checksum":"twos7","from":"x"}"#, r#"{"checksum":"sum7","from":5,"length":"y"}"#, r#"{"checksum":"xor7","from":-1}"#,
    ];
    for bad in forms {
        for rest in [
            format!(r#","commands":[{{"osc":"/c","frame":[1,{bad}]}}]"#),
            format!(r#","params":{{"value_type":"u7","set_frame":[1,{bad}],"get_frame":[1],"entries":[]}}"#),
            format!(r#","params":{{"value_type":"u7","set_frame":[1],"get_frame":[1,{bad}],"entries":[]}}"#),
            format!(r#","identity":{{"request":[240,65,1,{bad},247],"reply":[240]}}"#),
            format!(r#","programs":{{"program_change":false,"before":[[240,1,{bad},247]],"banks":[{{"name":"A","size":4}}]}}"#),
            format!(r#","programs":{{"banks":[{{"name":"A","size":4}}],"dump":{{"kind":"request","request":[240,1,{bad},247]}}}}"#),
        ] {
            let dir = common::temp("checksum-refused");
            let path = write05(&dir, ROLAND, &rest);
            let err = load(&path).expect_err(&rest);
            assert!(err.downcast_ref::<osd::Fault>().is_some(), "{bad} in {rest}: the cause is osd's fault: {err:#}");
        }
    }
}

#[test]
fn a_checksum_that_osd_reads_but_no_frame_could_compute_still_loads_and_the_other_commands_work() {
    let (_d, dev) = ok(ROLAND, r#","commands":[{"osc":"/c","frame":[1,"{checksum:twos7@9}"]},{"osc":"/fine","frame":[2,3]}]"#);
    assert_eq!(dev.commands.as_ref().map(Vec::len), Some(2));
    let out = encode(dev.clone(), OscMessage { addr: full_osc_path(&dev, "/fine"), args: vec![] });
    assert_eq!(out, vec![vec![0xF0, 0x41, 0x10, 0x42, 0x12, 2, 3, 0xF7]]);
    let bad = encode(dev.clone(), OscMessage { addr: full_osc_path(&dev, "/c"), args: vec![] });
    assert!(bad.is_empty(), "nothing is sent for the incoherent one");
}

#[test]
fn the_other_parameters_and_commands_still_work_beside_an_incoherent_checksum() {
    let (_d, d) = ok(ROLAND, r#","commands":[{"osc":"/bad","frame":[1,"{checksum:twos7@9}"]}],
        "params":{"value_type":"u7","set_frame":["{p}","{val}"],"get_frame":["{p}"],
        "entries":[{"osc":"/cutoff","pr":0,"p":16,"c":2,"r":0,"range":[0,127]}]}"#);
    let set = encode(d.clone(), OscMessage { addr: full_osc_path(&d, "/cutoff"), args: vec![OscType::Int(5)] });
    assert_eq!(set, vec![vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x10, 0x05, 0xF7]]);
}

#[test]
fn programs_and_dump_name_the_driver_and_the_block_when_a_checksum_cannot_be_computed() {
    let (_d, d) = ok("[]", r#","programs":{"banks":[{"name":"A","size":64}],
        "before":[[240,65,"{checksum:twos7@9}",247]],
        "dump":{"kind":"request","request":[240,62,"{index}","{checksum:sum7@9}",247]}}"#);
    for err in [
        programs::recall(&d, "A", 1, Some(2)).expect_err("the before frame holds an incoherent checksum"),
        programs::dump(&d, Some(("A", 6)), None).expect_err("the dump request holds an incoherent checksum"),
    ] {
        assert!(err.contains("Acme Box"), "the error names the driver: {err}");
        assert!(err.contains("programs"), "the error names the block: {err}");
        assert!(err.contains("checksum"), "the error names the checksum: {err}");
    }
}

#[test]
fn the_identity_request_names_its_block_when_its_checksum_cannot_be_computed() {
    let (_d, d) = ok("[]", r#","identity":{"request":[240,65,"{dev}",6,"{checksum:twos7@9}",247],
        "device_id":{"default":16},"reply":[240,65]}"#);
    let err = osc_bridge::detect::probe_of(d.identity.as_ref().unwrap()).expect_err("an incoherent checksum").to_string();
    assert!(err.contains("identity.request") && err.contains("checksum"), "{err}");
}

#[test]
fn an_offset_past_the_bytes_written_fails_at_build_naming_the_driver() {
    let (_d, d) = ok("[]", r#","programs":{"banks":[{"name":"A","size":4}],
        "dump":{"kind":"request","request":[240,1,"{checksum:twos7@2}",247]}}"#);
    let err = programs::dump(&d, Some(("A", 1)), None).expect_err("offset 2 is the checksum's own place");
    assert!(err.contains("Acme Box") && err.contains("checksum"), "{err}");
    let (_d, d) = ok("[]", r#","programs":{"banks":[{"name":"A","size":4}],
        "dump":{"kind":"request","request":[240,1,"{checksum:twos7@1+5}",247]}}"#);
    let err = programs::dump(&d, Some(("A", 1)), None).expect_err("5 bytes from 1 run past");
    assert!(err.contains("checksum"), "{err}");
}

#[test]
fn a_coherent_checksum_computes_as_before_in_a_driver_that_also_holds_an_incoherent_one() {
    let (_d, d) = ok(ROLAND, r#","commands":[{"osc":"/gs","frame":[64,0,127,0,"{checksum:twos7@5}"]},
        {"osc":"/bad","frame":[1,"{checksum:twos7@9}"]}]"#);
    let out = encode(d.clone(), OscMessage { addr: full_osc_path(&d, "/gs"), args: vec![] });
    assert_eq!(out, vec![vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7]]);
}

// ---- the static check lists the problems ----

#[test]
fn checksum_problems_lists_what_no_frame_could_compute() {
    // Header of 5 bytes, one literal: the checksum sits at offset 6.
    for bad in ["{checksum:twos7@6}", "{checksum:twos7@7}", "{checksum:sum7@5+2}", "{checksum:xor7@0+99}"] {
        let t = tokens(&format!(r#"[1,"{bad}"]"#));
        let problems = checksum_problems(&t, 5);
        assert_eq!(problems.len(), 1, "{bad}: {problems:?}");
        assert!(problems[0].contains("checksum"), "{bad}: {problems:?}");
    }
    for fine in ["{checksum:twos7@5}", "{checksum:xor7@0+6}", "{checksum:sum7@5+1}"] {
        let t = tokens(&format!(r#"[1,"{fine}"]"#));
        assert!(checksum_problems(&t, 5).is_empty(), "{fine}");
    }
}

#[test]
fn checksum_problems_counts_each_token_and_the_earlier_checksum_byte() {
    // The second token sits at offset 7: +3 from 5 runs past, +2 does not.
    let t = tokens(r#"[1,"{checksum:sum7@5}","{checksum:sum7@5+3}"]"#);
    assert_eq!(checksum_problems(&t, 5).len(), 1);
    assert!(checksum_problems(&tokens(r#"[1,"{checksum:sum7@5}","{checksum:sum7@5+2}"]"#), 5).is_empty());
    // Two incoherent tokens, two problems.
    assert_eq!(checksum_problems(&tokens(r#"["{checksum:xor7@0+99}","{checksum:xor7@1+99}"]"#), 0).len(), 2);
    // A whole message (base 0): offset 2 is the checksum's own place.
    assert_eq!(checksum_problems(&tokens(r#"[240,1,"{checksum:twos7@2}",247]"#), 0).len(), 1);
}

#[test]
fn a_length_past_a_variable_size_is_not_a_static_problem() {
    let t = tokens(r#"[240,"{family}","{checksum:twos7@1+9}",247]"#);
    assert!(checksum_problems(&t, 0).is_empty(), "the build decides");
}

// ---- lint and inspect warn ----

fn bin(args: &[&str], path: &std::path::Path) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_osc-bridge")).args(args).arg(path).output().unwrap()
}

fn warn_lines(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.contains("WARN")).collect()
}

const MANY: &str = r#","commands":[{"osc":"/c","frame":[1,"{checksum:twos7@6}"]}],
    "params":{"value_type":"u7","set_frame":["{p}","{checksum:xor7@0+99}"],"get_frame":["{p}","{checksum:twos7@0+99}"],
    "entries":[{"osc":"/cutoff","pr":0,"p":16,"c":2,"r":0,"range":[0,127]}]},
    "programs":{"banks":[{"name":"A","size":4}],"dump":{"kind":"request","request":[240,1,"{checksum:twos7@2}",247]}}"#;

#[test]
fn lint_warns_per_problem_naming_the_place_and_succeeds() {
    let dir = common::temp("checksum-lint");
    let path = write(&dir, ROLAND, MANY);
    let run = bin(&["lint"], &path);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "lint succeeds:\n{stderr}");
    let warns = warn_lines(&stderr);
    assert_eq!(warns.len(), 4, "{stderr}");
    for place in ["/c", "/cutoff", "param/get", "programs"] {
        assert!(warns.iter().any(|l| l.contains("checksum") && l.contains(place)), "no WARN at {place}:\n{stderr}");
    }
    assert!(stderr.contains("4 warning(s)"), "{stderr}");
}

#[test]
fn lint_counts_checksum_warnings_with_the_others() {
    let dir = common::temp("checksum-lint-count");
    let rest = r#","commands":[{"osc":"/c","frame":[1,"{checksum:twos7@6}"]},{"osc":"/s","frame":[1],"script":"return [];"}]"#;
    let path = write(&dir, ROLAND, rest);
    let run = bin(&["lint"], &path);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains("2 warning(s)"), "{stderr}");
}

#[test]
fn lint_of_a_clean_driver_says_no_checksum_warning() {
    let dir = common::temp("checksum-lint-clean");
    let path = write(&dir, ROLAND, r#","commands":[{"osc":"/c","frame":[1,"{checksum:twos7@5}"]}]"#);
    let run = bin(&["lint"], &path);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(warn_lines(&stderr).is_empty(), "{stderr}");
}

#[test]
fn inspect_prints_the_same_warn_lines_on_stderr_and_succeeds() {
    let dir = common::temp("checksum-inspect");
    let path = write(&dir, ROLAND, MANY);
    let run = bin(&["inspect"], &path);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "inspect succeeds:\n{stderr}");
    let warns = warn_lines(&stderr);
    assert_eq!(warns.len(), 4, "{stderr}");
    let lint = bin(&["lint"], &path);
    let lint_warns: Vec<String> = warn_lines(&String::from_utf8_lossy(&lint.stderr)).iter().map(|s| s.to_string()).collect();
    let inspect_warns: Vec<String> = warns.iter().map(|s| s.to_string()).collect();
    assert_eq!(inspect_warns, lint_warns, "the same lines");
}
