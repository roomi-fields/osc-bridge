//! A maker request that may be a load for another maker's device waits for the person's consent
//! on a device nobody recognised: the result of `detect_hardware` says which requests were held
//! back, and the tool publishes the argument that gives the consent.

use osc_bridge::capture::{Capture, FORMAT_VERSION};
use osc_bridge::detect::Probe;
use osc_bridge::mcp::{detect_capture_held, hex_bytes};
use serde_json::{json, Value};

const MYSTERY: &str = "Mystery Box:Mystery Box 20:0";
const MINILAB: &str = "Minilab3:Minilab3 MIDI 40:0";

fn probe(tag: u8, usb: &[(u16, u16)], names: &[&str]) -> Probe {
    Probe { request: vec![0xF0, tag, 0xF7], ports: vec![], usb: usb.to_vec(), match_ports: names.iter().map(|s| s.to_string()).collect() }
}

/// A capture of a MiniLab 3 (its USB ids read) and a device nobody knows.
fn capture() -> Capture {
    Capture::parse(&json!({
        "format_version": FORMAT_VERSION,
        "outputs": [MINILAB, MYSTERY],
        "inputs": [MINILAB, MYSTERY],
        "cards": " 6 [Minilab3       ]: USB-Audio - Minilab3\n                      Arturia Minilab3 at usb-0000:00:14.0-2, full speed\n",
        "usbids": [{"card": 6, "text": "1c75:020b\n"}], "clients": "",
        "requests": [], "received": [],
    }).to_string()).unwrap()
}

fn probes() -> Vec<Probe> {
    vec![probe(0x11, &[(0x1c75, 0x020b)], &[]), probe(0x12, &[], &["Elsewhere"]), probe(0x13, &[], &[])]
}

fn held_of(devices: &[Value], port: &str) -> Vec<String> {
    let d = devices.iter().find(|d| d["port"] == port).unwrap_or_else(|| panic!("{port} in {devices:?}"));
    d["held_back"].as_array().unwrap_or_else(|| panic!("held_back of {port}: {d}")).iter()
        .map(|h| h.as_str().unwrap().to_string()).collect()
}

fn hex(tag: u8) -> String {
    hex_bytes(&[0xF0, tag, 0xF7])
}

#[test]
fn the_result_lists_the_maker_requests_held_back_for_each_port_without_consent() {
    let dir = super::common::temp("detect-consent");
    let devices = detect_capture_held(dir.path(), &capture(), &probes(), &[]).unwrap();
    assert_eq!(held_of(&devices, MYSTERY), vec![hex(0x11), hex(0x12), hex(0x13)], "every maker request");
    assert_eq!(held_of(&devices, MINILAB), vec![hex(0x12), hex(0x13)], "its own maker's request is sent");
}

#[test]
fn a_consented_port_has_no_request_held_back_and_the_others_keep_theirs() {
    let dir = super::common::temp("detect-consent");
    let devices = detect_capture_held(dir.path(), &capture(), &probes(), &["mystery".to_string()]).unwrap();
    assert!(held_of(&devices, MYSTERY).is_empty());
    assert_eq!(held_of(&devices, MINILAB).len(), 2);
}

#[test]
fn an_exact_index_consents_one_port() {
    let dir = super::common::temp("detect-consent");
    let devices = detect_capture_held(dir.path(), &capture(), &probes(), &["0".to_string()]).unwrap();
    assert!(held_of(&devices, MINILAB).is_empty());
    assert_eq!(held_of(&devices, MYSTERY).len(), 3);
}

#[test]
fn a_consent_naming_several_ports_or_none_is_refused_naming_the_ports() {
    let dir = super::common::temp("detect-consent");
    let several = detect_capture_held(dir.path(), &capture(), &probes(), &["box".to_string(), "mini".to_string()]);
    assert!(several.is_ok(), "one port each");
    let both = detect_capture_held(dir.path(), &capture(), &probes(), &["20:0".to_string(), "40:0".to_string()]);
    assert!(both.is_ok());
    let err = detect_capture_held(dir.path(), &capture(), &probes(), &["m".to_string()]).expect_err("Minilab3 and Mystery both hold 'm'");
    assert!(err.contains(MINILAB) && err.contains(MYSTERY), "{err}");
    let err = detect_capture_held(dir.path(), &capture(), &probes(), &["nothing".to_string()]).expect_err("no port");
    assert!(err.contains("nothing") && err.contains(MYSTERY), "{err}");
}

#[test]
fn detect_hardware_publishes_the_consent_argument_and_the_safe_list_in_its_description() {
    let dir = super::common::temp("detect-consent");
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(dir.path().to_path_buf()),
        default_target: "127.0.0.1:9000".parse().unwrap(),
        online: false,
        private: vec![],
    };
    let input = format!("{}\n", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, "http://127.0.0.1:1", None, input.as_bytes(), &mut out).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    let tool = reply["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "detect_hardware").unwrap().clone();
    let consent = &tool["inputSchema"]["properties"]["consent_ports"];
    assert_eq!(consent["type"], "array", "{consent}");
    assert_eq!(consent["items"]["type"], "string", "{consent}");
    let what = consent["description"].as_str().unwrap().to_lowercase();
    assert!(what.contains("person") && what.contains("ask"), "the agent must have asked the person: {what}");
    let description = tool["description"].as_str().unwrap().to_lowercase();
    for word in ["universal", "usb", "consent", "held back"] {
        assert!(description.contains(word), "{word} missing from {description}");
    }
    assert!(!description.contains("harmless"), "a maker request is no longer called harmless: {description}");
    let device = &tool["outputSchema"]["properties"]["devices"]["items"]["properties"];
    assert_eq!(device["held_back"]["type"], "array", "{device}");
}
