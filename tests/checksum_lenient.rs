//! OB-20q.3: a checksum token the engine cannot compute never refuses a driver at load.
//! It fails where it is used (the build of the frame, naming the driver, the origin of the
//! frame and the token) and `lint` and `inspect` warn about it (OSD 0.4 accepts any
//! `frame_token` of the form `^\{.+\}$`; a document valid in an older version stays readable).
//!
//! Checksum definitions: see `tests/checksum.rs` (Electra One preset format).

mod common;

use osc_bridge::device::{Device, FrameToken};
use osc_bridge::frame::checksum_problems;
use osc_bridge::programs;
use osc_bridge::runtime::encode;
use rosc::{OscMessage, OscType};
use std::path::PathBuf;

const ROLAND: &str = "[240,65,16,66,18]";
const BAD_FORMS: [&str; 4] = ["{checksum:twos7@abc}", "{checksum:foo@2}", "{checksum:twos7@9}", "{checksum:twos7@0+50}"];

/// Write a driver named "Acme Box" whose body after the sysex block is `rest`.
fn write(dir: &tempfile::TempDir, header: &str, rest: &str) -> PathBuf {
    let json = format!(
        r#"{{"device":{{"name":"Acme Box","osc_prefix":"/b"}},"sysex":{{"header":{header},"footer":[247]}}{rest}}}"#
    );
    let path = dir.path().join("box.json");
    std::fs::write(&path, json).unwrap();
    path
}

fn ok(header: &str, rest: &str) -> (tempfile::TempDir, Device) {
    let dir = common::temp("checksum-lenient");
    let path = write(&dir, header, rest);
    let dev = Device::load(&path).unwrap_or_else(|e| panic!("driver refused at load: {e:#}"));
    (dir, dev)
}

