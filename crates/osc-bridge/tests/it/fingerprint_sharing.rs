//! Giving a device's maker, model and firmware sends its fingerprint to `POST /v1/fingerprints`: never for a private device or
//! offline, with no serial number, host name or local measure in the body; no consent is asked or
//! kept (nothing is written to the settings file), and a failure of the service leaves the
//! fingerprint here, with no retry.

use osc_bridge::synthub::online::{FingerprintBody, Online, Settings, Sharing};
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
    let result = online.share_fingerprint(&settings_path(dir.path()), "Arturia", "MiniLab 3", "1.0.2.3", &learned());
    assert_eq!(result, Sharing::Sent);
    let posts = fake.posts.lock().unwrap();
    assert_eq!(posts.len(), 1, "{posts:?}");
    assert_eq!(posts[0].0, "/v1/fingerprints");
    assert_eq!(posts[0].1["device"], json!({ "vendor": "Arturia", "model": "MiniLab 3", "firmware": "1.0.2.3" }));
    assert!(posts[0].1.get("name").is_none(), "no free name: {}", posts[0].1);
}

#[test]
fn the_body_keeps_the_identification_fields_and_the_maker_model_and_firmware_only() {
    let body = serde_json::to_value(FingerprintBody::new("Arturia", "MiniLab 3", "1.0.2.3", &learned())).unwrap();
    assert_eq!(body, json!({
        "device": { "vendor": "Arturia", "model": "MiniLab 3", "firmware": "1.0.2.3" },
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
    let body = serde_json::to_value(FingerprintBody::new("Acme", "X", "2.0", &fp)).unwrap();
    assert_eq!(body, json!({ "device": { "vendor": "Acme", "model": "X", "firmware": "2.0" }, "usb": { "vendor_id": 1, "product_id": 2 }, "ports": { "outputs": [["x"]] } }));
}

#[test]
fn the_posted_body_holds_no_serial_number_no_host_name_and_nothing_local() {
    let dir = super::common::temp("fp-clean");
    let fake = fake(200, "{}");
    let online = client(&fake, dir.path());
    assert_eq!(online.share_fingerprint(&settings_path(dir.path()), "Arturia", "MiniLab 3", "1.0.2.3", &learned()), Sharing::Sent);
    let sent = fake.posts.lock().unwrap()[0].1.to_string();
    for secret in ["SN-998877", "studio-pc", "anna", "serial", "hostname", "measured", "identity_reply_ms", "observed"] {
        assert!(!sent.contains(secret), "{secret} left in {sent}");
    }
}

#[test]
fn nothing_is_sent_for_a_device_whose_maker_and_model_a_private_entry_covers() {
    // The id of the device is `maker/model`, slugged: a private entry covers it whole or by maker.
    for private in ["arturia/minilab-3", "arturia/*"] {
        let dir = super::common::temp("fp-private");
        let fake = fake(200, "{}");
        let online = client(&fake, dir.path());
        let file = settings_path(dir.path());
        Settings { private: vec![private.into()], ..Settings::default() }.save(&file).unwrap();
        let result = online.share_fingerprint(&file, "Arturia", "MiniLab 3", "1.0.2.3", &learned());
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
    assert_eq!(online.share_fingerprint(&file, "Arturia", "MiniLab 3", "1.0.2.3", &learned()), Sharing::Sent);
    assert!(!file.exists(), "the settings file is not created");
    Settings { private: vec!["korg/*".into()], ..Settings::default() }.save(&file).unwrap();
    let before = std::fs::read_to_string(&file).unwrap();
    assert_eq!(online.share_fingerprint(&file, "Arturia", "MiniLab 3", "1.0.2.3", &learned()), Sharing::Sent);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}

#[test]
fn nothing_is_sent_when_the_offline_setting_is_on() {
    let dir = super::common::temp("fp-offline");
    let fake = fake(200, "{}");
    let online = client(&fake, dir.path());
    let file = settings_path(dir.path());
    Settings { offline: true, ..Settings::default() }.save(&file).unwrap();
    assert!(matches!(online.share_fingerprint(&file, "Arturia", "MiniLab 3", "1.0.2.3", &learned()), Sharing::Withheld(_)));
    assert!(fake.posts.lock().unwrap().is_empty());
}

#[test]
fn a_service_error_leaves_the_fingerprint_here_says_so_and_is_not_retried() {
    let dir = super::common::temp("fp-http-error");
    let fake = fake(500, "{\"error\":\"boom\"}");
    let online = client(&fake, dir.path());
    let result = online.share_fingerprint(&settings_path(dir.path()), "Arturia", "MiniLab 3", "1.0.2.3", &learned());
    let Sharing::Failed(why) = result else { panic!("{result:?}") };
    assert!(why.contains("500") || why.contains("boom"), "{why}");
    assert_eq!(fake.posts.lock().unwrap().len(), 1, "no retry");
}

#[test]
fn a_quota_answer_is_told_as_the_service_says_it() {
    let dir = super::common::temp("fp-429");
    let fake = fake(429, "Daily quota reached: please update osc-bridge to carry on.");
    let online = client(&fake, dir.path());
    let result = online.share_fingerprint(&settings_path(dir.path()), "Arturia", "MiniLab 3", "1.0.2.3", &learned());
    let Sharing::Failed(why) = result else { panic!("{result:?}") };
    assert!(why.contains("Daily quota reached"), "{why}");
    assert_eq!(fake.posts.lock().unwrap().len(), 1);
}

#[test]
fn an_unreachable_service_is_a_failure_not_a_panic() {
    let dir = super::common::temp("fp-unreachable");
    std::fs::write(dir.path().join("user"), ACCOUNT).unwrap();
    let online = super::common::online_in("http://127.0.0.1:1", dir.path());
    let result = online.share_fingerprint(&settings_path(dir.path()), "Arturia", "MiniLab 3", "1.0.2.3", &learned());
    assert!(matches!(result, Sharing::Failed(_)), "{result:?}");
}

#[test]
fn detect_hardware_publishes_the_maker_model_and_firmware_arguments_and_says_what_is_sent() {
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
    for arg in ["vendor", "model", "firmware"] {
        assert_eq!(props[arg]["type"], "string", "{arg}: {props}");
    }
    assert!(props.get("device_name").is_none(), "the free name left: {props}");
    assert!(!tool.to_string().contains("device_name"), "{tool}");
    let what = format!("{} {}", props["share_fingerprint"]["description"], tool["description"]).to_lowercase();
    for word in ["firmware", "usb", "identity_reply", "ports", "serial number", "host name", "/v1/fingerprints"] {
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

// ---- the three things the sharing needs ----

#[test]
fn a_share_missing_the_maker_the_model_or_the_firmware_sends_nothing() {
    for (vendor, model, firmware) in [("", "MiniLab 3", "1.0"), ("Arturia", "", "1.0"), ("Arturia", "MiniLab 3", ""), ("  ", "MiniLab 3", "1.0"), ("Arturia", "MiniLab 3", "   ")] {
        let dir = super::common::temp("fp-missing");
        let fake = fake(200, "{}");
        let online = client(&fake, dir.path());
        let result = online.share_fingerprint(&settings_path(dir.path()), vendor, model, firmware, &learned());
        assert_ne!(result, Sharing::Sent, "{vendor:?} {model:?} {firmware:?}");
        assert!(fake.posts.lock().unwrap().is_empty(), "{vendor:?} {model:?} {firmware:?}");
    }
}

fn share_call(args: Value) -> Value {
    let dir = super::common::temp("fp-tool-refusal");
    let mut all = json!({ "learn": true, "share_fingerprint": "minilab" });
    all.as_object_mut().unwrap().extend(args.as_object().unwrap().clone());
    let replies = super::common::mcp_session(
        "http://127.0.0.1:1", dir.path(), &[super::common::tool_call(1, "detect_hardware", all)]);
    replies.into_iter().next().unwrap()
}

fn refusal_text(reply: &Value) -> String {
    let text = format!("{} {}", reply["error"]["message"], super::common::texts(reply).join(" "));
    assert!(reply.get("error").is_some() || reply["result"]["isError"] == true, "refused: {reply}");
    text
}

#[test]
fn the_tool_refuses_a_share_before_listening_and_names_every_missing_field_in_its_own_words() {
    let text = refusal_text(&share_call(json!({ "vendor": "Arturia" })));
    assert!(text.contains("model") && text.contains("firmware"), "{text}");
    assert!(!text.contains("vendor"), "only what is missing is named: {text}");
    assert!(!text.contains("--model") && !text.contains("device_name"), "{text}");
    let text = refusal_text(&share_call(json!({ "vendor": "Arturia", "model": "MiniLab 3", "firmware": "  " })));
    assert!(text.contains("firmware") && !text.contains("model"), "{text}");
    let text = refusal_text(&share_call(json!({})));
    assert!(["vendor", "model", "firmware"].iter().all(|w| text.contains(w)), "{text}");
}

fn detect_learn(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_osc-bridge")).args(["detect", "--learn"]).args(args).output().unwrap()
}

#[test]
fn the_command_refuses_a_share_before_listening_and_names_every_missing_field_with_its_flag() {
    let out = detect_learn(&["--share-fingerprint", "bcr", "--vendor", "Behringer"]);
    assert!(!out.status.success());
    let shown = String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(shown.contains("--model") && shown.contains("--firmware"), "{shown}");
    assert!(!shown.contains("--device-name"), "{shown}");
    let out = detect_learn(&["--share-fingerprint", "bcr", "--vendor", "Behringer", "--model", "BCR-32", "--firmware", ""]);
    assert!(!out.status.success());
    let shown = String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(shown.contains("--firmware"), "{shown}");
}

#[test]
fn the_command_help_names_the_maker_the_model_and_the_firmware_and_no_free_name() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_osc-bridge")).args(["detect", "--help"]).output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout).to_string();
    for flag in ["--vendor", "--model", "--firmware"] {
        assert!(help.contains(flag), "{flag} in {help}");
    }
    assert!(!help.contains("device-name") && !help.contains("device_name"), "{help}");
}

#[test]
fn a_refusal_of_the_service_naming_the_maker_and_model_is_told_with_the_flags_of_the_command() {
    let said = "HTTP 400: device: its descriptor (maker/file), or its vendor and model";
    let command = osc_bridge::mcp::refusal_in_words(said, true);
    assert!(command.contains("--vendor") && command.contains("--model"), "{command}");
    let tool = osc_bridge::mcp::refusal_in_words(said, false);
    assert!(tool.contains("vendor") && tool.contains("model") && !tool.contains("--vendor"), "{tool}");
}

#[test]
fn a_refusal_of_the_service_that_names_no_field_is_told_as_it_is() {
    let said = "HTTP 429: Daily quota reached";
    assert_eq!(osc_bridge::mcp::refusal_in_words(said, true), said);
    assert_eq!(osc_bridge::mcp::refusal_in_words(said, false), said);
}

#[test]
fn the_privacy_policy_says_the_maker_model_and_firmware_are_sent_and_no_free_name() {
    let doc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../PRIVACY.md")).unwrap().to_lowercase();
    assert!(!doc.contains("device_name") && !doc.contains("--device-name"), "the free name left the documents");
    let start = doc.find("## a copy installed on your machine").expect("section");
    let rest = &doc[start + 3..];
    let section = &rest[..rest.find("\n## ").unwrap_or(rest.len())];
    for word in ["vendor", "model", "firmware"] {
        assert!(section.contains(word), "missing `{word}` in the section");
    }
}

#[test]
fn the_package_interface_says_the_maker_model_and_firmware_and_no_free_name() {
    let doc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/INTERFACE.md")).unwrap();
    assert!(!doc.contains("device_name") && !doc.contains("--device-name"), "the free name left the documents");
    for flag in ["--vendor", "--model", "--firmware"] {
        assert!(doc.contains(flag), "{flag}");
    }
}
