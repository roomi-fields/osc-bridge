//! When the SynthHub server marks this osc-bridge as deprecated
//! (`Deprecation` with `Link: <url>; rel="deprecation"`, RFC 9745),
//! osc-bridge says once per run, on standard error and in the result of the
//! next MCP tool call, that a newer osc-bridge exists and where. The MCP
//! protocol on standard output carries nothing else.
//!
//! The notice is once per run, so the run is a child process: this test runs
//! itself again with `NEWER_NOTICE_RUN` set, and reads its standard error.

mod common;

use common::{catalogue, mcp_session, temp, texts, tool_call, FakeSynthHub};
use serde_json::json;

const NEWER: &str = "https://synthhub.roomi-fields.com/osc-bridge/";

#[test]
fn a_newer_osc_bridge_is_announced_once_per_run() {
    if std::env::var_os("NEWER_NOTICE_RUN").is_some() {
        return one_run();
    }
    let run = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "a_newer_osc_bridge_is_announced_once_per_run", "--nocapture", "--test-threads=1"])
        .env("NEWER_NOTICE_RUN", "1")
        .output()
        .unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    assert!(run.status.success(), "the run failed:\n{stdout}\n{stderr}");
    assert_eq!(stderr.matches("newer osc-bridge").count(), 1, "said once on standard error:\n{stderr}");
    assert!(stderr.contains(NEWER), "{stderr}");
    assert!(!stdout.contains(NEWER), "nothing on standard output:\n{stdout}");
}

/// One run: an MCP session, then a driver fetched outside it, every reply
/// from the server marked as deprecated.
fn one_run() {
    let dir = temp("newer");
    let (root, cache) = (dir.join("server"), dir.join("cache"));
    catalogue(&root);
    let server = FakeSynthHub::start(&root, Some(NEWER));

    let replies = mcp_session(&server.base, &cache, &[
        common::initialize(),
        tool_call(1, "list_routes", json!({ "slug": "box" })),
        tool_call(2, "get_device_docs", json!({ "slug": "synth" })),
    ]);
    assert_eq!(replies.len(), 3, "{replies:?}");
    assert!(!replies[0].to_string().contains(NEWER), "not in initialize: {}", replies[0]);
    let first = texts(&replies[1]);
    assert!(first.iter().any(|t| t.contains("newer osc-bridge") && t.contains(NEWER)),
        "the next tool call's result says it: {first:?}");
    assert_ne!(replies[1]["result"]["isError"], true, "{}", replies[1]);
    assert!(texts(&replies[2]).iter().all(|t| !t.contains(NEWER)), "once: {}", replies[2]);

    // Another client of the same run says nothing more.
    let unused = dir.join("cache2");
    osc_bridge::online::Online::new(&server.base).fetch_driver("acme/synth", &unused).unwrap();
    std::fs::remove_dir_all(dir).ok();
}
