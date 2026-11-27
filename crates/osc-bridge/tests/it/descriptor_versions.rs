//! A driver or a profile earlier than OSD 0.7, from a folder, the cache or the service, is
//! converted to 0.7 before it is read and resolved, the kept file left as received. A document
//! the converter or the reader refuses is an error naming the file and `osd`'s fault, and is not
//! listed. A converted source keeps its trust, and an export credits every source.

use std::path::Path;

use crate::common::{files, mcp_session, temp, tool_call, FakeSynthHub, D05, P05};
use osc_bridge::device::load;
use serde_json::{json, Value};

const D07: &str = "https://w3id.org/osd/0.7/device.schema.json";

/// The results of the tool calls over one offline session on the drivers under `root`.
fn session(root: &Path, calls: &[(&str, Value)]) -> Vec<Value> {
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(root.to_path_buf()),
        default_target: "127.0.0.1:9".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: None,
    };
    let input: String = calls.iter().enumerate().map(|(n, (tool, args))| format!("{}\n", tool_call(n as u64 + 1, tool, args.clone()))).collect();
    let mut out = Vec::new();
    osc_bridge::mcp::serve_with_midi(opts, input.as_bytes(), &mut out, &crate::common::refusing("descriptor_versions")).unwrap();
    String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn one(root: &Path, tool: &str, args: Value) -> Value {
    session(root, &[(tool, args)]).remove(0)["result"].clone()
}

/// Two controls of a 0.5 driver claim the same CC: the converter refuses the second.
fn twice_claimed() -> String {
    format!(r#"{{"$schema":"{D05}","device":{{"name":"Dup","vendor":"Acme","osc_prefix":"/dup"}},
        "cc_params":{{"entries":[{{"osc":"/a","cc":7}},{{"osc":"/b","cc":7}}]}}}}"#)
}
const TWICE_AT: &str = "/cc_params/entries/1";

/// A 0.7 driver that holds a flat field: the reader refuses it.
fn flat_in_0_7() -> String {
    format!(r#"{{"$schema":"{D07}","device":{{"name":"Flat","vendor":"Acme","osc_prefix":"/flat"}},
        "cc_params":{{"entries":[{{"osc":"/a","cc":7,"ways":[]}}]}}}}"#)
}
const FLAT_AT: &str = "/cc_params/entries/0/cc";

fn good() -> String {
    format!(r#"{{"$schema":"{D05}","device":{{"name":"Good","vendor":"Acme","osc_prefix":"/good"}},
        "cc_params":{{"entries":[{{"osc":"/cutoff","cc":74}}]}}}}"#)
}

fn catalogue() -> tempfile::TempDir {
    let root = temp("versions");
    files(root.path(), &[("acme/dup.json", &twice_claimed()), ("acme/flat.json", &flat_in_0_7()), ("acme/good.json", &good())]);
    root
}

// ---- refusals ---------------------------------------------------------------

#[test]
fn a_document_the_converter_or_the_reader_refuses_fails_the_load_naming_the_file_and_the_fault() {
    let root = catalogue();
    for (file, at) in [("dup", TWICE_AT), ("flat", FLAT_AT)] {
        let said = format!("{:#}", load(root.path().join(format!("acme/{file}.json"))).expect_err(file));
        assert!(said.contains(&format!("{file}.json")) || said.contains(&format!("acme/{file}")), "{said}");
        assert!(said.contains(at), "{file}: {said}");
    }
}

