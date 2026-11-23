//! The whole path of a device with no driver to its first driver, on a fake MIDI device: a
//! virtual port pair this test makes through midir (the ALSA sequencer) and speaks to alone,
//! never to a port it did not create (real devices may be plugged into the night machine).
//! The learn mode names the next step, the mappings heard make a valid first driver, and the
//! two tools forward it to a fake service.

#![cfg(target_os = "linux")]

use midir::os::unix::{VirtualInput, VirtualOutput};
use midir::{MidiInput, MidiOutput};
use osc_bridge::detect::{learn_units, Unit};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const REPLY: [u8; 17] = [0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x00, 0x20, 0x6B, 0x02, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0xF7];
const ID: &str = "0123456789abcdef0123456789abcdef";

/// The fake device: it answers the identity request and moves a control (CC 74) over and over.
/// Returns the unit of its two virtual ports, as the bridge sees them, and a stop flag.
fn fake_device(marker: &str) -> (Unit, Arc<Mutex<bool>>) {
    let stop = Arc::new(Mutex::new(false));
    let out = MidiOutput::new(marker).unwrap();
    let conn = Arc::new(Mutex::new(out.create_virtual(&format!("{marker} out")).unwrap()));
    let on_request = conn.clone();
    let input = MidiInput::new(marker).unwrap();
    let listening = input.create_virtual(&format!("{marker} in"), move |_, msg, _| {
        if msg == osc_bridge::recognition::IDENTITY_REQUEST { let _ = on_request.lock().unwrap().send(&REPLY); }
    }, ()).unwrap();
    let (turning, flag) = (conn.clone(), stop.clone());
    std::thread::spawn(move || {
        let _keep = listening;
        while !*flag.lock().unwrap() {
            let _ = turning.lock().unwrap().send(&[0xB0, 74, 64]);
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    std::thread::sleep(Duration::from_millis(300));
    // The ports the bridge lists for it, and only them.
    let seen = osc_bridge::detect::units(&osc_bridge::detect::observe(false, Duration::ZERO, &[], &[]).unwrap());
    let mut mine = Unit { inputs: vec![], outputs: vec![], in_indexes: vec![], out_indexes: vec![], usb: None,
        client_kind: osc_bridge::detect::ClientKind::Unknown };
    for u in seen {
        for (n, i) in u.inputs.iter().zip(&u.in_indexes).filter(|(n, _)| n.contains(marker)) {
            mine.inputs.push(n.clone());
            mine.in_indexes.push(*i);
        }
        for (n, i) in u.outputs.iter().zip(&u.out_indexes).filter(|(n, _)| n.contains(marker)) {
            mine.outputs.push(n.clone());
            mine.out_indexes.push(*i);
        }
    }
    assert!(!mine.inputs.is_empty() && !mine.outputs.is_empty(), "the virtual ports are not listed: {mine:?}");
    (mine, stop)
}

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

#[test]
fn an_agent_goes_from_a_device_with_no_driver_to_a_first_driver_on_a_fake_midi_device() {
    let marker = format!("nbnight{}", std::process::id());
    let (mine, stop) = fake_device(&marker);

    // 1. Learn only this device: the result tells the next step.
    let learned = learn_units(std::slice::from_ref(&mine), Duration::from_millis(1500)).unwrap();
    *stop.lock().unwrap() = true;
    assert_eq!(learned.len(), 1);
    let fp = &learned[0].1.fingerprint;
    assert_eq!(fp["identity_reply"], "F0 7E 7F 06 02 00 20 6B 02 00 04 00 01 00 04 00 F7", "{fp}");
    let controls = fp["controls"].as_array().unwrap_or_else(|| panic!("no controls heard: {fp}"));
    let cc = controls.iter().find(|c| c["cc"] == 74).unwrap_or_else(|| panic!("CC 74 not heard: {controls:?}"));
    let empty = tempfile::tempdir().unwrap();
    let order = osc_bridge::synthub::catalogue::tier_rank;
    let report = osc_bridge::mcp::learn_report(&learned, &osc_bridge::mcp::DeviceIndex::build(empty.path(), order), Duration::from_millis(1500),
        &osc_bridge::mcp::ServiceMatches::NotLinked, order);
    let step = report["structuredContent"]["devices"][0]["next_step"].as_str().unwrap_or_else(|| panic!("{report}"));
    assert!(step.contains("report_mapping"), "{step}");

    // 2. The mappings heard, named by the name the person gave, and the first driver, go on.
    let name = "Night Synth";
    let mapping = json!({ "device": name, "label": "Cutoff", "cc": cc["cc"], "channel": 1 });
    let driver = json!({
        "$schema": "https://w3id.org/osd/0.5/device.schema.json",
        "device": { "name": name, "vendor": "Night", "osc_prefix": "/night_synth" },
        "cc_params": { "channel": 0, "entries": [{ "osc": "/cutoff", "cc": 74 }] },
    });
    let text = driver.to_string();
    osd::read(&text, osd::Kind::Device).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let source = "mapped on the device by report_mapping; license CC0-1.0";

    let (base, calls) = fake_service();
    let _config = super::common::ConfigHome::new();
    let home = tempfile::tempdir().unwrap();
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: None,
        default_target: "127.0.0.1:9".parse().unwrap(),
        online: true,
        private: vec![],
        tier_order: osc_bridge::synthub::catalogue::tier_rank,
    };
    let call = |id: u64, tool: &str, args: &Value| json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": tool, "arguments": args } });
    let input = format!("{}\n{}\n", call(1, "report_mapping", &mapping),
        call(2, "submit_driver", &json!({ "driver": driver, "source": source })));
    let mut out = Vec::new();
    osc_bridge::synthub::mcp::serve(opts, &base, Some(home.path().join("cache")), input.as_bytes(), &mut out).unwrap();
    for line in String::from_utf8(out).unwrap().lines() {
        let reply: Value = serde_json::from_str(line).unwrap();
        assert!(reply.get("error").is_none() && reply["result"]["isError"] != true, "{reply}");
    }
    let seen = calls.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0]["params"]["arguments"], mapping);
    assert_eq!(seen[1]["params"]["arguments"]["driver"], driver);
}
