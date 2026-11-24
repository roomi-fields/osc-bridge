//! A linked session goes to the SynthHub server for the descriptor of each of the two devices
//! `install_export` names (`device` and `on`) before it acts, as the other tools do for theirs: a
//! device the person keeps private is not asked for, each name is judged on its own, one device
//! named twice is asked for once, and a descriptor found nowhere gives the unknown-device error
//! naming it, before any MIDI port is opened.
//!
//! No network: the server is a fake one on a local port (`common`).

use crate::common::{catalogue, temp, tool_call, ConfigHome, FakeSynthHub};
use serde_json::{json, Value};

/// The `.json` descriptors the server was asked for, sorted (the maker profile and the notes aside).
fn asked(server: &FakeSynthHub) -> Vec<String> {
    let mut urls: Vec<String> = server.driver_requests().into_iter().map(|s| s.url)
        .filter(|u| u.ends_with(".json") && !u.contains("_profile"))
        .map(|u| u.trim_start_matches("/api/v1/devices/").to_string()).collect();
    urls.sort();
    urls
}

/// One linked session (empty cache, the given private devices) that makes one `install_export`
/// call; the server it asked, and the call's result.
fn install(private: &[&str], args: Value) -> (FakeSynthHub, Value, tempfile::TempDir) {
    let _home = ConfigHome::new();
    let dir = temp("install-export-fetch");
    catalogue(&dir.path().join("server"));
    let server = FakeSynthHub::start(&dir.path().join("server"), None);
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: None,
        default_target: "127.0.0.1:9".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: None,
    };
    let private: Vec<String> = private.iter().map(|p| p.to_string()).collect();
    let input = format!("{}\n", tool_call(1, "install_export", args));
    let mut out = Vec::new();
    osc_bridge::synthub::mcp::serve_with_midi(opts, &server.base, Some(dir.path().join("cache")), &private, input.as_bytes(), &mut out, &crate::common::refusing("install_export_fetch")).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    let result = reply["result"].clone();
    (server, result, dir)
}

fn text(r: &Value) -> String {
    r["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")
}

#[test]
fn install_export_fetches_the_descriptors_of_both_its_devices_before_acting() {
    let (server, _, _dir) = install(&[], json!({"device": "acme/box", "on": "acme/synth"}));
    assert_eq!(asked(&server), ["acme/box.json", "acme/synth.json"]);
}

#[test]
fn install_export_does_not_ask_the_server_for_a_private_device() {
    let (server, _, _dir) = install(&["acme/box", "acme/synth"], json!({"device": "acme/box", "on": "acme/synth"}));
    assert!(asked(&server).is_empty(), "{:?}", asked(&server));
}

#[test]
fn install_export_judges_the_private_filter_on_each_name_separately() {
    let (server, _, _dir) = install(&["acme/box"], json!({"device": "acme/box", "on": "acme/synth"}));
    assert_eq!(asked(&server), ["acme/synth.json"]);
    let (server, _, _dir) = install(&["acme/synth"], json!({"device": "acme/box", "on": "acme/synth"}));
    assert_eq!(asked(&server), ["acme/box.json"]);
}

#[test]
fn install_export_asks_once_for_a_device_that_is_both_its_source_and_its_target() {
    let (server, _, _dir) = install(&[], json!({"device": "acme/box", "on": "acme/box"}));
    assert_eq!(asked(&server), ["acme/box.json"]);
}

#[test]
fn install_export_names_a_device_the_server_does_not_know() {
    let (_server, r, _dir) = install(&[], json!({"device": "acme/ghost", "on": "acme/synth"}));
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text(&r).contains("acme/ghost"), "{}", text(&r));
}

#[test]
fn install_export_names_a_target_found_nowhere_and_opens_no_midi_port() {
    let (server, r, _dir) = install(&[], json!({"device": "acme/box", "on": "acme/ghost"}));
    assert_eq!(asked(&server), ["acme/box.json", "acme/ghost.json"]);
    assert_eq!(r["isError"], json!(true), "{r}");
    let message = text(&r);
    assert!(message.contains("acme/ghost"), "{message}");
    assert!(!message.contains("MIDI:"), "the MIDI port was reached for: {message}");
}
