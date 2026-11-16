//! Every script engine's `print` goes to the log (standard error), never to
//! standard output, where a local MCP session speaks; the Lua checksum helpers
//! give the byte `frame::checksum_byte` gives; one constructor makes every
//! engine.
//!
//! The `print` check runs in a child process: this test runs itself again with
//! `SCRIPT_PRINT_RUN` set, and reads the child's two output streams.

use osc_bridge::frame::checksum_byte;
use osc_bridge::scripting::ScriptEngine;
use osd::Algorithm;
use serde_json::json;

const MARKER: &str = "marker-from-a-printing-script";

#[test]
fn print_of_an_engine_without_a_driver_writes_to_the_log_not_to_standard_output() {
    if std::env::var_os("SCRIPT_PRINT_RUN").is_some() {
        return print_in_child();
    }
    let run = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "script_log_and_checksum::print_of_an_engine_without_a_driver_writes_to_the_log_not_to_standard_output", "--nocapture", "--test-threads=1"])
        .env("SCRIPT_PRINT_RUN", "1")
        .output()
        .unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    assert!(run.status.success(), "the run failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("running 1 test\n"), "the child played one test and no other:\n{stdout}\n{stderr}");
    assert!(!stdout.contains(MARKER), "a clean standard output:\n{stdout}");
    assert!(stderr.contains(MARKER), "the print is in the log:\n{stderr}");
}

/// An identity script, as detection and programs run it, that prints.
fn print_in_child() {
    let engine = ScriptEngine::new().unwrap();
    let ret = engine
        .call_json(&format!("print('{MARKER}', 1, nil) return true"), &json!({ "reply": [1, 2] }))
        .unwrap();
    assert_eq!(ret, json!(true));
}

fn lua(code: &str, bytes: &[i64]) -> i64 {
    let e = ScriptEngine::new().unwrap();
    e.eval_transform(&format!("return {code}({{{}}})", bytes.iter().map(i64::to_string).collect::<Vec<_>>().join(",")), 0).unwrap()
}

#[test]
fn lua_checksum_helpers_give_the_byte_of_frame_checksum_byte() {
    let inputs: [&[u8]; 6] = [
        &[],
        &[0x12, 0x34, 0x56],
        &[0xFF],
        &[0x80, 0x81, 0xFE, 0x7F],
        &[0xF0, 0x7E, 0x00, 0xFF, 0x80],
        &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    ];
    for bytes in inputs {
        let ints: Vec<i64> = bytes.iter().map(|b| i64::from(*b)).collect();
        assert_eq!(lua("ob.checksum_xor", &ints), i64::from(checksum_byte(Algorithm::Xor7, bytes)), "xor of {bytes:02X?}");
        assert_eq!(lua("ob.checksum_sum", &ints), i64::from(checksum_byte(Algorithm::Sum7, bytes)), "sum of {bytes:02X?}");
    }
}

#[test]
fn only_the_one_constructor_creates_a_lua_engine() {
    fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() { rs_files(&p, out) } else if p.extension().is_some_and(|e| e == "rs") { out.push(p) }
        }
    }
    let mut files = Vec::new();
    rs_files(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let makers: Vec<_> = files.iter()
        .filter(|f| std::fs::read_to_string(f).unwrap().lines().any(|l| {
            let l = l.trim_start();
            !l.starts_with("//") && (l.contains("Lua::new") || l.contains("Lua::unsafe_new"))
        }))
        .collect();
    assert_eq!(makers.len(), 1, "one site creates a Lua engine: {makers:?}");
    assert!(makers[0].ends_with("scripting.rs"), "{makers:?}");
}
