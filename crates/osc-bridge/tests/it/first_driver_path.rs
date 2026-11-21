//! The path of a device with no driver to its first driver, through the MCP server: the two
//! tools that carry it say so in their descriptions, and a call naming a device by the name the
//! person gave its fingerprint goes to the service as it is (the local copy does not refuse it
//! for lacking a driver). The service is a fake one on a local port.

use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const ID: &str = "0123456789abcdef0123456789abcdef";

/// A local service: the account delivery, the trust, and every tool call of the account logged.
fn fake_service() -> (String, Arc<Mutex<Vec<Value>>>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let calls: Arc<Mutex<Vec<Value>>> = Arc::default();
    let log = calls.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let url = req.url().to_string();
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
            let (status, reply) = if url == "/v1/access/challenge" {
                (200, json!({"challenge":"fake","algorithm":"sha256","bits":4,"deliver":"/v1/access/deliver"}))
            } else if url == "/v1/access/deliver" {
                (200, json!({ "id": ID, "user_id": ID, "mcp_path": format!("/u/{ID}/mcp"), "tier": "free" }))
            } else if url == "/v1/trust" {
                (200, json!({"devices":{}}))
            } else if url == format!("/u/{ID}/mcp") {
                log.lock().unwrap().push(serde_json::from_str(&body).unwrap_or(Value::Null));
                (200, json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"received"}]}}))
            } else {
                (404, json!({}))
            };
            let _ = req.respond(tiny_http::Response::from_string(reply.to_string()).with_status_code(status));
        }
    });
    (base, calls)
}

/// One linked session (no driver here) making `request`; the tool calls the service received.
fn linked(request: Value) -> (Value, Vec<Value>) {
    let _home = super::common::ConfigHome::new();
    let dir = super::common::temp("first-driver");
    let (base, calls) = fake_service();
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: None,
        default_target: "127.0.0.1:9".parse().unwrap(),
        online: true,
        private: vec![],
    };
    let input = format!("{request}\n");
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, &base, Some(dir.path().join("cache")), input.as_bytes(), &mut out).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    let seen = calls.lock().unwrap().clone();
    (reply, seen)
}

fn description_of(tool: &str) -> String {
    let (reply, _) = linked(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
    reply["result"]["tools"].as_array().unwrap().iter()
        .find(|t| t["name"] == tool).unwrap()["description"].as_str().unwrap().to_lowercase()
}

#[test]
fn report_mapping_describes_mapping_the_controls_of_a_device_with_no_driver_by_its_fingerprint_name() {
    let d = description_of("report_mapping");
    assert!(d.contains("no driver"), "{d}");
    assert!(d.contains("fingerprint") && d.contains("device_name"), "the device is named as its fingerprint was shared: {d}");
    assert!(d.contains("submit_driver"), "the next step is the first driver: {d}");
}

#[test]
fn submit_driver_describes_a_first_driver_built_from_the_mappings_with_its_source_and_licence() {
    let d = description_of("submit_driver");
    assert!(d.contains("first driver") && d.contains("report_mapping"), "{d}");
    assert!(d.contains("fingerprint"), "{d}");
    assert!(d.contains("licen"), "the licence goes with the source: {d}");
    assert!(d.contains("source"), "{d}");
}

#[test]
fn a_mapping_for_a_device_named_by_its_fingerprint_name_goes_to_the_service_as_it_is() {
    let args = json!({ "device": "My Odd Synth", "label": "Cutoff", "cc": 74, "channel": 1,
        "identity_hex": "F0 7E 00 06 02 00 20 6B 04 00 02 00 01 00 F7" });
    let (reply, calls) = linked(json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": { "name": "report_mapping", "arguments": args } }));
    assert!(reply.get("error").is_none() && reply["result"]["isError"] != true, "{reply}");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["params"]["name"], "report_mapping");
    assert_eq!(calls[0]["params"]["arguments"], args);
}

#[test]
fn a_first_driver_of_a_device_with_no_driver_goes_to_the_service_as_it_is() {
    let args = json!({
        "driver": { "device": { "name": "My Odd Synth", "vendor": "Acme", "osc_prefix": "/my_odd_synth" },
                    "cc_params": [{ "name": "Cutoff", "cc": 74, "channel": 1 }] },
        "source": "mapped on the device by report_mapping; license CC0-1.0",
    });
    let (reply, calls) = linked(json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call",
        "params": { "name": "submit_driver", "arguments": args } }));
    assert!(reply.get("error").is_none() && reply["result"]["isError"] != true, "{reply}");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["params"]["name"], "submit_driver");
    assert_eq!(calls[0]["params"]["arguments"], args);
}

