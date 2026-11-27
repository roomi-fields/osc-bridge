//! How a 0.7 identity recognises a device: its maker under `identity.manufacturerId`, and its
//! rules on port names as OSD 0.7 defines them (section 5.3.7). No MIDI port is opened: the
//! hardware is the shared fake.

use std::path::Path;

use crate::common::{files, temp, tool_call, FakeHardware};
use osc_bridge::device::load;
use osc_bridge::install::install;
use osc_bridge::midi_port::Midi;
use osc_bridge::programs::Clock;
use serde_json::{json, Value};

const D07: &str = "https://w3id.org/osd/0.7/device.schema.json";
/// The platform of the machine and one that is not.
const HERE: &str = if cfg!(target_os = "windows") { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "linux" };
const ELSEWHERE: &str = if cfg!(target_os = "windows") { "linux" } else { "windows" };
const STACK_HERE: &str = if cfg!(target_os = "windows") { "winmm" } else if cfg!(target_os = "macos") { "coremidi" } else { "alsa-seq" };

/// A driver whose names say nothing of any port of these tests.
fn driver(identity: Value) -> String {
    json!({ "$schema": D07, "device": { "name": "Quux", "vendor": "Zorg", "osc_prefix": "/quux" }, "identity": identity }).to_string()
}

fn catalogue(list: &[(&str, &str)]) -> tempfile::TempDir {
    let root = temp("port-rules");
    files(root.path(), list);
    root
}

/// The candidates `detect_hardware` names for a device on the port `label`.
fn candidates(root: &Path, label: &str, reply: &[u8]) -> Vec<Value> {
    let hardware = FakeHardware::device(label, reply);
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(root.to_path_buf()),
        default_target: "127.0.0.1:9".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: None,
    };
    let input = format!("{}\n", tool_call(1, "detect_hardware", json!({})));
    let mut out = Vec::new();
    osc_bridge::mcp::serve_with_midi(opts, input.as_bytes(), &mut out, &hardware.opener()).unwrap();
    let r: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    assert_ne!(r["result"]["isError"], json!(true), "{r}");
    let device = &r["result"]["structuredContent"]["devices"][0];
    assert_eq!(device["port"], label, "{r}");
    device["candidates"].as_array().cloned().unwrap_or_default()
}

/// An Arturia identity reply (maker 00 20 6B).
const ARTURIA: [u8; 17] = [0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x00, 0x20, 0x6B, 0x02, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0xF7];

// ---- the maker --------------------------------------------------------------

#[test]
fn the_maker_of_identity_manufacturer_id_written_or_given_by_a_profile_proves_the_maker() {
    let own = driver(json!({ "manufacturerId": [0, 32, 107], "ports": [{ "operator": "contains", "value": "Blue" }] }));
    let from_profile = driver(json!({ "profile": "acme", "ports": [{ "operator": "contains", "value": "Blue" }] }));
    let profile = r#"{"$schema":"https://w3id.org/osd/0.7/profile.schema.json","identity":{"manufacturerId":[0,32,107]}}"#;
    for (text, name) in [(own, "written"), (from_profile, "from the profile")] {
        let root = catalogue(&[("acme/_profile.json", profile), ("acme/quux.json", &text)]);
        let found = candidates(root.path(), "Blue Thing", &ARTURIA);
        assert_eq!(found[0]["device"], "acme/quux", "{name}: {found:?}");
        let proofs = found[0]["proofs"].as_array().unwrap();
        assert!(proofs.contains(&json!({ "nature": "medium", "kind": "maker", "port": "Blue Thing" })), "{name}: {found:?}");
    }
}