#[test]
fn a_profile_the_converter_refuses_fails_the_load_naming_the_profile_and_the_fault() {
    let root = temp("versions");
    let profile = format!(r#"{{"$schema":"{P05}","identity":{{"wat":1}}}}"#);
    let device = format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box"}},"identity":{{"profile":"acme","ports":["Box"]}}}}"#);
    files(root.path(), &[("acme/_profile.json", &profile), ("acme/box.json", &device)]);
    let said = format!("{:#}", load(root.path().join("acme/box.json")).expect_err("the profile is refused"));
    assert!(said.contains("_profile") || said.contains("acme:"), "{said}");
    assert!(said.contains("/identity/wat"), "{said}");
}

#[test]
fn list_devices_leaves_a_refused_driver_out_and_lists_the_others() {
    let root = catalogue();
    let r = one(root.path(), "list_devices", json!({}));
    let listed: Vec<&str> = r["structuredContent"]["devices"].as_array().unwrap().iter().map(|d| d["device"].as_str().unwrap()).collect();
    assert_eq!(listed, ["acme/good"], "{r}");
}

#[test]
fn every_device_tool_that_names_a_refused_driver_answers_with_the_file_and_the_fault() {
    let root = catalogue();
    for (file, at) in [("dup", TWICE_AT), ("flat", FLAT_AT)] {
        let id = format!("acme/{file}");
        let calls = [
            ("get_device_docs", json!({ "device": id })),
            ("list_routes", json!({ "device": id })),
            ("export_device", json!({ "device": id, "format": "oscquery" })),
            ("list_roles", json!({ "device": id })),
            ("set_by_role", json!({ "device": id, "role": "filter/cutoff", "value": 0.5 })),
            ("get_by_role", json!({ "device": id, "role": "filter/cutoff" })),
            ("install_export", json!({ "device": id, "on": "acme/good" })),
            ("install_export", json!({ "device": "acme/good", "on": id })),
        ];
        for ((tool, _), reply) in calls.iter().zip(session(root.path(), &calls)) {
            assert!(reply["result"]["isError"] == json!(true) || reply.get("error").is_some(), "{tool}: {reply}");
            let said = reply.to_string();
            assert!(said.contains(&id), "{tool} names the file: {said}");
            assert!(said.contains(at), "{tool} gives the fault: {said}");
        }
    }
}

#[test]
fn building_the_index_says_one_line_per_refused_file_on_the_standard_error() {
    let root = catalogue();
    let home = temp("versions-home");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_osc-bridge"))
        .args(["mcp", "--offline", "--devices-dir"]).arg(root.path())
        .env("HOME", home.path()).env("XDG_CONFIG_HOME", home.path()).env("XDG_CACHE_HOME", home.path())
        .env("APPDATA", home.path()).env("LOCALAPPDATA", home.path())
        .env(crate::common::MIDI_ENV, "failing")
        .stdin(std::process::Stdio::null())
        .output().unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    for (file, at) in [("dup", TWICE_AT), ("flat", FLAT_AT)] {
        let lines: Vec<&str> = said.lines().filter(|l| l.contains(&format!("acme/{file}"))).collect();
        assert_eq!(lines.len(), 1, "{file}: {said}");
        assert!(lines[0].contains(at), "{file}: {said}");
    }
    assert!(!said.contains("acme/good"), "{said}");
}

// ---- the cache and the service ----------------------------------------------

#[test]
fn a_driver_of_the_service_is_kept_as_received_and_served_in_0_7() {
    let dir = temp("versions-service");
    let (server_dir, cache) = (dir.path().join("server"), dir.path().join("cache"));
    let profile = format!(r#"{{"$schema":"{P05}","identity":{{"reply":[240,126]}}}}"#);
    let device = format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box"}},
        "identity":{{"profile":"acme","ports":["Box"]}},"cc_params":{{"entries":[{{"osc":"/cutoff","cc":74}}]}}}}"#);
    files(&server_dir, &[("acme/_profile.json", &profile), ("acme/box.json", &device)]);
    let server = FakeSynthHub::start(&server_dir, None);
    let replies = mcp_session(&server.base, &cache, &[tool_call(1, "list_routes", json!({ "device": "acme/box" }))]);
    let r = &replies[0]["result"];
    assert_eq!(r["structuredContent"]["cc_params"][0]["ways"], json!([{ "ctrlType": "cc", "ctrlIndex": [74] }]), "{r}");
    for (file, received) in [("acme/box.json", &device), ("acme/_profile.json", &profile)] {
        assert_eq!(&std::fs::read_to_string(cache.join(file)).unwrap(), received, "{file} is kept as received");
    }
}

