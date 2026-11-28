use super::*;
use crate::mcp::{catalogue_tool, hardware_candidates, hex_bytes, match_port, probes_json, DeviceIndex, McpOptions};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The drivers the tests read: a few of the catalogue's, and synthetic ones.
const FIXTURES: &str = "tests/fixtures/devices";

fn neutral(_: &str) -> u8 { 0 }

/// An order that puts the tier whose name starts with the later letter first: the fixture
/// folder's two cards of `acme/box` (`contributed`, `vendor-doc`) are then told apart without
/// the test knowing the catalogue's tiers.
fn later_letter_first(tier: &str) -> u8 { 255 - tier.bytes().next().unwrap_or(0) }

fn fixtures() -> DeviceIndex {
    DeviceIndex::build(Path::new(FIXTURES), neutral)
}

fn test_opts() -> McpOptions {
    McpOptions {
        devices_dir: Some(PathBuf::from(FIXTURES)),
        default_target: "127.0.0.1:7777".parse().unwrap(),
        tier_order: neutral,
        detect_mode: None,
    }
}

#[test]
fn the_privacy_command_records_choices_by_catalogue_id() {
    let dir = tempfile::Builder::new().prefix("osc-bridge-privacy-").tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let devices = fixtures();
    let shown = privacy(&path, &devices, None, None, &[], &[]).unwrap();
    assert!(shown.contains("Linked to the SyntHub server") && shown.contains("Private devices: none"), "{shown}");
    assert!(!path.exists(), "showing writes nothing");
    let shown = privacy(&path, &devices, None, Some(true), &["minilogue".into()], &[]).unwrap();
    assert!(shown.contains("Offline") && shown.contains("korg/minilogue"), "{shown}");
    assert_eq!(crate::synthub::online::Settings::load(Some(&path)).unwrap(),
        crate::synthub::online::Settings { offline: true, private: vec!["korg/minilogue".into()] });
    assert!(privacy(&path, &devices, None, None, &["no-such-synth".into()], &[]).is_err());
    privacy(&path, &devices, None, Some(false), &[], &["korg/minilogue".into()]).unwrap();
    assert_eq!(crate::synthub::online::Settings::load(Some(&path)).unwrap(), crate::synthub::online::Settings::default());
}

#[test]
fn a_private_device_keeps_its_reports_on_the_machine() {
    let index = fixtures();
    let opts = test_opts();
    // Never reached: the refusal comes first.
    let link = Link { private: vec!["korg/minilogue".into()], ..Link::new("http://127.0.0.1:9") };
    for (tool, args) in [("report_result", json!({"device": "korg/minilogue", "outcome": "works"})),
                         ("report_mapping", json!({"device": "minilogue", "address": "/filter/cutoff", "cc": 43})),
                         ("submit_fix", json!({"device": "korg/minilogue", "content": "{}"})),
                         ("share_preset", json!({"device": "korg/minilogue", "name": "Warm", "own_sound": true, "hex": "F0 42 F7"}))] {
        let err = call_tool(tool, &args, &opts, &index, &link).unwrap_err();
        assert!(err.contains("private on this machine") && err.contains("private list"), "{tool}: {err}");
    }
    let driver = json!({"driver": {"device": {"name": "minilogue", "vendor": "KORG", "osc_prefix": "/minilogue"}}});
    assert!(call_tool("submit_driver", &driver, &opts, &index, &link).unwrap_err().contains("private"));
    let trust = call_tool("get_trust", &json!({"device": "korg/minilogue"}), &opts, &index, &link).unwrap();
    assert_eq!(trust["structuredContent"]["private"], json!(true), "answered on the machine, the server is never asked");
    let vendor = Link { private: vec!["korg/*".into()], ..Link::new("http://127.0.0.1:9") };
    assert!(call_tool("report_result", &json!({"device": "korg/ms2000", "outcome": "works"}), &opts, &index, &vendor)
        .unwrap_err().contains("private"));
    let other = call_tool("report_result", &json!({"device": "arturia/minilab3", "outcome": "works"}), &opts, &index, &vendor).unwrap_err();
    assert!(other.contains("could not be reached"), "another device is shared: {other}");
}

