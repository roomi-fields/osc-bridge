//! The tool lists osc-bridge publishes (the bundle manifest, docs/MCP.md, the
//! two READMEs) name exactly the tools the code offers offline, and the
//! manifest says what the code's schemas say. Online tools (`take_sound`,
//! `load_preset`, the server's relayed tools) have their schemas from the
//! server and are in no local list; server.json carries no list.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;

fn root_file(name: &str) -> (String, String) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    // A checkout may carry CRLF line endings (Windows); the lists are read with \n.
    (name.to_string(), text.replace("\r\n", "\n"))
}

/// name -> description of every tool the code offers with no link to the server.
fn offline_tools() -> BTreeMap<String, String> {
    let dir = super::common::temp("tool-lists");
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(dir.path().to_path_buf()),
        default_target: "127.0.0.1:9000".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: None,
    };
    let request = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let input = format!("{request}\n");
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, input.as_bytes(), &mut out).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    reply["result"]["tools"].as_array().unwrap().iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t["description"].as_str().unwrap().to_string()))
        .collect()
}

fn names(tools: &BTreeMap<String, String>) -> BTreeSet<String> {
    tools.keys().cloned().collect()
}

fn assert_same_tools(file: &str, listed: &BTreeSet<String>, defined: &BTreeSet<String>) {
    let missing: Vec<_> = defined.difference(listed).collect();
    let extra: Vec<_> = listed.difference(defined).collect();
    assert!(missing.is_empty() && extra.is_empty(),
        "{file}: tools missing from the list: {missing:?}; tools listed that the code does not define: {extra:?}");
}

const NUMBER_WORDS: [&str; 21] = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
    "twenty", "thirty"];

fn has_number_word(text: &str) -> bool {
    text.split(|c: char| !c.is_alphabetic()).any(|w| NUMBER_WORDS.contains(&w.to_lowercase().as_str()))
}

/// The paragraph of a README that lists the tools (the one naming `get_device_docs`).
fn tools_paragraph(file: &str, text: &str) -> String {
    text.split("\n\n").find(|p| p.contains("`get_device_docs`"))
        .unwrap_or_else(|| panic!("{file}: no paragraph lists the tools")).to_string()
}

fn backticked_names(paragraph: &str) -> BTreeSet<String> {
    paragraph.split('`').skip(1).step_by(2)
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .map(str::to_string).collect()
}

#[test]
fn bundle_manifest_names_every_offline_tool_and_no_other() {
    let (file, text) = root_file("mcpb/manifest.json");
    let manifest: Value = serde_json::from_str(&text).unwrap();
    let listed: BTreeSet<String> = manifest["tools"].as_array().unwrap().iter()
        .map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_same_tools(&file, &listed, &names(&offline_tools()));
}

#[test]
fn bundle_manifest_describes_each_tool_as_the_code_does() {
    let (file, text) = root_file("mcpb/manifest.json");
    let manifest: Value = serde_json::from_str(&text).unwrap();
    let defined = offline_tools();
    for t in manifest["tools"].as_array().unwrap() {
        let name = t["name"].as_str().unwrap();
        if let Some(code) = defined.get(name) {
            assert_eq!(t["description"].as_str(), Some(code.as_str()), "{file}: description of {name} differs from the code's");
        }
    }
}

/// The input schema (`required` names, property names) of an offline tool, from the code.
fn code_schema(tool: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let dir = super::common::temp("tool-schemas");
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(dir.path().to_path_buf()),
        default_target: "127.0.0.1:9000".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
        detect_mode: None,
    };
    let request = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let input = format!("{request}\n");
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, input.as_bytes(), &mut out).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    let t = reply["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == tool).unwrap().clone();
    schema_names(&t)
}

fn schema_names(tool: &Value) -> (BTreeSet<String>, BTreeSet<String>) {
    let s = &tool["inputSchema"];
    let required = s["required"].as_array().into_iter().flatten().map(|v| v.as_str().unwrap().to_string()).collect();
    let props = s["properties"].as_object().unwrap().keys().cloned().collect();
    (required, props)
}

#[test]
fn the_preset_tools_require_a_device_and_offer_no_pause_nor_request_argument_in_code_and_manifest() {
    let (_, text) = root_file("mcpb/manifest.json");
    let manifest: Value = serde_json::from_str(&text).unwrap();
    for tool in ["restore_presets", "backup_presets"] {
        let (required, props) = code_schema(tool);
        assert!(required.contains("device"), "{tool} (code): device is required");
        assert!(!props.contains("gap_ms") && !props.contains("request_hex"), "{tool} (code): {props:?}");
        let m = manifest["tools"].as_array().unwrap().iter().find(|t| t["name"] == tool).unwrap();
        assert_eq!(schema_names(m), (required, props), "{tool}: the manifest says what the code's schema says");
    }
    let (_, restore) = code_schema("restore_presets");
    assert!(restore.contains("bank") && restore.contains("number"), "{restore:?}");
}

#[test]
fn mcp_guide_tool_table_names_every_offline_tool_and_no_other() {
    let (file, text) = root_file("docs/MCP.md");
    let lines: Vec<&str> = text.lines().collect();
    let head = lines.iter().position(|l| l.starts_with("| Tool |")).unwrap_or_else(|| panic!("{file}: no tool table"));
    let listed: BTreeSet<String> = lines[head + 2..].iter().take_while(|l| l.starts_with('|'))
        .filter_map(|l| l.split('|').nth(1)).map(|c| c.trim().trim_matches('`').to_string()).collect();
    assert_same_tools(&file, &listed, &names(&offline_tools()));
}

#[test]
fn mcp_guide_tool_table_title_carries_no_written_number() {
    let (file, text) = root_file("docs/MCP.md");
    let lines: Vec<&str> = text.lines().collect();
    let head = lines.iter().position(|l| l.starts_with("| Tool |")).unwrap();
    let title = lines[..head].iter().rev().find(|l| l.starts_with('#')).unwrap();
    assert!(!has_number_word(title) && !title.chars().any(|c| c.is_ascii_digit()), "{file}: title {title:?} writes a number");
}

#[test]
fn readme_tool_paragraphs_name_every_offline_tool_without_a_written_number() {
    let defined = names(&offline_tools());
    for name in ["README.md", "npm/README.md"] {
        let (file, text) = root_file(name);
        let paragraph = tools_paragraph(&file, &text);
        assert_same_tools(&file, &backticked_names(&paragraph), &defined);
        assert!(!has_number_word(&paragraph), "{file}: the tools paragraph writes a number in letters");
    }
}
