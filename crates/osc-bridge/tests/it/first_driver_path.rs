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
    linked_private(request, &[])
}

/// The same session with devices the person keeps private on this machine.
fn linked_private(request: Value, private: &[&str]) -> (Value, Vec<Value>) {
    let _home = super::common::ConfigHome::new();
    let dir = super::common::temp("first-driver");
    let (base, calls) = fake_service();
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: None,
        default_target: "127.0.0.1:9".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: None,
    };
    let private: Vec<String> = private.iter().map(|p| p.to_string()).collect();
    let input = format!("{request}\n");
    let mut out = Vec::new();
    osc_bridge::synthub::mcp::serve(opts, &base, Some(dir.path().join("cache")), &private, input.as_bytes(), &mut out).unwrap();
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
    assert!(d.contains("fingerprint") && d.contains("vendor") && d.contains("model"), "the device is named by its maker and model as its fingerprint was shared: {d}");
    assert!(!d.contains("vendor/model") && !d.contains("slug"), "no slugged id is imposed, the service refuses it: {d}");
    assert!(d.contains("driver id"), "a device with a card keeps its catalogue id: {d}");
    assert!(!d.contains("device_name"), "{d}");
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

use osc_bridge::synthub::learn::{Learned, Unit};
use osc_bridge::mcp::DeviceIndex;
use std::time::Duration;

fn catalogue() -> DeviceIndex {
    DeviceIndex::build(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/devices"),
                       osc_bridge::synthub::catalogue::tier_rank)
}

fn unit(port: &str) -> Unit {
    Unit { inputs: vec![port.into()], outputs: vec![port.into()], in_indexes: vec![0], out_indexes: vec![0], usb: None,
           client_kind: osc_bridge::detect::ClientKind::Unknown }
}

fn learned(port: &str) -> Learned {
    let words = osc_bridge::synthub::learn::port_words(port);
    Learned {
        fingerprint: json!({ "$schema": "https://w3id.org/osd/0.6/fingerprint.schema.json", "observed": "2026-11-21",
            "via": "native", "ports": { "inputs": [words], "outputs": [words] },
            "controls": [{ "type": "cc", "channel": 1, "number": 74 }] }),
        measured: json!({}),
    }
}

fn report(ports: &[&str]) -> Value {
    let learned: Vec<(Unit, Learned)> = ports.iter().map(|p| (unit(p), learned(p))).collect();
    osc_bridge::synthub::learn::learn_report(&learned, &catalogue(), Duration::from_millis(5000),
        &osc_bridge::synthub::learn::ServiceMatches::NotLinked, osc_bridge::synthub::catalogue::tier_rank)
}

fn device_of(report: &Value, at: usize) -> &Value { &report["structuredContent"]["devices"][at] }

#[test]
fn a_device_the_catalogue_does_not_recognise_carries_a_next_step_naming_it_then_mapping_its_controls() {
    let r = report(&["Zork Thing 1"]);
    let step = device_of(&r, 0)["next_step"].as_str().unwrap_or_else(|| panic!("no next_step: {r}")).to_lowercase();
    assert!(step.contains("name"), "{step}");
    assert!(step.contains("report_mapping"), "{step}");
    assert!(!step.contains("device_name"), "{step}");
}

#[test]
fn the_next_step_of_an_unknown_device_names_the_three_things_the_sharing_needs() {
    for ports in [&["Zork Thing 1"][..]] {
        let r = report(ports);
        let step = device_of(&r, 0)["next_step"].as_str().unwrap().to_lowercase();
        for word in ["vendor", "model", "firmware"] {
            assert!(step.contains(word), "{word} in {step}");
        }
        assert!(step.contains("share_fingerprint"), "{step}");
    }
}

const REPLY_WITH_VERSION: &str = "F0 7E 00 06 02 00 20 6B 04 00 02 00 01 00 02 03 F7";

fn learned_with_reply(port: &str, reply: &str) -> Learned {
    let mut l = learned(port);
    l.fingerprint["identity_reply"] = json!(reply);
    l
}