#[test]
fn a_presets_files_on_this_machine_are_read_before_sharing() {
    let dir = tempfile::Builder::new().prefix("osc-bridge-share-").tempdir().unwrap();
    let (syx, mid, ogg) = (dir.path().join("p.syx"), dir.path().join("p.mid"), dir.path().join("p.ogg"));
    std::fs::write(&syx, [0xF0, 0x42, 0x30, 0x01, 0xF7, 0xF0, 0x42, 0x30, 0x02, 0xF7]).unwrap();
    std::fs::write(&mid, b"MThd").unwrap();
    std::fs::write(&ogg, b"OggS").unwrap();
    let p = |f: &PathBuf| f.display().to_string();
    let (sent, audio) = read_local_files(&json!({"device": "korg/minilogue", "file": p(&syx), "midi_file": p(&mid),
        "audio_file": p(&ogg)})).unwrap();
    assert_eq!(sent, json!({"device": "korg/minilogue", "hex": "F0 42 30 01 F7 F0 42 30 02 F7", "midi_base64": "TVRoZA=="}));
    assert_eq!(audio.as_deref(), Some(&b"OggS"[..]));
    assert!(read_local_files(&json!({"file": p(&syx), "hex": "F0 F7"})).unwrap_err().contains("not both"));
}

type Shared<T> = std::sync::Arc<std::sync::Mutex<Vec<T>>>;

/// A stand-in for the hosted server, on a free local port: it delivers
/// accounts by its challenge (answering 401 to an id it did not issue, like
/// the real one), hands out the drivers under `root` one at a time (a file and
/// its notes, a device's variants, the drivers matching MIDI ports), answers
/// the catalogue tools, gives a fixed trust list, and answers every forwarded
/// tool call with the tool's name. The forwarded calls it received are in the
/// returned list, and the bodies of the detect requests in the second.
fn fake_server(root: &Path) -> (String, Shared<String>, Shared<Value>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let detects = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (root, seen, asked_detect) = (root.to_path_buf(), calls.clone(), detects.clone());
    std::thread::spawn(move || {
        let index = DeviceIndex::build(&root, later_letter_first);
        let mut issued: Vec<String> = Vec::new();
        let challenge = json!({ "challenge": "fake", "algorithm": "sha256", "bits": 4, "deliver": "/v1/access/deliver" }).to_string();
        for mut req in server.incoming_requests() {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
            let url = req.url().to_string();
            let file = |rel: &str| std::fs::read_to_string(root.join(rel)).ok();
            let (status, reply) = match url.as_str() {
                "/v1/access/challenge" => (200, challenge.clone()),
                "/v1/access/deliver" => {
                    let id = format!("{:032x}", issued.len() + 1);
                    issued.push(id.clone());
                    (200, json!({ "id": id, "user_id": id, "mcp_path": format!("/u/{id}/mcp"), "tier": "free" }).to_string())
                }
                "/v1/trust" => (200, json!({ "devices": { "korg/minilogue": { "status": "unconfirmed" } } }).to_string()),
                u if u.starts_with("/api/v1/devices/") => match file(&u["/api/v1/devices/".len()..]) {
                    Some(text) => (200, text),
                    None => (404, "not found\n".into()),
                },
                u if u.starts_with("/v1/devices/") => match index.find(&u["/v1/devices/".len()..]) {
                    Some(best) => {
                        let variants: Vec<Value> = index.entries.iter().filter(|e| e.slug == best.slug)
                            .map(|e| json!({ "id": e.id, "slug": e.slug })).collect();
                        (200, json!({ "best": best.id, "variants": variants }).to_string())
                    }
                    None => (404, json!({ "error": "no such device" }).to_string()),
                },
                "/v1/detect" if req.method() == &tiny_http::Method::Get => (200, probes_json(&crate::mcp::detect_probes(&index)).to_string()),
                "/v1/detect" => {
                    let asked: Value = serde_json::from_str(&body).unwrap();
                    asked_detect.lock().unwrap().push(asked.clone());
                    let cat = hardware_candidates(&index);
                    let ports: Vec<Value> = asked["ports"].as_array().unwrap().iter().map(|p| {
                        let replies: Vec<Vec<u8>> = p["replies_hex"].as_array().into_iter().flatten()
                            .map(|h| crate::librarian::parse_hex(h.as_str().unwrap()).unwrap()).collect();
                        json!({ "port": p["port"], "devices": match_port(&cat, p["port"].as_str().unwrap(), None, &replies, neutral) })
                    }).collect();
                    (200, json!({ "ports": ports }).to_string())
                }
                "/mcp" => {
                    let rpc: Value = serde_json::from_str(&body).unwrap();
                    let result = catalogue_tool(rpc["params"]["name"].as_str().unwrap(), &rpc["params"]["arguments"], &index, &|_| None)
                        .unwrap().unwrap();
                    (200, json!({ "jsonrpc": "2.0", "id": 1, "result": result }).to_string())
                }
                u if u.starts_with("/u/") && u.ends_with("/mcp") => {
                    let id = &u[3..u.len() - 4];
                    if issued.iter().any(|i| i == id) {
                        let rpc: Value = serde_json::from_str(&body).unwrap();
                        let name = rpc["params"]["name"].as_str().unwrap().to_string();
                        seen.lock().unwrap().push(name.clone());
                        (200, json!({ "jsonrpc": "2.0", "id": 1, "result": { "content": [{ "type": "text", "text": name }] } }).to_string())
                    } else {
                        // Unknown account: the service answers 401 with the challenge itself.
                        (401, challenge.clone())
                    }
                }
                _ => (404, "{}".into()),
            };
            let _ = req.respond(tiny_http::Response::from_string(reply).with_status_code(status));
        }
    });
    (base, calls, detects)
}

