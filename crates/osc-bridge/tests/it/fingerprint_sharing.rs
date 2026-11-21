//! Naming a device sends its fingerprint to `POST /v1/fingerprints`: never for a private device or
//! offline, with no serial number, host name or local measure in the body; no consent is asked or
//! kept (nothing is written to the settings file), and a failure of the service leaves the
//! fingerprint here, with no retry.

use osc_bridge::online::{FingerprintBody, Online, Settings, Sharing};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};

const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

struct Fake {
    base: String,
    posts: Arc<Mutex<Vec<(String, Value)>>>,
}

/// A local service answering every request with `status` and `reply`, logging the POSTs.
fn fake(status: u16, reply: &'static str) -> Fake {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let posts: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let log = posts.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
            if req.method().as_str() == "POST" {
                log.lock().unwrap().push((req.url().to_string(), serde_json::from_str(&body).unwrap_or(Value::Null)));
            }
            let _ = req.respond(tiny_http::Response::from_string(reply).with_status_code(status));
        }
    });
    Fake { base, posts }
}

fn client(fake: &Fake, dir: &Path) -> Online {
    std::fs::write(dir.join("user"), ACCOUNT).unwrap();
    super::common::online_in(&fake.base, dir)
}

/// A learned fingerprint, with a serial number, a host name and local things that must not leave.
fn learned() -> Value {
    json!({
        "$schema": "https://w3id.org/osd/0.6/fingerprint.schema.json",
        "observed": "studio-pc.local /home/anna/rig",
        "via": "native",
        "hostname": "studio-pc",
        "serial": "SN-998877",
        "usb": { "vendor_id": 7285, "product_id": 523, "serial": "SN-998877" },
        "identity_reply": "F0 7E 00 06 02 00 20 6B 04 00 02 00 01 00 F7",
        "ports": { "inputs": [["minilab3", "midi"]], "outputs": [["minilab3", "midi"]] },
        "sends": { "active_sensing": true },
        "controls": [{ "cc": 74, "channel": 1 }],
        "measured": { "identity_reply_ms": 12.5 }
    })
}

fn settings_path(dir: &Path) -> std::path::PathBuf {
    dir.join("osc-bridge").join("settings.toml")
}

#[test]
fn a_named_fingerprint_is_posted_to_the_fingerprints_endpoint() {
    let dir = super::common::temp("fp-send");
    let fake = fake(200, "{}");
    let online = client(&fake, dir.path());
    let result = online.share_fingerprint(&settings_path(dir.path()), "MiniLab 3", &learned());
    assert_eq!(result, Sharing::Sent);
    let posts = fake.posts.lock().unwrap();
    assert_eq!(posts.len(), 1, "{posts:?}");
    assert_eq!(posts[0].0, "/v1/fingerprints");
    assert_eq!(posts[0].1["name"], "MiniLab 3");
}

#[test]
fn the_body_keeps_the_identification_fields_and_the_name_only() {
    let body = serde_json::to_value(FingerprintBody::new("MiniLab 3", &learned())).unwrap();
    assert_eq!(body, json!({
        "name": "MiniLab 3",
        "usb": { "vendor_id": 7285, "product_id": 523 },
        "identity_reply": "F0 7E 00 06 02 00 20 6B 04 00 02 00 01 00 F7",
        "ports": { "inputs": [["minilab3", "midi"]], "outputs": [["minilab3", "midi"]] },
        "sends": { "active_sensing": true },
        "controls": [{ "cc": 74, "channel": 1 }],
    }));
}

#[test]
fn a_field_the_device_did_not_give_is_absent_from_the_body() {
    let fp = json!({ "usb": { "vendor_id": 1, "product_id": 2 }, "ports": { "outputs": [["x"]] } });
    let body = serde_json::to_value(FingerprintBody::new("X", &fp)).unwrap();
    assert_eq!(body, json!({ "name": "X", "usb": { "vendor_id": 1, "product_id": 2 }, "ports": { "outputs": [["x"]] } }));
}

#[test]
fn the_posted_body_holds_no_serial_number_no_host_name_and_nothing_local() {
    let dir = super::common::temp("fp-clean");
    let fake = fake(200, "{}");
    let online = client(&fake, dir.path());
    assert_eq!(online.share_fingerprint(&settings_path(dir.path()), "MiniLab 3", &learned()), Sharing::Sent);
    let sent = fake.posts.lock().unwrap()[0].1.to_string();
    for secret in ["SN-998877", "studio-pc", "anna", "serial", "hostname", "measured", "identity_reply_ms", "observed"] {
        assert!(!sent.contains(secret), "{secret} left in {sent}");
    }
}

