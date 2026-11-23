//! The learn mode of `detect_hardware`, through the MCP server: published in the
//! tool's schema, and a listening time outside 1000-30000 ms refused before any
//! MIDI port is touched.

use serde_json::{json, Value};

fn ask(request: Value) -> String {
    let dir = super::common::temp("learn-mode");
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(dir.path().to_path_buf()),
        default_target: "127.0.0.1:9000".parse().unwrap(),
        online: false,
        private: vec![],
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
    };
    let input = format!("{request}\n");
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, input.as_bytes(), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn detect_hardware_publishes_the_learn_mode_and_its_listening_time() {
    let out = ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
    let reply: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    let tool = reply["result"]["tools"].as_array().unwrap().iter()
        .find(|t| t["name"] == "detect_hardware").unwrap().clone();
    let props = &tool["inputSchema"]["properties"];
    assert_eq!(props["learn"]["type"], "boolean", "{props}");
    assert_eq!(props["listen_ms"]["type"], "integer", "{props}");
    let listen = props["listen_ms"]["description"].as_str().unwrap();
    assert!(listen.contains("1000") && listen.contains("30000") && listen.contains("5000"), "{listen}");
    let device = &tool["outputSchema"]["properties"]["devices"]["items"]["properties"];
    assert!(device.get("fingerprint").is_some() && device.get("measured").is_some(), "{device}");
    assert_eq!(device["inputs"]["type"], "array", "one element per device lists all its input names: {device}");
    assert_eq!(device["outputs"]["type"], "array", "one element per device lists all its output names: {device}");
    let measured = device["measured"]["description"].as_str().unwrap();
    assert!(measured.contains("identity_reply_port") && measured.contains("periods_port"), "{measured}");
}

#[test]
fn detect_hardware_in_learn_mode_refuses_a_listening_time_outside_the_interval() {
    for bad in [999, 30001] {
        let out = ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "detect_hardware", "arguments": { "learn": true, "listen_ms": bad } } }));
        assert!(out.contains("1000") && out.contains("30000"), "{bad}: {out}");
        assert!(!out.contains("\"devices\""), "{bad}: scanned anyway: {out}");
    }
}