#[test]
fn a_call_names_the_driver_it_needs() {
    let call = |name: &str, args: Value| json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": name, "arguments": args}});
    assert_eq!(driver_wanted(&call("list_routes", json!({"device": "minilogue"}))), Some("minilogue"));
    assert_eq!(driver_wanted(&call("get_device_docs", json!({"device": "korg/minilogue"}))), Some("korg/minilogue"));
    assert_eq!(driver_wanted(&call("list_routes", json!({"slug": "minilogue"}))), None, "slug no longer names a device");
    assert_eq!(driver_wanted(&call("export_device", json!({"device": "korg/minilogue", "format": "midnam"}))), Some("korg/minilogue"));
    assert_eq!(driver_wanted(&call("report_result", json!({"device": "minilogue", "outcome": "works"}))), Some("minilogue"));
    assert_eq!(driver_wanted(&call("list_devices", json!({"query": "korg"}))), None, "the server lists them");
    assert_eq!(driver_wanted(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})), None);
}

#[test]
fn a_local_copy_fetches_the_drivers_it_needs_and_forwards_reports() {
    let dir = tempfile::Builder::new().prefix("osc-bridge-link-").tempdir().unwrap();
    let tmp = dir.path();
    let cache = tmp.join("cache");
    let (base, calls, _) = fake_server(Path::new(FIXTURES));
    let link = Link { client: crate::synthub::online::Online::keeping_account_in(&base, Some(tmp.join("user"))), ..Link::new("unused") };

    // By slug, its best variant; with its notes and the driver it takes its MIDI from.
    let fetched = crate::synthub::catalogue::fetch(&link.client, "microfreak_tools", &cache).unwrap();
    assert_eq!(fetched, cache.join("arturia/microfreak.vendor.fw-microfreak-tools-0.1.0.json"));
    assert!(fetched.with_extension("md").is_file() && cache.join("arturia/microfreak.json").is_file());
    assert_eq!(crate::device::load(&fetched).unwrap().device.osc_prefix, "/microfreak_tools");
    assert_eq!(crate::synthub::catalogue::fetch(&link.client, "box", &cache).unwrap(), cache.join("acme/box.vendor.fw-2.json"));
    assert!(crate::synthub::catalogue::fetch(&link.client, "acme/none", &cache).unwrap_err().to_string().contains("no driver acme/none"));
    assert!(link.client.fetch_driver("../secret", &cache).is_err());
    assert_eq!(DeviceIndex::build(&cache, neutral).entries.len(), 3, "only what was asked for is on this machine");

    // The server lists and matches the drivers; what is cached here is not the catalogue.
    let opts = McpOptions { devices_dir: None, ..test_opts() };
    let index = DeviceIndex::build(&cache, neutral);
    let listed = call_tool("list_devices", &json!({"query": "minilogue"}), &opts, &index, &link).unwrap();
    assert_eq!(listed["structuredContent"]["devices"][0]["device"], "korg/minilogue", "{listed}");
    let matched = link.client.detect(&[json!({"port": "minilogue MIDI 1"})]).unwrap();
    assert_eq!(matched[0]["devices"][0]["id"], "korg/minilogue", "{matched:?}");
    let acme = crate::recognition::Identity { manufacturer: vec![0, 33, 9], family: 1, model: 1, version: [2, 0, 0, 0] };
    let matched = link.client.detect(&[json!({"port": "USB MIDI", "replies_hex": [hex_bytes(&acme.reply())]})]).unwrap();
    assert_eq!(matched[0]["devices"][0]["firmware"], "2.0.0.0", "{matched:?}");
    assert_eq!(link.client.probes().unwrap(), crate::mcp::detect_probes(&DeviceIndex::build(Path::new(FIXTURES), neutral)));

    // Reports go out under the account kept on this machine.
    let user = link.user(false).unwrap();
    assert_eq!(std::fs::read_to_string(tmp.join("user")).unwrap(), user, "the account is kept");
    let reported = call_tool("report_result", &json!({"device": "korg/minilogue", "outcome": "works", "firmware": "2.10"}),
        &opts, &index, &link).unwrap();
    assert_eq!(reported["content"][0]["text"], "report_result", "{reported}");

    std::fs::write(tmp.join("user"), "0".repeat(32)).unwrap();
    let relinked = Link { client: crate::synthub::online::Online::keeping_account_in(&base, Some(tmp.join("user"))), ..Link::new("unused") };
    let trust = call_tool("get_trust", &json!({"device": "korg/minilogue"}), &opts, &index, &relinked).unwrap();
    assert_eq!(trust["content"][0]["text"], "get_trust", "an account the server forgot is replaced: {trust}");
    assert_ne!(std::fs::read_to_string(tmp.join("user")).unwrap(), "0".repeat(32));
    assert_eq!(*calls.lock().unwrap(), ["report_result", "get_trust"]);
}