#[test]
fn every_device_tool_that_names_a_driver_of_the_service_that_is_refused_answers_with_the_file_and_the_fault() {
    let dir = temp("versions-service-refused");
    let (server_dir, cache) = (dir.path().join("server"), dir.path().join("cache"));
    // A driver the converter refuses, one the reader refuses, one whose maker profile is refused.
    let profile = format!(r#"{{"$schema":"{P05}","identity":{{"wat":1}}}}"#);
    let profiled = format!(r#"{{"$schema":"{D05}","device":{{"name":"Box","vendor":"Prof","osc_prefix":"/profiled"}},
        "identity":{{"profile":"prof","ports":["Box"]}}}}"#);
    files(&server_dir, &[("acme/dup.json", &twice_claimed()), ("acme/flat.json", &flat_in_0_7()), ("acme/good.json", &good()),
        ("prof/_profile.json", &profile), ("prof/box.json", &profiled)]);
    let server = FakeSynthHub::start(&server_dir, None);
    for (id, at) in [("acme/dup", TWICE_AT), ("acme/flat", FLAT_AT), ("prof/box", "/identity/wat")] {
        // One session for all the calls: the driver is refused at each, not at the first only.
        let calls = [
            ("get_device_docs", json!({ "device": id })),
            ("list_routes", json!({ "device": id })),
            ("export_device", json!({ "device": id, "format": "oscquery" })),
            ("list_roles", json!({ "device": id })),
            ("set_by_role", json!({ "device": id, "role": "filter/cutoff", "value": 0.5 })),
            ("get_by_role", json!({ "device": id, "role": "filter/cutoff" })),
            ("install_export", json!({ "device": id, "on": "acme/good" })),
            ("install_export", json!({ "device": "acme/good", "on": id })),
        ];
        let requests: Vec<Value> = calls.iter().enumerate().map(|(n, (tool, args))| tool_call(n as u64 + 1, tool, args.clone())).collect();
        let replies = mcp_session(&server.base, &cache, &requests);
        assert_eq!(replies.len(), calls.len(), "{id}");
        for ((tool, _), reply) in calls.iter().zip(replies) {
            assert!(reply["result"]["isError"] == json!(true) || reply.get("error").is_some(), "{id} {tool}: {reply}");
            let said = reply.to_string();
            assert!(said.contains(id), "{tool} names the file: {said}");
            assert!(said.contains(at), "{id}: {tool} gives the fault: {said}");
        }
    }
}

// ---- the trust of a converted source ----------------------------------------

/// A driver at the OSC prefix `prefix` whose first source is `source`.
fn sourced(schema: &str, name: &str, prefix: &str, source: Value) -> String {
    let key = if schema == D07 { "sources" } else { "_sources" };
    let mut d = json!({ "$schema": schema, "device": { "name": name, "vendor": "Acme", "osc_prefix": prefix } });
    d[key] = json!([source]);
    d.to_string()
}

fn contributed(provider: &str) -> Value {
    json!({ "type": "contributed", "provider": provider, "url": "https://example.org/x" })
}

#[test]
fn a_contributed_source_of_a_product_provider_keeps_the_tier_it_had() {
    let root = temp("versions-tiers");
    let d06 = "https://w3id.org/osd/0.6/device.schema.json";
    let old = |kind: &str| json!({ "type": kind, "url": "https://example.org/x" });
    let cases = [
        ("electra", sourced(D07, "A", "/electra", contributed("electra-one")), "electra-preset"),
        ("pencil", sourced(D07, "B", "/pencil", contributed("pencilresearch")), "pencilresearch"),
        ("kraft", sourced(D07, "C", "/kraft", contributed("knobkraft")), "knobkraft"),
        ("plain", sourced(D07, "D", "/plain", json!({ "type": "contributed" })), "contributed"),
        ("vendor", sourced(D07, "E", "/vendor", json!({ "type": "vendor-doc", "provider": "electra-one" })), "vendor-doc"),
        ("old_electra", sourced(d06, "F", "/old_electra", old("electra-preset")), "electra-preset"),
        ("old_pencil", sourced(D05, "G", "/old_pencil", old("pencilresearch")), "pencilresearch"),
        ("old_kraft", sourced(d06, "H", "/old_kraft", old("knobkraft")), "knobkraft"),
        ("old_midnam", sourced(d06, "I", "/old_midnam", old("midnam")), "contributed"),
    ];
    for (file, text, _) in &cases {
        files(root.path(), &[(&format!("acme/{file}.json"), text)]);
    }
    let r = one(root.path(), "list_devices", json!({}));
    let listed = r["structuredContent"]["devices"].as_array().unwrap();
    for (file, _, tier) in &cases {
        let d = listed.iter().find(|d| d["device"] == format!("acme/{file}")).unwrap_or_else(|| panic!("{file}: {r}"));
        assert_eq!(d["source_tier"], *tier, "{file}");
    }
}