fn cmd_rest(token: &str) -> String {
    format!(r#","commands":[{{"osc":"/c","frame":[1,"{token}"]}},{{"osc":"/fine","frame":[2,3]}}]"#)
}

fn tokens(json: &str) -> Vec<FrameToken> {
    serde_json::from_str(json).unwrap()
}

// ---- criteria 1, 6, 7: nothing refuses the load ----

#[test]
fn criterion_1_6_7_a_driver_with_a_bare_checksum_loads_and_its_other_commands_work() {
    let (_d, dev) = ok(ROLAND, &cmd_rest("{checksum}"));
    let out = encode(dev.clone(), OscMessage { addr: dev.full_osc_path("/fine"), args: vec![] });
    assert_eq!(out, vec![vec![0xF0, 0x41, 0x10, 0x42, 0x12, 2, 3, 0xF7]]);
}

#[test]
fn criterion_6_7_the_four_measured_forms_all_load() {
    for token in BAD_FORMS {
        let (_d, dev) = ok(ROLAND, &cmd_rest(token));
        assert_eq!(dev.commands.len(), 2, "{token}");
    }
}

#[test]
fn criterion_6_every_unreadable_or_incoherent_form_loads_in_every_frame_place() {
    let forms = [
        "{checksum}", "{checksum:crc16@5}", "{checksum:twos@5}", "{checksum:twos7}", "{checksum:twos7@}",
        "{checksum:twos7@x}", "{checksum:twos7@5+}", "{checksum:@5}", "{checksum:sum7@5+y}",
        "{checksum:twos7@6}", "{checksum:twos7@7}", "{checksum:sum7@5+2}", "{checksum:xor7@0+99}",
    ];
    for bad in forms {
        for rest in [
            format!(r#","commands":[{{"osc":"/c","frame":[1,"{bad}"]}}]"#),
            format!(r#","params":{{"value_type":"u7","set_frame":[1,"{bad}"],"get_frame":[1],"entries":[]}}"#),
            format!(r#","params":{{"value_type":"u7","set_frame":[1],"get_frame":[1,"{bad}"],"entries":[]}}"#),
            format!(r#","identity":{{"request":[240,65,1,"{bad}",247],"reply":[240]}}"#),
            format!(r#","programs":{{"program_change":false,"before":[[240,1,"{bad}",247]],"banks":[{{"name":"A","size":4}}]}}"#),
            format!(r#","programs":{{"banks":[{{"name":"A","size":4}}],"dump":{{"kind":"request","request":[240,1,"{bad}",247]}}}}"#),
            format!(r#","commands":[{{"osc":"/c","frame":[1,"{{checksum:sum7@5}}","{bad}"]}}]"#),
        ] {
            let _ = ok(ROLAND, &rest);
        }
    }
}

#[test]
fn criterion_6_the_other_parameters_and_commands_still_work_beside_a_bad_token() {
    let (_d, d) = ok(ROLAND, r#","commands":[{"osc":"/bad","frame":[1,"{checksum}"]}],
        "params":{"value_type":"u7","set_frame":["{p}","{val}"],"get_frame":["{p}"],
        "entries":[{"osc":"/cutoff","pr":0,"p":16,"c":2,"r":0,"range":[0,127]}]}"#);
    let set = encode(d.clone(), OscMessage { addr: d.full_osc_path("/cutoff"), args: vec![OscType::Int(5)] });
    assert_eq!(set, vec![vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x10, 0x05, 0xF7]]);
}

// ---- criteria 2, 7, 8: the build fails, naming the driver, the origin and the token ----

/// Child of the tests below: encode one OSC message to the driver of `CHK_FILE`, print how
/// many messages came out. Its standard error carries the build error.
#[test]
fn child_encode_one_message() {
    let (Ok(file), Ok(addr)) = (std::env::var("CHK_FILE"), std::env::var("CHK_ADDR")) else { return };
    let dev = Device::load(std::path::Path::new(&file)).unwrap();
    let args = if addr.ends_with("/param/get") {
        vec![OscType::Int(0), OscType::Int(16), OscType::Int(2), OscType::Int(0)]
    } else {
        vec![OscType::Int(5)]
    };
    let out = encode(dev.clone(), OscMessage { addr, args });
    println!("SENT:{}", out.len());
}

/// Run the child on a driver; return its standard error and the number of messages sent.
fn build_error(header: &str, rest: &str, addr: &str) -> (String, usize) {
    let dir = common::temp("checksum-lenient-child");
    let path = write(&dir, header, rest);
    let run = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_encode_one_message", "--nocapture", "--test-threads=1"])
        .env("CHK_FILE", &path)
        .env("CHK_ADDR", addr)
        .output()
        .unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr).to_string());
    assert!(run.status.success(), "the child failed:\n{stdout}\n{stderr}");
    let sent = stdout.lines().find_map(|l| l.rsplit_once("SENT:").map(|(_, n)| n.trim())).unwrap_or_else(|| panic!("no SENT line:\n{stdout}\n{stderr}"));
    (stderr, sent.parse().unwrap())
}

#[test]
fn criterion_2_7_8_a_command_with_each_measured_form_fails_at_build_naming_driver_command_and_token() {
    for token in BAD_FORMS {
        let (err, sent) = build_error(ROLAND, &cmd_rest(token), "/b/c");
        assert_eq!(sent, 0, "nothing is sent for {token}");
        assert!(err.contains(token), "the error names {token}:\n{err}");
        assert!(err.contains("Acme Box"), "the error names the driver for {token}:\n{err}");
        assert!(err.contains("/b/c"), "the error names the command's OSC address for {token}:\n{err}");
    }
}

#[test]
fn criterion_2_8_an_unreadable_form_names_the_three_algorithms() {
    for token in ["{checksum}", "{checksum:foo@2}", "{checksum:twos7@abc}"] {
        let (err, sent) = build_error(ROLAND, &cmd_rest(token), "/b/c");
        assert_eq!(sent, 0);
        for algorithm in ["twos7", "sum7", "xor7"] {
            assert!(err.contains(algorithm), "{token}: the error names {algorithm}:\n{err}");
        }
    }
}

#[test]
fn criterion_2_8_a_parameter_set_and_get_fail_at_build_naming_driver_address_and_token() {
    let bad = "{checksum}";
    let entries = r#""entries":[{"osc":"/cutoff","pr":0,"p":16,"c":2,"r":0,"range":[0,127]}]"#;
    let set = format!(r#","params":{{"value_type":"u7","set_frame":["{{p}}","{{val}}","{bad}"],"get_frame":["{{p}}"],{entries}}}"#);
    let (err, sent) = build_error(ROLAND, &set, "/b/cutoff");
    assert_eq!(sent, 0);
    assert!(err.contains(bad) && err.contains("Acme Box") && err.contains("/b/cutoff"), "{err}");

    let get = format!(r#","params":{{"value_type":"u7","set_frame":["{{p}}","{{val}}"],"get_frame":["{{p}}","{bad}"],{entries}}}"#);
    let (err, sent) = build_error(ROLAND, &get, "/b/param/get");
    assert_eq!(sent, 0);
    assert!(err.contains(bad) && err.contains("Acme Box") && err.contains("/b/param/get"), "{err}");
}

#[test]
fn criterion_2_8_programs_and_dump_name_driver_block_and_token() {
    let bad = "{checksum:foo@1}";
    let (_d, d) = ok("[]", &format!(r#","programs":{{"banks":[{{"name":"A","size":64}}],
        "before":[[240,65,"{bad}",247]],
        "dump":{{"kind":"request","request":[240,62,"{{index}}","{bad}",247]}}}}"#));
    for err in [
        programs::recall(&d, "A", 1, Some(2)).expect_err("the before frame holds an unreadable token"),
        programs::dump(&d, Some(("A", 6)), None).expect_err("the dump request holds an unreadable token"),
    ] {
        assert!(err.contains(bad), "{err}");
        assert!(err.contains("Acme Box"), "the error names the driver: {err}");
        assert!(err.contains("programs"), "the error names the block: {err}");
        for algorithm in ["twos7", "sum7", "xor7"] {
            assert!(err.contains(algorithm), "{err}");
        }
    }
}

#[test]
fn criterion_2_8_the_identity_request_names_its_block_and_the_token() {
    let bad = "{checksum:foo@1}";
    let (_d, d) = ok("[]", &format!(r#","identity":{{"request":[240,65,"{{dev}}",6,"{bad}",247],
        "device_id":{{"default":16}},"reply":[240,65]}}"#));
    let err = osc_bridge::detect::probe_of(d.identity.as_ref().unwrap()).expect_err("an unreadable token").to_string();
    assert!(err.contains(bad), "{err}");
    assert!(err.contains("identity.request"), "{err}");
}

#[test]
fn criterion_3_7_an_offset_past_the_bytes_written_still_fails_at_build_with_the_token() {
    let (_d, d) = ok("[]", r#","programs":{"banks":[{"name":"A","size":4}],
        "dump":{"kind":"request","request":[240,1,"{checksum:twos7@2}",247]}}"#);
    let err = programs::dump(&d, Some(("A", 1)), None).expect_err("offset 2 is the checksum's own place");
    assert!(err.contains("{checksum:twos7@2}") && err.contains("Acme Box"), "{err}");
    let (_d, d) = ok("[]", r#","programs":{"banks":[{"name":"A","size":4}],
        "dump":{"kind":"request","request":[240,1,"{checksum:twos7@1+5}",247]}}"#);
    let err = programs::dump(&d, Some(("A", 1)), None).expect_err("5 bytes from 1 run past");
    assert!(err.contains("{checksum:twos7@1+5}"), "{err}");
}

#[test]
fn criterion_9_a_well_formed_coherent_token_computes_as_before_in_a_driver_that_also_holds_a_bad_one() {
    let (_d, d) = ok(ROLAND, r#","commands":[{"osc":"/gs","frame":[64,0,127,0,"{checksum:twos7@5}"]},
        {"osc":"/bad","frame":[1,"{checksum}"]}]"#);
    let out = encode(d.clone(), OscMessage { addr: d.full_osc_path("/gs"), args: vec![] });
    assert_eq!(out, vec![vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7]]);
}

// ---- criterion 10: the static check lists the problems ----

#[test]
fn criterion_10_checksum_problems_lists_what_no_frame_could_compute() {
    // Header of 5 bytes, one literal: the checksum sits at offset 6.
    for bad in ["{checksum}", "{checksum:foo@2}", "{checksum:twos7@abc}", "{checksum:twos7@6}", "{checksum:twos7@7}",
                "{checksum:sum7@5+2}", "{checksum:xor7@0+99}"] {
        let t = tokens(&format!(r#"[1,"{bad}"]"#));
        let problems = checksum_problems(&t, 5);
        assert_eq!(problems.len(), 1, "{bad}: {problems:?}");
        assert!(problems[0].contains(bad), "{bad}: {problems:?}");
    }
    for fine in ["{checksum:twos7@5}", "{checksum:xor7@0+6}", "{checksum:sum7@5+1}"] {
        let t = tokens(&format!(r#"[1,"{fine}"]"#));
        assert!(checksum_problems(&t, 5).is_empty(), "{fine}");
    }
}

#[test]
fn criterion_10_checksum_problems_counts_each_token_and_the_earlier_checksum_byte() {
    // The second token sits at offset 7: +3 from 5 runs past, +2 does not.
    let t = tokens(r#"[1,"{checksum:sum7@5}","{checksum:sum7@5+3}"]"#);
    let problems = checksum_problems(&t, 5);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("{checksum:sum7@5+3}"));
    assert!(checksum_problems(&tokens(r#"[1,"{checksum:sum7@5}","{checksum:sum7@5+2}"]"#), 5).is_empty());
    // Two bad tokens, two problems.
    assert_eq!(checksum_problems(&tokens(r#"["{checksum}","{checksum:foo@1}"]"#), 0).len(), 2);
    // A whole message (base 0): offset 2 is the checksum's own place.
    assert_eq!(checksum_problems(&tokens(r#"[240,1,"{checksum:twos7@2}",247]"#), 0).len(), 1);
}

#[test]
fn criterion_10_a_length_past_a_variable_size_is_not_a_static_problem() {
    let t = tokens(r#"[240,"{family}","{checksum:twos7@1+9}",247]"#);
    assert!(checksum_problems(&t, 0).is_empty(), "the build decides");
}

// ---- criterion 11: lint and inspect warn ----

fn bin(args: &[&str], path: &std::path::Path) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_osc-bridge")).args(args).arg(path).output().unwrap()
}

fn warn_lines(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.contains("WARN")).collect()
}

const MANY: &str = r#","commands":[{"osc":"/c","frame":[1,"{checksum}"]}],
    "params":{"value_type":"u7","set_frame":["{p}","{checksum:foo@1}"],"get_frame":["{p}","{checksum:twos7@0+99}"],
    "entries":[{"osc":"/cutoff","pr":0,"p":16,"c":2,"r":0,"range":[0,127]}]},
    "programs":{"banks":[{"name":"A","size":4}],"dump":{"kind":"request","request":[240,1,"{checksum:twos7@2}",247]}}"#;

#[test]
fn criterion_11_lint_warns_per_problem_naming_token_and_place_and_succeeds() {
    let dir = common::temp("checksum-lint");
    let path = write(&dir, ROLAND, MANY);
    let run = bin(&["lint"], &path);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "lint succeeds:\n{stderr}");
    let warns = warn_lines(&stderr);
    assert_eq!(warns.len(), 4, "{stderr}");
    for (token, place) in [("{checksum}", "/c"), ("{checksum:foo@1}", "/cutoff"), ("{checksum:twos7@0+99}", "param/get"), ("{checksum:twos7@2}", "programs")] {
        assert!(warns.iter().any(|l| l.contains(token) && l.contains(place)), "no WARN for {token} at {place}:\n{stderr}");
    }
    assert!(stderr.contains("4 warning(s)"), "{stderr}");
    assert!(!stderr.contains("scripted-fallback"), "{stderr}");
}

#[test]
fn criterion_11_lint_counts_checksum_warnings_with_the_others() {
    let dir = common::temp("checksum-lint-count");
    let rest = r#","commands":[{"osc":"/c","frame":[1,"{checksum}"]},{"osc":"/s","frame":[1],"script":"return [];"}]"#;
    let path = write(&dir, ROLAND, rest);
    let run = bin(&["lint"], &path);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains("2 warning(s)"), "{stderr}");
}

#[test]
fn criterion_11_lint_of_a_clean_driver_says_no_checksum_warning() {
    let dir = common::temp("checksum-lint-clean");
    let path = write(&dir, ROLAND, r#","commands":[{"osc":"/c","frame":[1,"{checksum:twos7@5}"]}]"#);
    let run = bin(&["lint"], &path);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(warn_lines(&stderr).is_empty(), "{stderr}");
}

#[test]
fn criterion_11_inspect_prints_the_same_warn_lines_on_stderr_and_succeeds() {
    let dir = common::temp("checksum-inspect");
    let path = write(&dir, ROLAND, MANY);
    let run = bin(&["inspect"], &path);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "inspect succeeds:\n{stderr}");
    let warns = warn_lines(&stderr);
    assert_eq!(warns.len(), 4, "{stderr}");
    for token in ["{checksum}", "{checksum:foo@1}", "{checksum:twos7@0+99}", "{checksum:twos7@2}"] {
        assert!(warns.iter().any(|l| l.contains(token)), "no WARN for {token}:\n{stderr}");
    }
    let lint = bin(&["lint"], &path);
    let lint_warns: Vec<String> = warn_lines(&String::from_utf8_lossy(&lint.stderr)).iter().map(|s| s.to_string()).collect();
    let inspect_warns: Vec<String> = warns.iter().map(|s| s.to_string()).collect();
    assert_eq!(inspect_warns, lint_warns, "the same lines");
}

// ---- criterion 12: the changelog ----

#[test]
fn criterion_12_the_changelog_says_it_under_unreleased() {
    let changelog = std::fs::read_to_string("CHANGELOG.md").unwrap();
    let start = changelog.find(&format!("## [{}]", ["Unre", "leased"].concat())).expect("an Unreleased section");
    let rest = &changelog[start + 1..];
    let unreleased = &rest[..rest.find("\n## [").unwrap_or(rest.len())];
    for w in ["checksum", "loads", "build", "lint", "inspect"] {
        assert!(unreleased.contains(w), "Unreleased should say {w}:\n{unreleased}");
    }
}