#[test]
fn the_link_answers_a_local_tool_with_the_text_the_local_answer_gives() {
    let dir = tempfile::Builder::new().prefix("osc-bridge-link-").tempdir().unwrap();
    let (base, calls, _) = fake_server(Path::new(FIXTURES));
    let link = Link { client: crate::synthub::online::Online::keeping_account_in(&base, Some(dir.path().join("user"))), ..Link::new("unused") };
    let index = fixtures();
    let opts = test_opts();
    let unreachable = json!({ "target": "127.0.0.1:9" });
    for (tool, args) in [("list_links", unreachable.clone()), ("add_link", json!({ "to": "/b" })),
                         ("get_device_docs", json!({ "device": "korg/minilogue" })),
                         ("list_routes", json!({ "slug": "minilogue" })),
                         ("restore_presets", json!({ "device": "acme/box", "out_port": "0", "file": "x.syx" }))] {
        let through = call_tool(tool, &args, &opts, &index, &link);
        let local = crate::mcp::call_local(tool, &args, &test_opts(), &index, &crate::midi_port::open_hardware,
            &crate::mcp::Known { trust: Value::Null, candidates: Vec::new(), status: &|_| None, private: &|_| false });
        assert_eq!(through, local, "{tool}");
    }
    assert!(calls.lock().unwrap().is_empty(), "a local tool never goes to the service");
}

#[test]
fn the_link_sends_the_usb_ids_of_a_port_that_has_them_as_numbers_and_none_otherwise() {
    let dir = tempfile::Builder::new().prefix("osc-bridge-link-").tempdir().unwrap();
    let (base, _, detects) = fake_server(Path::new(FIXTURES));
    let client = crate::synthub::online::Online::keeping_account_in(&base, Some(dir.path().join("user")));
    let found = vec![
        crate::detect::Probed { out_index: Some(0), out_name: Some("BCR32".into()), in_index: Some(0), in_name: Some("BCR32".into()),
            identity: None, usb: Some((0x1397, 0x12cf)), replies: Vec::new(), client_kind: crate::detect::ClientKind::Unknown },
        crate::detect::Probed { out_index: Some(1), out_name: Some("Virtual".into()), in_index: Some(1), in_name: Some("Virtual".into()),
            identity: None, usb: None, replies: Vec::new(), client_kind: crate::detect::ClientKind::Unknown },
    ];
    let answers = detect_answers(&client, &found).unwrap();
    assert_eq!(answers.len(), 2, "one answer of the service per port");
    let sent = detects.lock().unwrap().clone();
    let ports = sent[0]["ports"].as_array().unwrap();
    assert_eq!(ports[0]["usb"], json!({ "vendor_id": 5015, "product_id": 4815 }), "{sent:?}");
    assert!(ports[1].get("usb").is_none(), "{sent:?}");
    assert_eq!(ports[0]["replies_hex"], json!([]));
}

#[test]
fn the_driver_submission_tool_names_the_source_the_format_calls_contributed() {
    let text = linked_tool_schemas().to_string();
    assert!(!text.contains("agent-contributed"), "the format does not name agent-contributed");
    assert!(text.contains("source 'contributed'"), "the tool must say the file carries source 'contributed'");
}
