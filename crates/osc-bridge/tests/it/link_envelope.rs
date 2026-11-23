//! The 16 local tools answer with no link and no service; the link answers its own 20 tools first and
//! hands every other request to the local answer, which gives the same text through the link as
//! without it. The link's schemas are held apart from the local ones.
//!
//! No network: the service is a fake one on a local port (`common`).

use crate::common::{catalogue, temp, tool_call, ConfigHome, FakeSynthHub};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn options(devices: Option<&std::path::Path>) -> osc_bridge::mcp::McpOptions {
    osc_bridge::mcp::McpOptions {
        devices_dir: devices.map(|d| d.to_path_buf()),
        default_target: "127.0.0.1:9".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
    }
}

fn replies(out: Vec<u8>) -> Vec<Value> {
    String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn lines(requests: &[Value]) -> String {
    requests.iter().map(|r| format!("{r}\n")).collect()
}

/// The local server: no service, no account, no address of a service.
fn local_session(devices: &std::path::Path, requests: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    osc_bridge::mcp::serve(options(Some(devices)), lines(requests).as_bytes(), &mut out).unwrap();
    replies(out)
}

/// The server of the link, answering through the fake service at `base`.
fn linked_session(devices: Option<&std::path::Path>, base: &str, cache: &std::path::Path, private: &[&str], requests: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    let private: Vec<String> = private.iter().map(|p| p.to_string()).collect();
    osc_bridge::synthub::mcp::serve(options(devices), base, Some(cache.to_path_buf()), &private, lines(requests).as_bytes(), &mut out).unwrap();
    replies(out)
}

fn list() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })
}

