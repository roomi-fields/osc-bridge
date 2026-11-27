//! The MCP device tools give each control with the `ways` of OSD 0.7, as the driver writes
//! them, in place of the flat fields of earlier versions; its `range` is the reference scale of
//! the control and its `usage` the text of `value_description`.

use crate::common::{files, temp, tool_call};
use serde_json::{json, Value};

/// The result of one tool call over one offline session on the drivers under `root`.
fn call(root: &std::path::Path, tool: &str, args: Value) -> Value {
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(root.to_path_buf()),
        default_target: "127.0.0.1:9".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: None,
    };
    let input = format!("{}\n", tool_call(1, tool, args));
    let mut out = Vec::new();
    osc_bridge::mcp::serve_with_midi(opts, input.as_bytes(), &mut out, &crate::common::refusing("route_ways")).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    reply["result"].clone()
}

fn routes(document: Value) -> Value {
    let root = temp("route-ways");
    files(root.path(), &[("acme/box.json", &document.to_string())]);
    let r = call(root.path(), "list_routes", json!({ "device": "acme/box" }));
    assert_ne!(r["isError"], json!(true), "{r}");
    r["structuredContent"].clone()
}

/// The flat fields of a control before 0.7.
const FLAT: [&str; 7] = ["cc", "cc_lsb", "nrpn_msb", "nrpn_lsb", "nrpn_lsb_range", "rpn_msb", "rpn_lsb"];

fn ways_of_filter() -> Value {
    json!([
        { "ctrlType": "nrpn", "ctrlIndex": [1, 2], "value_bits": 14, "range": [0, 16383], "transmit": "absolute", "recognize": "both", "nrpn_lsb_range": [0, 10] },
        { "ctrlType": "cc", "ctrlIndex": [74], "range": [0, 127], "recognize": "none" },
        { "ctrlType": "cc", "ctrlIndex": [75], "in": { "mode": ["single", "multi"] }, "slice": [0, 63] },
        { "ctrlType": "pBend" }
    ])
}

fn driver() -> Value {
    json!({
        "$schema": "https://w3id.org/osd/0.7/device.schema.json",
        "device": { "name": "Box", "vendor": "Acme", "osc_prefix": "/box" },
        "contexts": { "mode": { "values": [{ "id": "single" }, { "id": "multi" }] } },
        "cc_params": { "entries": [
            { "osc": "/filter", "value_description": "0 closed, 127 open", "ways": ways_of_filter() },
            { "osc": "/pan", "scale": [-64, 63], "ways": [{ "ctrlType": "cc", "ctrlIndex": [10], "range": [0, 127] }] },
            { "osc": "/partial", "range": [0, 100], "ways": [] },
            { "osc": "/fine", "ways": [{ "ctrlType": "cc", "ctrlIndex": [20] }, { "ctrlType": "cc", "ctrlIndex": [21, 53], "value_bits": 14 }] }
        ] },
        "params": { "value_type": "u7", "get_frame": [240, 1, { "arg": "p" }, 247], "set_frame": [240, 2, { "arg": "p" }, { "arg": "val" }, 247],
            "entries": [
                { "osc": "/volume", "ways": [{ "ctrlType": "sysex", "ctrlIndex": [3], "range": [0, 99] }] },
                { "osc": "/unknown", "ways": [] }
            ] }
    })
}

#[test]
fn list_routes_gives_each_control_its_ways_as_0_7_writes_them_in_their_order() {
    let r = routes(driver());
    let cc = &r["cc_params"];
    assert_eq!(cc[0]["address"], "/box/filter", "{r}");
    assert_eq!(cc[0]["ways"], ways_of_filter(), "{r}");
    assert_eq!(cc[2]["ways"], json!([]), "a control without a way has an empty list: {r}");
    assert_eq!(r["params"][0]["ways"], json!([{ "ctrlType": "sysex", "ctrlIndex": [3], "range": [0, 99] }]), "{r}");
    assert_eq!(r["params"][1]["ways"], json!([]), "{r}");
}

#[test]
fn list_routes_gives_no_flat_field_of_an_earlier_version() {
    let r = routes(driver());
    for table in ["cc_params", "params"] {
        for entry in r[table].as_array().unwrap() {
            assert!(entry.get("ways").is_some(), "{entry}");
            for field in FLAT {
                assert!(entry.get(field).is_none(), "{field} in {entry}");
            }
        }
    }
}

#[test]
fn list_routes_gives_the_reference_scale_as_range_and_value_description_as_usage() {
    let r = routes(driver());
    let cc = &r["cc_params"];
    assert_eq!(cc[0]["range"], json!([0, 16383]), "the most precise way: {r}");
    assert_eq!(cc[1]["range"], json!([-64, 63]), "the written scale: {r}");
    assert_eq!(cc[2]["range"], json!([0, 100]), "the range of a control without a way: {r}");
    assert_eq!(cc[3]["range"], json!([0, 16383]), "the range the 14-bit way implies: {r}");
    assert_eq!(cc[0]["usage"], "0 closed, 127 open", "{r}");
    assert!(cc[1]["usage"].is_null(), "{r}");
    assert_eq!(r["params"][0]["range"], json!([0, 99]), "{r}");
    assert!(r["params"][0]["usage"].is_null(), "a parameter of 0.7 has no value_description: {r}");
}

#[test]
fn list_routes_gives_a_0_5_driver_the_ways_of_its_conversion_and_its_usage_note() {
    let r = routes(json!({
        "$schema": crate::common::D05,
        "device": { "name": "Box", "vendor": "Acme", "osc_prefix": "/box" },
        "cc_params": { "entries": [{ "osc": "/porta", "cc": 5, "nrpn_msb": 0, "nrpn_lsb": 9, "range": [0, 100], "_usage": "0: Off" }] }
    }));
    let entry = &r["cc_params"][0];
    assert_eq!(entry["ways"], json!([
        { "ctrlType": "cc", "ctrlIndex": [5], "range": [0, 100] },
        { "ctrlType": "nrpn", "ctrlIndex": [0, 9], "value_bits": 14, "range": [0, 100] }
    ]), "{r}");
    assert_eq!((&entry["usage"], &entry["range"]), (&json!("0: Off"), &json!([0, 100])), "{r}");
}

#[test]
fn the_description_of_detect_hardware_says_the_fingerprint_is_of_osd_0_7() {
    let root = temp("route-ways");
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(root.path().to_path_buf()),
        default_target: "127.0.0.1:9".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: None,
    };
    let input = format!("{}\n", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
    let mut out = Vec::new();
    osc_bridge::mcp::serve_with_midi(opts, input.as_bytes(), &mut out, &crate::common::refusing("route_ways")).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    let tool = reply["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "detect_hardware").unwrap();
    let said = tool.to_string();
    assert!(said.contains("OSD 0.7 fingerprint"), "{said}");
    assert!(!said.contains("OSD 0.6"), "{said}");
}
