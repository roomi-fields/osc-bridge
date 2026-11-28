//! The library publishes what the program built outside the workspace reads of the bridge: this
//! test is an external client, so it sees only what is `pub`.

use osc_bridge::detect::{client_key, client_number, ClientKey};
use osc_bridge::mcp::{
    build_index, call_local, detect_port, device_arg, format_detection, port_answer, proofs_json,
    software_cards_only, tool_names, unlisted_text, Known, McpOptions,
};
use osc_bridge::recognition::{Nature, Proof};
use serde_json::{json, Value};
use std::path::Path;

fn devices() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/devices"))
}

fn options() -> McpOptions {
    McpOptions {
        tier_order: |_| 0,
        devices_dir: Some(devices().to_path_buf()),
        default_target: "127.0.0.1:9000".parse().unwrap(),
        detect_mode: None,
    }
}

#[test]
fn client_number_reads_the_final_alsa_pair() {
    assert_eq!(client_number("Minilogue MIDI 1 20:0"), Some(20));
    assert_eq!(client_number("Minilogue MIDI 1"), None);
    assert_eq!(client_number("Box x:1"), None);
}

#[test]
fn client_key_groups_ports_of_one_client() {
    assert!(client_key("Box MIDI 1 20:0") == client_key("Box MIDI 2 20:1"));
    assert!(client_key("Box MIDI 1 20:0") != client_key("Box MIDI 1 21:0"));
    assert!(matches!(client_key("Box 20:0"), ClientKey::Number(20)));
    assert!(matches!(client_key("Virtual Thing"), ClientKey::Name(_)));
}

#[test]
fn device_arg_reads_device_and_refuses_the_former_slug() {
    assert_eq!(device_arg(&json!({ "device": "acme/box" })), Ok("acme/box"));
    assert!(device_arg(&json!({ "slug": "box" })).unwrap_err().contains("no longer exists"));
    assert!(device_arg(&json!({})).unwrap_err().contains("missing device"));
}

#[test]
fn tool_names_lists_the_sixteen_local_tools() {
    let names = tool_names(&options());
    assert_eq!(names.len(), 16);
    assert!(names.contains(&"send") && names.contains(&"detect_hardware"));
}

#[test]
fn port_answer_gives_the_devices_in_the_order_of_the_service_each_with_its_own_confirmation_fields() {
    let first = json!({ "id": "z/last-by-name", "name": "Z", "probability": 0.2, "confirmations": 2, "disputed": true,
        "state": "confirming", "state_label": "in disagreement" });
    let second = json!({ "id": "a/b", "name": "B", "probability": 0.9 });
    let got = port_answer(&json!({ "devices": [first, second] }));
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!((&got[0]["id"], &got[1]["id"]), (&json!("z/last-by-name"), &json!("a/b")), "{got:?}");
    for field in ["name", "probability", "confirmations", "disputed", "state", "state_label"] {
        assert_eq!(got[0][field], first[field], "{field}: {got:?}");
    }
    for field in ["confirmations", "disputed", "state", "state_label"] {
        assert!(got[1].get(field).is_none(), "{field} belongs to the other device: {got:?}");
    }
    assert!(got.iter().all(|d| d["kind"] == "candidate"), "{got:?}");
}

#[test]
fn port_answer_does_not_read_the_learned_list() {
    let got = port_answer(&json!({
        "devices": [{ "id": "a/b" }],
        "learned": [{ "id": "a/b", "confirmations": 2, "disputed": false, "state": "confirming", "state_label": "being confirmed" }],
    }));
    assert_eq!(got.len(), 1, "{got:?}");
    for field in ["confirmations", "disputed", "state", "state_label"] {
        assert!(got[0].get(field).is_none(), "{field}: {got:?}");
    }
}

#[test]
fn port_answer_gives_the_named_unlisted_devices_only_when_there_is_no_device() {
    let unlisted = json!([{ "vendor": "Acme" }, { "firmware": "1" }, { "model": "Thing" }]);
    for answer in [json!({ "unlisted": unlisted }), json!({ "devices": [], "unlisted": unlisted })] {
        let got = port_answer(&answer);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.iter().all(|u| u["kind"] == "unlisted"), "{got:?}");
    }
    let got = port_answer(&json!({ "devices": [{ "id": "a/b" }], "unlisted": unlisted }));
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["id"], "a/b", "{got:?}");
}

#[test]
fn port_answer_names_no_device_from_the_former_candidates_list() {
    assert!(port_answer(&json!({ "candidates": [{ "id": "a/b" }] })).is_empty());
}

#[test]
fn unlisted_text_names_maker_model_firmware_and_confirmations() {
    let u = json!({ "vendor": "Acme", "model": "Box", "firmware": "1.2", "confirmations": 3 });
    assert_eq!(unlisted_text(&u), "Acme Box, firmware 1.2, 3 confirmation(s)");
    assert_eq!(unlisted_text(&json!({ "vendor": "Acme" })), "Acme");
}

#[test]
fn format_detection_says_when_no_port_is_found() {
    assert!(format_detection(&[], true).contains("No MIDI port found."));
    let one = json!({ "port": "Box 20:0", "usb": { "vendor_id": 4661, "product_id": 2 } });
    let text = format_detection(&[one], false);
    assert!(text.contains("Box 20:0") && text.contains("usb: 1235:0002"), "{text}");
}

#[test]
fn detect_port_carries_usb_ids_only_when_read() {
    let with = detect_port("Box", vec!["F0".into()], Some((1, 2)));
    assert_eq!(with["usb"], json!({ "vendor_id": 1, "product_id": 2 }));
    assert_eq!(with["replies_hex"], json!(["F0"]));
    assert!(detect_port("Box", vec![], None).get("usb").is_none());
}

#[test]
fn proofs_json_publishes_nature_and_kind() {
    let p = [Proof { nature: Nature::Strong, kind: "usb id" }];
    assert_eq!(proofs_json(&p), json!([{ "nature": "strong", "kind": "usb id" }]));
}

#[test]
fn build_index_walks_the_folder_and_software_cards_only_keeps_software() {
    let index = build_index(devices(), &options());
    assert!(!index.entries.is_empty());
    let software = index.entries.iter().find(|e| e.kind == "software").expect("a software card").id.clone();
    let hardware = index.entries.iter().find(|e| e.kind != "software").expect("a hardware card").id.clone();
    let kept = software_cards_only(&index, vec![json!({ "id": software }), json!({ "id": hardware }), json!({ "id": "none/none" })]);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0]["id"], software.as_str());
}

#[test]
fn call_local_answers_a_catalogue_tool_and_refuses_an_unknown_one() {
    let opts = options();
    let index = build_index(devices(), &opts);
    let status = |_: &str| -> Option<String> { None };
    let private = |_: &str| false;
    let known = Known { trust: Value::Null, candidates: vec![], status: &status, private: &private };
    let open = || -> anyhow::Result<Box<dyn osc_bridge::midi_port::Midi>> { anyhow::bail!("no MIDI") };
    let listed = call_local("list_devices", &json!({}), &opts, &index, &open, &known).unwrap();
    assert!(listed.to_string().contains("acme/box"));
    let err = call_local("nope", &json!({}), &opts, &index, &open, &known).unwrap_err();
    assert_eq!(err, "unknown tool: nope");
    let slug = call_local("list_routes", &json!({ "slug": "x" }), &opts, &index, &open, &known).unwrap_err();
    assert!(slug.contains("no longer exists"));
}

#[test]
fn detection_entry_points_are_published() {
    let _ = osc_bridge::mcp::detect_tool;
    let _ = osc_bridge::mcp::detect_hardware;
}