fn names_of(schemas: &Value) -> BTreeSet<String> {
    schemas["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

fn service() -> (FakeSynthHub, tempfile::TempDir) {
    let dir = temp("link-envelope");
    catalogue(&dir.path().join("server"));
    (FakeSynthHub::start(&dir.path().join("server"), None), dir)
}

#[test]
fn the_local_server_offers_the_sixteen_local_tools_with_the_schemas_the_bridge_holds() {
    let dir = temp("link-envelope");
    let reply = &local_session(dir.path(), &[list()])[0];
    let offered = reply["result"].clone();
    assert_eq!(offered["tools"].as_array().unwrap().len(), 16);
    assert_eq!(offered, osc_bridge::mcp::local_tool_schemas());
    for linked in ["take_sound", "load_preset", "get_trust", "report_result", "share_preset"] {
        assert!(!names_of(&offered).contains(linked), "{linked} is the link's");
    }
}

#[test]
fn the_local_server_without_a_folder_of_cards_serves_an_empty_index() {
    let _home = ConfigHome::new();
    let mut out = Vec::new();
    let request = tool_call(1, "list_devices", json!({}));
    osc_bridge::mcp::serve(options(None), lines(&[request]).as_bytes(), &mut out).unwrap();
    let reply = &replies(out)[0];
    assert_eq!(reply["result"]["structuredContent"]["count"], json!(0), "{reply}");
}

#[test]
fn the_schemas_of_the_link_are_the_twenty_tools_the_local_server_does_not_have() {
    let linked = osc_bridge::synthub::mcp::linked_tool_schemas();
    let names = names_of(&linked);
    assert_eq!(linked["tools"].as_array().unwrap().len(), 20);
    let expected: BTreeSet<String> = osc_bridge::synthub::online::FORWARDED_TOOLS.iter().chain(&["take_sound", "load_preset"])
        .map(|n| n.to_string()).collect();
    assert_eq!(names, expected);
    assert!(names.is_disjoint(&names_of(&osc_bridge::mcp::local_tool_schemas())));
}

#[test]
fn the_tools_of_the_link_name_a_device_by_device_and_never_by_slug() {
    let linked = osc_bridge::synthub::mcp::linked_tool_schemas();
    for t in linked["tools"].as_array().unwrap() {
        assert!(t["inputSchema"]["properties"].get("slug").is_none(), "{} still takes slug", t["name"]);
        assert!(!serde_json::to_string(t).unwrap().contains("or slug"), "{} says 'or slug'", t["name"]);
    }
}

#[test]
fn a_linked_session_offers_the_local_schemas_and_the_link_schemas_unchanged() {
    let _home = ConfigHome::new();
    let (server, dir) = service();
    let reply = &linked_session(None, &server.base, &dir.path().join("cache"), &[], &[list()])[0];
    let offered: Vec<Value> = reply["result"]["tools"].as_array().unwrap().clone();
    assert_eq!(offered.len(), 36);
    let expected: Vec<Value> = osc_bridge::mcp::local_tool_schemas()["tools"].as_array().unwrap().iter()
        .chain(osc_bridge::synthub::mcp::linked_tool_schemas()["tools"].as_array().unwrap()).cloned().collect();
    let by_name = |tools: &[Value]| tools.iter().map(|t| (t["name"].as_str().unwrap().to_string(), t.clone())).collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(by_name(&offered), by_name(&expected));
}

#[test]
fn a_local_tool_gives_the_same_reply_through_the_link_as_without_it() {
    let _home = ConfigHome::new();
    let (server, dir) = service();
    let devices = dir.path().join("kept");
    catalogue(&devices);
    let requests = [
        tool_call(1, "list_links", json!({ "target": "127.0.0.1:9" })),
        tool_call(2, "add_link", json!({ "to": "/b" })),
        tool_call(3, "get_device_docs", json!({ "device": "acme/box" })),
        tool_call(4, "list_routes", json!({ "slug": "box" })),
        tool_call(5, "list_routes", json!({ "device": "no-such-device" })),
        tool_call(6, "restore_presets", json!({ "device": "acme/box", "out_port": "0", "file": "x.syx" })),
        tool_call(7, "list_roles", json!({ "device": "acme/box" })),
    ];
    let alone = local_session(&devices, &requests);
    let through = linked_session(Some(&devices), &server.base, &dir.path().join("cache"), &[], &requests);
    assert_eq!(alone.len(), 7);
    assert_eq!(through, alone);
    assert!(alone[3]["result"]["isError"] == true || alone[3].get("error").is_some(), "{}", alone[3]);
}

#[test]
fn a_tool_of_the_link_is_unknown_to_the_local_server_and_answered_by_the_link() {
    let _home = ConfigHome::new();
    let (server, dir) = service();
    let devices = dir.path().join("kept");
    catalogue(&devices);
    for tool in ["take_sound", "load_preset", "get_trust", "report_result"] {
        let call = tool_call(1, tool, json!({ "device": "acme/box" }));
        let alone = &local_session(&devices, std::slice::from_ref(&call))[0];
        assert_eq!(alone["error"]["code"], json!(-32602), "{tool}: {alone}");
        let through = &linked_session(Some(&devices), &server.base, &dir.path().join("cache"), &["acme/box"], std::slice::from_ref(&call))[0];
        assert!(through.get("error").is_none(), "{tool}: the link answers it: {through}");
    }
}

#[test]
fn the_link_answers_a_private_device_without_asking_the_service_and_refuses_what_would_be_sent() {
    let _home = ConfigHome::new();
    let (server, dir) = service();
    let devices = dir.path().join("kept");
    catalogue(&devices);
    let replies = linked_session(Some(&devices), &server.base, &dir.path().join("cache"), &["acme/box"], &[
        tool_call(1, "get_trust", json!({ "device": "acme/box" })),
        tool_call(2, "report_result", json!({ "device": "acme/box", "outcome": "works" })),
    ]);
    assert_eq!(replies[0]["result"]["structuredContent"]["private"], json!(true), "{}", replies[0]);
    assert_eq!(replies[1]["result"]["isError"], json!(true));
    let text = replies[1]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("private on this machine") && text.contains("private list"), "{text}");
    assert!(server.requests.lock().unwrap().iter().all(|s| !s.url.ends_with("/mcp")), "the service was asked nothing");
}

#[test]
fn the_link_asks_the_service_to_list_devices_unless_the_person_keeps_a_folder_of_drivers() {
    let _home = ConfigHome::new();
    let (server, dir) = service();
    let devices = dir.path().join("kept");
    catalogue(&devices);
    let asked = &linked_session(None, &server.base, &dir.path().join("cache"), &[], &[tool_call(1, "list_devices", json!({}))])[0];
    let text = asked["result"]["content"][0]["text"].as_str().unwrap();
    assert!(asked["result"]["isError"] == true && text.contains("asks the SynthHub server"), "the service is asked: {asked}");
    let kept = &linked_session(Some(&devices), &server.base, &dir.path().join("cache"), &[], &[tool_call(1, "list_devices", json!({}))])[0];
    assert_eq!(kept["result"]["structuredContent"]["count"], json!(2), "the kept folder answers locally: {kept}");
}