#[test]
fn a_prefix_goes_to_the_driver_whose_provider_ranks_first() {
    let root = temp("versions-rank");
    files(root.path(), &[
        // A plain contribution first by path, then the providers.
        ("acme/a.json", &sourced(D07, "Plain", "/one", json!({ "type": "contributed" }))),
        ("acme/b.json", &sourced(D07, "Kraft", "/one", contributed("knobkraft"))),
        ("acme/c.json", &sourced(D07, "Pencil", "/two", contributed("pencilresearch"))),
        ("acme/d.json", &sourced(D07, "Electra", "/two", contributed("electra-one"))),
        ("acme/e.json", &sourced("https://w3id.org/osd/0.6/device.schema.json", "Midnam", "/three", json!({ "type": "midnam" }))),
        ("acme/f.json", &sourced(D07, "Kraft again", "/three", contributed("knobkraft"))),
    ]);
    for (prefix, name) in [("one", "Kraft"), ("two", "Electra"), ("three", "Kraft again")] {
        let r = one(root.path(), "list_routes", json!({ "device": prefix }));
        assert_eq!(r["structuredContent"]["device"], name, "{prefix}: {r}");
    }
}

// ---- the credit of an export ------------------------------------------------

fn credited(schema: &str, prefix: &str, source: (&str, &str), midi_of: Option<&str>) -> String {
    let key = if schema == D07 { "sources" } else { "_sources" };
    let mut d = json!({ "$schema": schema, "device": { "name": "Box", "vendor": "Acme", "osc_prefix": prefix } });
    d[key] = json!([{ "type": "community-re", "url": source.0, "license": source.1 }]);
    match midi_of {
        Some(target) => d["midi_of"] = json!(target),
        None if schema == D07 => d["cc_params"] = json!({ "entries": [{ "osc": "/cutoff", "ways": [{ "ctrlType": "cc", "ctrlIndex": [74] }] }] }),
        None => d["cc_params"] = json!({ "entries": [{ "osc": "/cutoff", "cc": 74 }] }),
    }
    d.to_string()
}

#[test]
fn an_export_credits_the_drivers_sources_and_those_of_the_midi_it_shares_with_their_licences() {
    let d06 = "https://w3id.org/osd/0.6/device.schema.json";
    for chain in [[D07, D07, D07], [D05, D05, D05], [D05, d06, D07]] {
        let root = temp("versions-credit");
        let sources = [("https://example.org/stock", "CC-BY-SA-4.0"), ("https://example.org/mid", "GPL-3.0-only"), ("https://example.org/leaf", "Apache-2.0")];
        files(root.path(), &[
            ("acme/stock.json", &credited(chain[0], "/stock", sources[0], None)),
            ("acme/mid.json", &credited(chain[1], "/mid", sources[1], Some("acme/stock"))),
            ("acme/leaf.json", &credited(chain[2], "/leaf", sources[2], Some("acme/mid"))),
        ]);
        for format in ["midnam", "oscquery"] {
            let r = one(root.path(), "export_device", json!({ "device": "acme/leaf", "format": format }));
            assert_ne!(r["isError"], json!(true), "{chain:?} {format}: {r}");
            let document = r["content"][0]["text"].as_str().unwrap();
            for (url, license) in sources {
                assert!(document.contains(url) && document.contains(license), "{chain:?} {format}: {url} {license} in {document}");
            }
        }
    }
}