#[test]
fn nothing_is_sent_for_a_device_whose_given_name_a_private_entry_covers() {
    // (private entry, name the person gave): the id or the slug of the name is covered.
    for (private, name) in [("MiniLab 3", "MiniLab 3"), ("minilab-3", "MiniLab 3"), ("arturia/*", "arturia/minilab-3")] {
        let dir = super::common::temp("fp-private");
        let fake = fake(200, "{}");
        let online = client(&fake, dir.path());
        let file = settings_path(dir.path());
        Settings { private: vec![private.into()], ..Settings::default() }.save(&file).unwrap();
        let result = online.share_fingerprint(&file, name, &learned());
        assert!(matches!(result, Sharing::Withheld(_)), "{private}: {result:?}");
        assert!(fake.posts.lock().unwrap().is_empty(), "{private}");
    }
}

#[test]
fn sending_a_fingerprint_writes_nothing_to_the_settings_file() {
    let dir = super::common::temp("fp-no-settings");
    let fake = fake(200, "{}");
    let online = client(&fake, dir.path());
    let file = settings_path(dir.path());
    assert_eq!(online.share_fingerprint(&file, "MiniLab 3", &learned()), Sharing::Sent);
    assert!(!file.exists(), "the settings file is not created");
    Settings { private: vec!["korg/*".into()], ..Settings::default() }.save(&file).unwrap();
    let before = std::fs::read_to_string(&file).unwrap();
    assert_eq!(online.share_fingerprint(&file, "MiniLab 3", &learned()), Sharing::Sent);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}

#[test]
fn nothing_is_sent_when_the_offline_setting_is_on() {
    let dir = super::common::temp("fp-offline");
    let fake = fake(200, "{}");
    let online = client(&fake, dir.path());
    let file = settings_path(dir.path());
    Settings { offline: true, ..Settings::default() }.save(&file).unwrap();
    assert!(matches!(online.share_fingerprint(&file, "MiniLab 3", &learned()), Sharing::Withheld(_)));
    assert!(fake.posts.lock().unwrap().is_empty());
}

#[test]
fn a_service_error_leaves_the_fingerprint_here_says_so_and_is_not_retried() {
    let dir = super::common::temp("fp-http-error");
    let fake = fake(500, "{\"error\":\"boom\"}");
    let online = client(&fake, dir.path());
    let result = online.share_fingerprint(&settings_path(dir.path()), "MiniLab 3", &learned());
    let Sharing::Failed(why) = result else { panic!("{result:?}") };
    assert!(why.contains("500") || why.contains("boom"), "{why}");
    assert_eq!(fake.posts.lock().unwrap().len(), 1, "no retry");
}

#[test]
fn a_quota_answer_is_told_as_the_service_says_it() {
    let dir = super::common::temp("fp-429");
    let fake = fake(429, "Daily quota reached: please update osc-bridge to carry on.");
    let online = client(&fake, dir.path());
    let result = online.share_fingerprint(&settings_path(dir.path()), "MiniLab 3", &learned());
    let Sharing::Failed(why) = result else { panic!("{result:?}") };
    assert!(why.contains("Daily quota reached"), "{why}");
    assert_eq!(fake.posts.lock().unwrap().len(), 1);
}

#[test]
fn an_unreachable_service_is_a_failure_not_a_panic() {
    let dir = super::common::temp("fp-unreachable");
    std::fs::write(dir.path().join("user"), ACCOUNT).unwrap();
    let online = super::common::online_in("http://127.0.0.1:1", dir.path());
    let result = online.share_fingerprint(&settings_path(dir.path()), "MiniLab 3", &learned());
    assert!(matches!(result, Sharing::Failed(_)), "{result:?}");
}

#[test]
fn detect_hardware_publishes_the_name_and_the_share_arguments_and_says_what_is_sent() {
    let dir = super::common::temp("fp-schema");
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(dir.path().to_path_buf()),
        default_target: "127.0.0.1:9000".parse().unwrap(),
        online: false,
        private: vec![],
    };
    let input = format!("{}\n", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, "http://127.0.0.1:1", None, input.as_bytes(), &mut out).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    let tool = reply["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "detect_hardware").unwrap().clone();
    let props = &tool["inputSchema"]["properties"];
    assert_eq!(props["share_fingerprint"]["type"], "string", "{props}");
    assert_eq!(props["device_name"]["type"], "string", "{props}");
    let what = format!("{} {}", props["share_fingerprint"]["description"], tool["description"]).to_lowercase();
    for word in ["usb", "identity_reply", "ports", "serial number", "host name", "/v1/fingerprints"] {
        assert!(what.contains(word), "the description must say what is sent: missing {word}: {what}");
    }
}

#[test]
fn the_privacy_policy_says_what_is_sent_when_and_that_private_and_offline_stop_it() {
    let doc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../PRIVACY.md")).unwrap().to_lowercase();
    let start = doc.find("## a copy installed on your machine").expect("section");
    let rest = &doc[start + 3..];
    let section = &rest[..rest.find("\n## ").unwrap_or(rest.len())];
    for word in ["device fingerprint", "serial number", "host name", "private", "offline"] {
        assert!(section.contains(word), "missing `{word}` in the section");
    }
}