fn report_with_reply(port: &str, reply: Option<&str>) -> Value {
    let mut l = learned(port);
    if let Some(r) = reply { l.fingerprint["identity_reply"] = json!(r); }
    osc_bridge::synthub::learn::learn_report(&[(unit(port), l)], &catalogue(), Duration::from_millis(5000),
        &osc_bridge::synthub::learn::ServiceMatches::NotLinked, osc_bridge::synthub::catalogue::tier_rank)
}

#[test]
fn the_firmware_of_the_identity_reply_is_proposed_for_the_person_to_confirm() {
    let r = report_with_reply("Zork Thing 1", Some(REPLY_WITH_VERSION));
    let d = device_of(&r, 0);
    assert_eq!(d["proposed_firmware"], "1.0.2.3", "{r}");
    let step = d["next_step"].as_str().unwrap();
    assert!(step.contains("1.0.2.3"), "the step proposes it: {step}");
    assert!(r["content"][0]["text"].as_str().unwrap().contains("1.0.2.3"));
}

#[test]
fn a_device_with_no_identity_reply_has_no_proposed_firmware_and_its_step_asks_the_person() {
    let r = report_with_reply("BCR-32 CTRL", None);
    let d = device_of(&r, 0);
    assert!(d.get("proposed_firmware").is_none(), "{r}");
    let step = d["next_step"].as_str().unwrap().to_lowercase();
    assert!(step.contains("firmware") && step.contains("ask"), "{step}");
}

/// The `device` argument a share's next step gives `report_mapping`.
fn shared_device(vendor: &str, model: &str) -> String {
    let step = osc_bridge::synthub::learn::shared_next_step(vendor, model);
    assert!(step.contains("report_mapping") && !step.contains("device_name"), "{step}");
    let from = step.find("(device: ").unwrap_or_else(|| panic!("no device in {step}")) + "(device: ".len();
    let len = step[from..].find("); ").unwrap_or_else(|| panic!("device not closed in {step}"));
    step[from..from + len].to_string()
}

/// The service's rule, copied here as the test's own aid: a device without a card is named by its
/// model, or by its maker and model, case-insensitive, spaces reduced to one.
fn service_accepts(device: &str, vendor: &str, model: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let d = norm(device);
    d == norm(model) || d == norm(&format!("{vendor} {model}"))
}

#[test]
fn after_a_share_the_next_step_names_the_device_as_the_person_declared_it() {
    assert_eq!(shared_device("Sequential", "Prophet Rev2"), "Sequential Prophet Rev2");
    assert_eq!(shared_device("Behringer", "BCR-32"), "Behringer BCR-32");
    assert_eq!(shared_device("Black Corporation", "Deckard's Dream"), "Black Corporation Deckard's Dream");
}

#[test]
fn after_a_share_the_next_step_carries_no_slug_of_the_name() {
    for (v, m) in [("Sequential", "Prophet Rev2"), ("Arturia", "MiniLab 3"), ("Black Corporation", "Deckard's Dream")] {
        let step = osc_bridge::synthub::learn::shared_next_step(v, m);
        assert!(!step.contains('/') && !step.contains("-rev2") && !step.contains("minilab-3") && !step.contains("black-"), "{step}");
    }
}

#[test]
fn a_model_already_starting_with_the_maker_is_given_alone() {
    assert_eq!(shared_device("Sequential", "Sequential Prophet Rev2"), "Sequential Prophet Rev2");
    assert_eq!(shared_device("sequential", "SEQUENTIAL Prophet"), "SEQUENTIAL Prophet");
    assert_eq!(shared_device("Black Corporation", "black corporation Deckard"), "black corporation Deckard");
}

#[test]
fn a_model_only_starting_with_the_letters_of_the_maker_is_not_taken_for_it() {
    assert_eq!(shared_device("Korg", "Korgasm"), "Korg Korgasm");
}

#[test]
fn spaces_around_and_inside_the_maker_and_the_model_are_reduced_to_one() {
    assert_eq!(shared_device("  Arturia ", "  MiniLab   3 "), "Arturia MiniLab 3");
    assert_eq!(shared_device(" Black   Corporation", "Deckard\t Dream "), "Black Corporation Deckard Dream");
    assert_eq!(shared_device(" Sequential", "  Sequential   Prophet  Rev2"), "Sequential Prophet Rev2");
}