#[test]
fn inspect_says_the_maker_of_identity_manufacturer_id_on_its_mid_line() {
    let profile05 = format!(r#"{{"$schema":"{}","identity":{{"reply":[240,126]}}}}"#, crate::common::P05);
    let profile = r#"{"$schema":"https://w3id.org/osd/0.7/profile.schema.json","identity":{"manufacturerId":[0,32,107]}}"#;
    let written = driver(json!({ "manufacturerId": [0, 33, 9], "ports": [] , "usb": [{ "vendor_id": 1, "product_id": 2 }] }));
    let given = driver(json!({ "profile": "acme", "usb": [{ "vendor_id": 1, "product_id": 2 }] }));
    let earlier = format!(r#"{{"$schema":"{}","device":{{"name":"Old","osc_prefix":"/old","manufacturer_id":[125]}},"identity":{{"profile":"zed","ports":["Old"]}}}}"#, crate::common::D05);
    let root = catalogue(&[("acme/_profile.json", profile), ("zed/_profile.json", &profile05),
        ("acme/written.json", &written), ("acme/given.json", &given), ("zed/old.json", &earlier)]);
    for (file, mid) in [("acme/written", "MID:      00 21 09"), ("acme/given", "MID:      00 20 6B"), ("zed/old", "MID:      7D")] {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        osc_bridge::commands::inspect(&root.path().join(format!("{file}.json")), &mut out, &mut err).unwrap_or_else(|e| panic!("{file}: {e:#}"));
        let out = String::from_utf8(out).unwrap();
        assert!(out.lines().any(|l| l == mid), "{file}: {out}");
    }
}

// ---- the rules on port names ------------------------------------------------

fn rule(operator: &str, value: &str) -> Value {
    json!({ "operator": operator, "value": value })
}

/// Whether a driver whose one port rule is `rule` is a candidate for the port `Blue Thing 1`.
fn recognises(rule: Value) -> bool {
    let root = catalogue(&[("acme/quux.json", &driver(json!({ "ports": [rule] })))]);
    candidates(root.path(), "Blue Thing 1", &[]).iter().any(|c| c["device"] == "acme/quux")
}

#[test]
fn each_operator_compares_a_port_name_without_regard_to_case() {
    for (operator, value, expected) in [
        ("equals", "BLUE THING 1", true), ("equals", "blue thing", false),
        ("contains", "tHiNg", true), ("contains", "red", false),
        ("starts_with", "BLUE t", true), ("starts_with", "thing", false),
        ("ends_with", "G 1", true), ("ends_with", "blue", false),
    ] {
        assert_eq!(recognises(rule(operator, value)), expected, "{operator} {value}");
    }
}

#[test]
fn a_rule_of_the_platform_of_the_machine_recognises_and_one_of_another_platform_or_of_a_stack_does_not() {
    let on = |platform: &str, stack: Option<&str>| {
        let mut r = rule("contains", "Blue");
        r["platform"] = json!(platform);
        if let Some(s) = stack {
            r["stack"] = json!(s);
        }
        r
    };
    assert!(recognises(on(HERE, None)));
    assert!(!recognises(on(ELSEWHERE, None)));
    assert!(!recognises(on(HERE, Some(STACK_HERE))));
}

struct Ports(Vec<String>, Vec<String>);
impl Midi for Ports {
    fn outputs(&self) -> Vec<String> { self.0.clone() }
    fn send(&mut self, port: &str, _messages: &[Vec<u8>], _why: &str) -> Result<(), String> {
        self.1.push(port.to_string());
        Ok(())
    }
    fn received(&mut self, _wait: std::time::Duration) -> Vec<(String, Vec<u8>)> { Vec::new() }
}
struct NoClock;
impl Clock for NoClock { fn sleep(&mut self, _d: std::time::Duration) {} }

/// The ports the messages of an install went to, on a target whose port rules are `rules`, the
/// outputs of the machine being `outputs`.
fn installed_on(rules: Value, outputs: &[&str]) -> Result<Vec<String>, String> {
    let target = json!({ "$schema": D07, "device": { "name": "Target", "vendor": "Zed", "osc_prefix": "/target" },
        "identity": { "ports": rules },
        "resources": [{ "resource": "X-Names", "canSet": "full", "mediaTypes": ["application/xml"], "export": "midnam",
            "via": { "file_write": { "path": ["Names", { "variable": "device", "suffix": ".midnam" }], "carried_by": "sysex", "steps": [
                { "step": "open", "send": [240, 0, 32, 9, 2, 247] },
                { "step": "write", "chunk_bytes": 4000, "coding": "mcoded7", "send": [240, 0, 32, 9, 4, { "file": "chunk" }, 247] },
                { "step": "close", "send": [240, 0, 32, 9, 5, 247] } ] } } }] });
    let source = json!({ "$schema": D07, "device": { "name": "Box", "vendor": "Acme", "osc_prefix": "/box" },
        "cc_params": { "entries": [{ "osc": "/cutoff", "ways": [{ "ctrlType": "cc", "ctrlIndex": [74] }] }] },
        "sources": [{ "type": "community-re", "url": "https://example.org/box", "license": "CC0-1.0" }] });
    let root = catalogue(&[("zed/target.json", &target.to_string()), ("acme/box.json", &source.to_string())]);
    let target = load(root.path().join("zed/target.json")).unwrap_or_else(|e| panic!("{e:#}"));
    let source = load(root.path().join("acme/box.json")).unwrap_or_else(|e| panic!("{e:#}"));
    let mut midi = Ports(outputs.iter().map(|s| s.to_string()).collect(), Vec::new());
    install(&source, &target, true, &mut midi, &mut NoClock)?;
    midi.1.dedup();
    Ok(midi.1)
}

#[test]
fn an_export_goes_to_the_output_a_rule_without_direction_or_of_output_names() {
    let mut output = rule("equals", "BETA");
    output["direction"] = json!("output");
    let mut input = rule("contains", "alpha");
    input["direction"] = json!("input");
    assert_eq!(installed_on(json!([input, output]), &["alpha", "beta"]).unwrap(), ["beta"]);
    assert_eq!(installed_on(json!([rule("starts_with", "bet")]), &["alpha", "beta"]).unwrap(), ["beta"]);
}

#[test]
fn an_export_goes_to_no_output_named_only_by_rules_of_input_another_platform_or_a_stack() {
    for (key, value) in [("direction", "input"), ("platform", ELSEWHERE), ("stack", STACK_HERE)] {
        let mut r = rule("contains", "beta");
        r[key] = json!(value);
        if key == "stack" {
            r["platform"] = json!(HERE);
        }
        let said = installed_on(json!([r]), &["alpha", "beta"]).expect_err(key);
        assert!(said.contains("not plugged"), "{key}: {said}");
    }
}

#[test]
fn an_identity_that_holds_only_an_endpoint_name_loads() {
    let root = catalogue(&[("acme/quux.json", &driver(json!({ "endpoint_name": { "operator": "equals", "value": "Quux" } })))]);
    load(root.path().join("acme/quux.json")).unwrap_or_else(|e| panic!("{e:#}"));
}

#[test]
fn a_detection_probe_keeps_ports_and_match_ports_as_the_values_of_the_rules() {
    let identity = json!({ "request": [240, 125, 1, 247], "to": "named_ports", "reply": [240, 125, 2],
        "ports": [rule("starts_with", "Blue"), rule("equals", "Red Port")] });
    let root = catalogue(&[("acme/quux.json", &driver(identity))]);
    let index = osc_bridge::mcp::DeviceIndex::build(root.path(), osc_bridge::synthub::catalogue::tier_rank);
    let probes = osc_bridge::mcp::probes_json(&osc_bridge::mcp::detect_probes(&index));
    assert_eq!(probes["probes"][0]["ports"], json!(["Blue", "Red Port"]), "{probes}");
    assert_eq!(probes["probes"][0]["match_ports"], json!(["Blue", "Red Port"]), "{probes}");
}
