//! The formats `export` offers are those of osd-formats, each with its name, media type and
//! extension from osd-formats, none written in osc-bridge; `export_device` stays read-only.

use crate::common::{files, temp, tool_call};
use osc_bridge::export::{export, Format};
use serde_json::{json, Value};

fn source(extra: &str) -> osd::Device {
    let text = format!(
        r#"{{"$schema":"https://w3id.org/osd/0.6/device.schema.json",
        "device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box"}},
        "cc_params":{{"entries":[{{"osc":"/cutoff","cc":74}},{{"osc":"/res","cc":71}}]}}{extra}}}"#
    );
    match osd::read(&text, osd::Kind::Device).unwrap_or_else(|e| panic!("{e}")) {
        osd::Descriptor::Device(d) => *d,
        _ => unreachable!(),
    }
}

const CREDIT: &str = r#","_sources":[{"type":"community-re","url":"https://example.org/box","author":"A maker","license":"CC-BY-4.0"}]"#;

#[test]
fn every_format_of_osd_formats_is_offered_by_its_name_with_its_media_type_and_extension() {
    for f in osd_formats::Format::ALL {
        let ours: Format = f.name().parse().unwrap_or_else(|e| panic!("{}: {e}", f.name()));
        assert_eq!(ours.media_type(), f.media_type(), "{}", f.name());
        assert_eq!(ours.extension(), f.extension(), "{}", f.name());
    }
}

#[test]
fn a_name_is_read_without_regard_to_case() {
    assert!("Deluge".parse::<Format>().is_ok());
    assert!("MIDNAM".parse::<Format>().is_ok());
}

#[test]
fn an_unknown_format_is_refused_with_the_list_of_osd_formats() {
    let e = "no-such-format".parse::<Format>().unwrap_err().to_string();
    assert!(e.contains("no-such-format"), "{e}");
    for f in osd_formats::Format::ALL {
        assert!(e.contains(f.name()), "{} is in the list: {e}", f.name());
    }
}

#[test]
fn the_deluge_export_of_a_device_with_cc_names_is_the_definition_file_osd_formats_gives() {
    let dev = source(CREDIT);
    let ours = export(&dev, "deluge".parse().unwrap()).unwrap();
    let resolved = osd::Resolved { device: dev.clone(), files: Vec::new(), capabilities: Vec::new(), inherited: Vec::new() };
    let theirs = osd_formats::export(&resolved, osd_formats::Format::Deluge).unwrap();
    assert_eq!(ours.value, theirs.value);
    assert!(ours.value.contains("<midiDevice>"), "{}", ours.value);
    assert!(ours.value.contains("74=") && ours.value.contains("/cutoff"), "{}", ours.value);
}

#[test]
fn a_format_osd_formats_adds_is_exported_without_a_case_of_its_own() {
    let dev = source(CREDIT);
    for f in [osd_formats::Format::Midnam, osd_formats::Format::OscQuery, osd_formats::Format::Deluge] {
        let out = export(&dev, f.name().parse().unwrap()).unwrap_or_else(|e| panic!("{}: {e}", f.name()));
        assert!(!out.value.is_empty(), "{}", f.name());
    }
}

#[test]
fn the_oscquery_export_writes_a_command_whose_address_takes_a_number_captured() {
    let dev = source(&format!(
        r#","commands":[{{"osc":["/box/track/",{{"capture":"n"}},"/volume"],"args":[{{"name":"n","type":"int","range":[1,8]}},{{"name":"value","type":"float","range":[0,1]}}]}}]{CREDIT}"#
    ));
    let out = export(&dev, "oscquery".parse().unwrap()).unwrap();
    assert!(out.value.contains("volume"), "the numbered command is in the namespace: {}", out.value);
    assert!(out.value.contains("track"), "{}", out.value);
}

#[test]
fn the_oscquery_export_keeps_a_plain_command_beside_a_numbered_one() {
    let dev = source(&format!(
        r#","commands":[{{"osc":"/box/play","args":[]}},{{"osc":["/box/track/",{{"capture":"n"}},"/mute"],"args":[{{"name":"n","type":"int","range":[1,8]}}]}}]{CREDIT}"#
    ));
    let out = export(&dev, "oscquery".parse().unwrap()).unwrap();
    assert!(out.value.contains("play"), "{}", out.value);
    assert!(out.value.contains("mute"), "{}", out.value);
}

// ---- the tool ------------------------------------------------------------------------------

fn call(root: &tempfile::TempDir, tool: &str, args: Value) -> Value {
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(root.path().to_path_buf()),
        default_target: "127.0.0.1:9".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
    };
    let input = format!("{}\n", tool_call(1, tool, args));
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, input.as_bytes(), &mut out).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    reply["result"].clone()
}

fn catalogue() -> tempfile::TempDir {
    let root = temp("export-formats");
    let text = format!(
        r#"{{"$schema":"https://w3id.org/osd/0.6/device.schema.json","device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box"}},
        "cc_params":{{"entries":[{{"osc":"/cutoff","cc":74}}]}}{CREDIT}}}"#
    );
    files(root.path(), &[("acme/box.json", &text)]);
    root
}

#[test]
fn export_device_gives_the_deluge_definition_with_its_media_type_and_file_name() {
    let root = catalogue();
    let r = call(&root, "export_device", json!({"device": "acme/box", "format": "deluge"}));
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(r["structuredContent"]["media_type"], "application/xml");
    assert!(r["structuredContent"]["file_name"].as_str().unwrap().ends_with(".xml"), "{r}");
    assert!(r["content"][0]["text"].as_str().unwrap().contains("<midiDevice>"), "{r}");
}

#[test]
fn export_device_refuses_an_unknown_format_with_the_list_of_formats() {
    let root = catalogue();
    let r = call(&root, "export_device", json!({"device": "acme/box", "format": "nope"}));
    assert_eq!(r["isError"], json!(true), "{r}");
    let text = r["content"][0]["text"].as_str().unwrap();
    for f in osd_formats::Format::ALL {
        assert!(text.contains(f.name()), "{} in {text}", f.name());
    }
}

#[test]
fn export_device_takes_no_consent_and_install_export_alone_carries_it() {
    let root = catalogue();
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(root.path().to_path_buf()),
        default_target: "127.0.0.1:9".parse().unwrap(),
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
    };
    let input = format!("{}\n", json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, input.as_bytes(), &mut out).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    let tools = reply["result"]["tools"].as_array().unwrap();
    let props = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("no tool {name}"))["inputSchema"]["properties"].clone();
    assert!(props("export_device").get("confirm_overwrite").is_none());
    let install = props("install_export");
    for k in ["device", "on", "confirm_overwrite"] {
        assert!(install.get(k).is_some(), "install_export takes {k}: {install}");
    }
    let export_tool = tools.iter().find(|t| t["name"] == "export_device").unwrap();
    assert_eq!(export_tool["annotations"]["readOnlyHint"], json!(true));
    let install_tool = tools.iter().find(|t| t["name"] == "install_export").unwrap();
    assert_ne!(install_tool["annotations"]["readOnlyHint"], json!(true));
}