// ---- the learn mode's result: the next step of a device with no driver ----

use osc_bridge::detect::{Learned, Unit};
use osc_bridge::mcp::DeviceIndex;
use std::time::Duration;

fn catalogue() -> DeviceIndex {
    DeviceIndex::build(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/devices"))
}

fn unit(port: &str) -> Unit {
    Unit { inputs: vec![port.into()], outputs: vec![port.into()], in_indexes: vec![0], out_indexes: vec![0], usb: None }
}

fn learned(port: &str) -> Learned {
    let words = osc_bridge::detect::port_words(port);
    Learned {
        fingerprint: json!({ "$schema": "https://w3id.org/osd/0.6/fingerprint.schema.json", "observed": "2026-11-21",
            "via": "native", "ports": { "inputs": [words], "outputs": [words] },
            "controls": [{ "type": "cc", "channel": 1, "number": 74 }] }),
        measured: json!({}),
    }
}

fn report(ports: &[&str]) -> Value {
    let learned: Vec<(Unit, Learned)> = ports.iter().map(|p| (unit(p), learned(p))).collect();
    osc_bridge::mcp::learn_report(&learned, &catalogue(), Duration::from_millis(5000))
}

fn device_of(report: &Value, at: usize) -> &Value { &report["structuredContent"]["devices"][at] }

#[test]
fn a_device_the_catalogue_does_not_recognise_carries_a_next_step_naming_it_then_mapping_its_controls() {
    let r = report(&["Zork Thing 1"]);
    let step = device_of(&r, 0)["next_step"].as_str().unwrap_or_else(|| panic!("no next_step: {r}")).to_lowercase();
    assert!(step.contains("name"), "{step}");
    assert!(step.contains("report_mapping"), "{step}");
    assert!(step.contains("device_name") && step.contains("device"), "{step}");
}

#[test]
fn the_text_of_the_report_carries_the_next_step_of_each_unrecognised_device() {
    let r = report(&["Zork Thing 1", "Quux Board 1"]);
    let text = r["content"][0]["text"].as_str().unwrap();
    for at in 0..2 {
        let step = device_of(&r, at)["next_step"].as_str().unwrap_or_else(|| panic!("no next_step: {r}"));
        assert!(text.contains(step), "{step} not in {text}");
    }
}

#[test]
fn a_device_whose_driver_matches_carries_no_next_step() {
    let r = report(&["Acme Box 1"]);
    assert!(device_of(&r, 0).get("next_step").is_none(), "{r}");
    assert!(!r["content"][0]["text"].as_str().unwrap().contains("next_step"), "{r}");
}

#[test]
fn a_recognised_and_an_unrecognised_device_side_by_side_only_the_second_has_a_next_step() {
    let r = report(&["Acme Box 1", "Zork Thing 1"]);
    assert!(device_of(&r, 0).get("next_step").is_none(), "{r}");
    assert!(device_of(&r, 1).get("next_step").is_some(), "{r}");
}

#[test]
fn the_report_keeps_what_it_gave_before_the_next_step() {
    let r = report(&["Zork Thing 1"]);
    let d = device_of(&r, 0);
    assert!(d.get("fingerprint").is_some() && d.get("measured").is_some() && d["port"] == "Zork Thing 1", "{r}");
    assert_eq!(r["structuredContent"]["learn"], true);
    assert_eq!(r["structuredContent"]["listen_ms"], 5000);
}