#[test]
fn the_device_of_the_next_step_is_accepted_by_the_services_rule_for_the_shared_devices() {
    for (v, m) in [("Sequential", "Prophet Rev2"), ("Sequential", "Sequential Prophet Rev2"), ("Black Corporation", "Deckard's Dream"),
                   ("Black Corporation", "Black Corporation Deckard"), ("  Arturia ", "  MiniLab   3 "), ("Behringer", "BCR-32"),
                   ("Korg", "Korgasm")] {
        let device = shared_device(v, m);
        assert!(service_accepts(&device, v, m), "{device:?} refused for {v:?} / {m:?}");
        assert!(!device.contains('/') && !device.starts_with(' ') && !device.ends_with(' ') && !device.contains("  "), "{device:?}");
    }
}

#[test]
fn the_next_step_of_an_unknown_device_before_a_share_does_not_impose_the_slugged_id() {
    let r = report(&["Zork Thing 1"]);
    let step = device_of(&r, 0)["next_step"].as_str().unwrap().to_lowercase();
    assert!(step.contains("report_mapping"), "{step}");
    assert!(!step.contains("vendor/model") && !step.contains("slug"), "{step}");
    assert!(step.contains("vendor") && step.contains("model"), "{step}");
}

fn map_call(device: &str, private: &[&str]) -> (Value, Vec<Value>) {
    linked_private(json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "report_mapping",
        "arguments": { "device": device, "label": "Cutoff", "cc": 74, "channel": 1 } } }), private)
}

#[test]
fn a_mapping_named_as_the_next_step_gives_it_is_refused_when_the_private_list_covers_the_device() {
    for private in ["sequential/prophet-rev2", "sequential/*"] {
        let (reply, calls) = map_call("Sequential Prophet Rev2", &[private]);
        assert!(reply.get("error").is_some() || reply["result"]["isError"] == true, "{private}: {reply}");
        assert!(calls.is_empty(), "{private}: a private device is not sent: {calls:?}");
    }
}

#[test]
fn a_mapping_named_as_the_next_step_gives_it_is_forwarded_when_nothing_is_private() {
    let (reply, calls) = map_call("Sequential Prophet Rev2", &[]);
    assert!(reply.get("error").is_none() && reply["result"]["isError"] != true, "{reply}");
    assert_eq!(calls.len(), 1, "{calls:?}");
    let (_, calls) = map_call("Sequential Prophet Rev2", &["korg/*", "arturia/minilab-3"]);
    assert_eq!(calls.len(), 1, "another device's privacy does not stop it: {calls:?}");
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

/// The identity reply of the Acme Box card: Acme's id, the family and member the card declares.
const ACME_BOX_REPLY: &str = "F0 7E 00 06 02 00 21 09 01 00 01 00 00 00 00 00 F7";

#[test]
fn a_device_whose_driver_matches_by_a_strong_proof_carries_no_next_step() {
    let r = report_with_reply("Acme Box 1", Some(ACME_BOX_REPLY));
    assert!(device_of(&r, 0).get("driver").is_some(), "{r}");
    assert!(device_of(&r, 0).get("next_step").is_none(), "{r}");
    assert!(!r["content"][0]["text"].as_str().unwrap().contains("next_step"), "{r}");
}

#[test]
fn a_recognised_and_an_unrecognised_device_side_by_side_only_the_second_has_a_next_step() {
    let known = learned_with_reply("Acme Box 1", ACME_BOX_REPLY);
    let other = learned("Zork Thing 1");
    let r = osc_bridge::synthub::learn::learn_report(&[(unit("Acme Box 1"), known), (unit("Zork Thing 1"), other)],
        &catalogue(), Duration::from_millis(5000), &osc_bridge::synthub::learn::ServiceMatches::NotLinked,
        osc_bridge::synthub::catalogue::tier_rank);
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

#[test]
fn no_tool_description_or_schema_speaks_of_a_free_device_name() {
    let (reply, _) = linked(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
    assert!(!reply.to_string().contains("device_name"), "a published tool still names device_name");
}
