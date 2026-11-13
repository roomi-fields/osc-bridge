use super::*;

/// An empty index — these handshake/dispatch tests don't touch devices.
fn empty_index() -> DeviceIndex {
    DeviceIndex { entries: Vec::new(), by_slug: HashMap::new() }
}

/// The drivers the tests read: a few of the catalogue's, and synthetic ones.
const FIXTURES: &str = "tests/fixtures/devices";

fn fixtures() -> DeviceIndex {
    DeviceIndex::build(Path::new(FIXTURES))
}

fn test_opts() -> McpOptions {
    McpOptions {
        devices_dir: Some(PathBuf::from(FIXTURES)),
        default_target: "127.0.0.1:7777".parse().unwrap(),
        online: false,
        private: Vec::new(),
    }
}

#[test]
fn list_devices_filters_by_words_vendor_kind_and_limit() {
    let idx = fixtures();
    let run = |args: Value| list_devices(&idx, &DeviceFilter::from_args(&args), &|_| None).unwrap()["structuredContent"].clone();
    let all = run(json!({}));
    assert_eq!(all["count"].as_u64().unwrap() as usize, idx.entries.len());
    let mini = run(json!({"query": "KORG minilogue"}));
    let names: Vec<&str> = mini["devices"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("minilogue")), "{names:?}");
    assert!(names.iter().all(|n| n.to_lowercase().contains("minilogue")), "{names:?}");
    let novation = run(json!({"vendor": "novation", "limit": 1}));
    assert!(novation["count"].as_u64().unwrap() > 1);
    assert_eq!(novation["devices"].as_array().unwrap().len(), 1);
    let software = run(json!({"kind": "software"}));
    assert!(software["devices"].as_array().unwrap().iter().all(|d| d["kind"] == "software"));
}

#[test]
fn a_private_device_keeps_its_reports_on_the_machine() {
    let index = fixtures();
    let opts = McpOptions { online: true, private: vec!["korg/minilogue".into()], ..test_opts() };
    // Never reached: the refusal comes first.
    let link = Link::new("http://127.0.0.1:9");
    let caller = Some(&link);
    for (tool, args) in [("report_result", json!({"device": "korg/minilogue", "outcome": "works"})),
                         ("report_mapping", json!({"device": "minilogue", "address": "/filter/cutoff", "cc": 43})),
                         ("submit_fix", json!({"device": "korg/minilogue", "content": "{}"})),
                         ("share_preset", json!({"device": "korg/minilogue", "name": "Warm", "own_sound": true, "hex": "F0 42 F7"}))] {
        let err = call_tool(tool, &args, &opts, &index, caller).unwrap_err();
        assert!(err.contains("private on this machine") && err.contains("private list"), "{tool}: {err}");
    }
    let driver = json!({"driver": {"device": {"name": "minilogue", "vendor": "KORG", "osc_prefix": "/minilogue"}}});
    assert!(call_tool("submit_driver", &driver, &opts, &index, caller).unwrap_err().contains("private"));
    let trust = call_tool("get_trust", &json!({"device": "korg/minilogue"}), &opts, &index, caller).unwrap();
    assert_eq!(trust["structuredContent"]["private"], json!(true), "answered on the machine, the server is never asked");
    let vendor = McpOptions { private: vec!["korg/*".into()], ..opts };
    assert!(call_tool("report_result", &json!({"device": "korg/ms2000", "outcome": "works"}), &vendor, &index, caller)
        .unwrap_err().contains("private"));
    let other = call_tool("report_result", &json!({"device": "arturia/minilab3", "outcome": "works"}), &vendor, &index, caller).unwrap_err();
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
    let opts = McpOptions { online: true, ..test_opts() };
    assert!(tool_names(&opts).contains(&"load_preset") && !tool_names(&test_opts()).contains(&"load_preset"),
        "loading a shared preset is offered only when linked to the server");
}

#[test]
fn the_privacy_command_records_choices_by_catalogue_id() {
    let dir = tempfile::Builder::new().prefix("osc-bridge-privacy-").tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let devices = fixtures();
    let shown = privacy(&path, &devices, None, None, &[], &[]).unwrap();
    assert!(shown.contains("Linked to the SynthHub server") && shown.contains("Private devices: none"), "{shown}");
    assert!(!path.exists(), "showing writes nothing");
    let shown = privacy(&path, &devices, None, Some(true), &["minilogue".into()], &[]).unwrap();
    assert!(shown.contains("Offline") && shown.contains("korg/minilogue"), "{shown}");
    assert_eq!(crate::online::Settings::load(Some(&path)).unwrap(),
        crate::online::Settings { offline: true, private: vec!["korg/minilogue".into()] });
    assert!(privacy(&path, &devices, None, None, &["no-such-synth".into()], &[]).is_err());
    privacy(&path, &devices, None, Some(false), &[], &["korg/minilogue".into()]).unwrap();
    assert_eq!(crate::online::Settings::load(Some(&path)).unwrap(), crate::online::Settings::default());
}

#[test]
fn an_offline_copy_does_not_offer_the_server_tools() {
    let idx = fixtures();
    let resp = handle_request(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "report_result", "arguments": {"device": "korg/minilogue", "outcome": "works"}}}),
        &test_opts(), &idx, None).unwrap();
    assert_eq!(resp["error"]["code"], json!(-32602));
}

#[test]
fn the_catalogue_tools_are_shared_with_the_hosted_server() {
    let idx = fixtures();
    let routes = catalogue_tool("list_routes", &json!({"slug": "minilogue"}), &idx, &|_| None).unwrap().unwrap();
    assert!(routes["structuredContent"].is_object());
    let listed = catalogue_tool("list_devices", &json!({"query": "minilogue", "limit": 1}), &idx, &|_| Some("unconfirmed".into()))
        .unwrap().unwrap();
    assert_eq!(listed["structuredContent"]["devices"][0]["usage_status"], "unconfirmed");
    assert!(catalogue_tool("send", &json!({}), &idx, &|_| None).is_none());
    let midnam = catalogue_tool("export_device", &json!({"device": "korg/minilogue", "format": "midnam"}), &idx, &|_| None)
        .unwrap().unwrap();
    assert_eq!(midnam["structuredContent"]["media_type"], "application/xml");
    assert!(midnam["content"][0]["text"].as_str().unwrap().contains("<MIDINameDocument>"));
    let oscq = catalogue_tool("export_device", &json!({"device": "minilogue", "format": "oscquery"}), &idx, &|_| None)
        .unwrap().unwrap();
    let ns: Value = serde_json::from_str(oscq["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(ns["FULL_PATH"], "/");
    assert!(catalogue_tool("export_device", &json!({"device": "minilogue", "format": "ctrlr"}), &idx, &|_| None)
        .unwrap().is_err());
    let names: Vec<String> = tool_schemas()["tools"].as_array().unwrap().iter()
        .map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert!(crate::online::FORWARDED_TOOLS.iter().all(|f| names.iter().any(|n| n == f)), "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with("relay_")), "relay tools belong to the hosted server");
}

#[test]
fn device_index_builds_from_a_folder_of_drivers() {
    let idx = fixtures();
    assert!(idx.entries.len() > 10, "expected every test driver, got {}", idx.entries.len());
    let p = idx.find_path("minilogue").expect("/minilogue slug should resolve");
    assert!(p.to_string_lossy().contains("korg"));
    assert_eq!(idx.find("korg/minilogue").map(|e| e.slug.as_str()), Some("minilogue"));
    // Unknown slug resolves to nothing rather than walking the tree.
    assert!(idx.find_path("does-not-exist").is_none());
}

#[test]
fn tools_list_has_nine_tools() {
    let t = tools_list(&test_opts());
    let arr = t.get("tools").unwrap().as_array().unwrap();
    assert_eq!(arr.len(), 9);
    let names: Vec<&str> = arr.iter().map(|t| t.get("name").unwrap().as_str().unwrap()).collect();
    for expected in ["list_devices", "get_device_docs", "list_routes", "export_device", "send", "get_status",
                     "detect_hardware", "backup_presets", "restore_presets"] {
        assert!(names.contains(&expected), "tool {expected} missing");
    }
}

#[test]
fn every_tool_declares_schema_and_annotations() {
    let t = tools_list(&test_opts());
    for tool in t.get("tools").unwrap().as_array().unwrap() {
        let name = tool.get("name").and_then(Value::as_str).unwrap();
        assert!(tool.get("inputSchema").map(Value::is_object).unwrap_or(false),
            "tool {name} missing inputSchema");
        assert!(tool.get("outputSchema").map(Value::is_object).unwrap_or(false),
            "tool {name} missing outputSchema");
        let ann = tool.get("annotations").and_then(Value::as_object);
        assert!(ann.is_some(), "tool {name} missing annotations");
        assert!(ann.unwrap().contains_key("title"), "tool {name} annotations missing title");
    }
}

#[test]
fn list_devices_result_carries_structured_content() {
    let idx = fixtures();
    let res = list_devices(&idx, &DeviceFilter::default(), &|_| None).unwrap();
    let sc = res.get("structuredContent").expect("structuredContent present");
    assert_eq!(sc.get("count").and_then(Value::as_u64).unwrap() as usize, idx.entries.len());
    assert_eq!(sc.get("devices").and_then(Value::as_array).unwrap().len(), idx.entries.len());
    // The text fallback block is still there for non-structured clients.
    assert!(res.get("content").and_then(Value::as_array).is_some());
}

#[test]
fn list_routes_exposes_expects_on_surface_entries() {
    let idx = fixtures();
    // Find any device with a non-empty cc_params surface and check that
    // every entry carries an `expects` string (defaulted to "continuous").
    let mut checked = false;
    for e in &idx.entries {
        let res = match list_routes(&idx, &e.slug) { Ok(r) => r, Err(_) => continue };
        let sc = res.get("structuredContent").unwrap();
        let cc = sc.get("cc_params").and_then(Value::as_array).unwrap();
        if cc.is_empty() { continue; }
        for entry in cc {
            let nature = entry.get("expects").and_then(Value::as_str);
            assert!(nature.is_some(), "cc_param on {} missing expects", e.slug);
            assert!(!nature.unwrap().is_empty());
        }
        checked = true;
        break;
    }
    assert!(checked, "expected at least one device with cc_params");
}

#[test]
fn shared_slug_resolves_to_the_most_trustworthy_variant() {
    let idx = fixtures();
    let mut by_slug: HashMap<&str, Vec<&DeviceIndexEntry>> = HashMap::new();
    for e in &idx.entries { by_slug.entry(e.slug.as_str()).or_default().push(e); }
    let mut checked = 0;
    for (slug, variants) in by_slug.iter().filter(|(s, v)| !s.is_empty() && v.len() > 1) {
        let best = variants.iter().map(|e| tier_rank(&e.tier)).min().unwrap();
        let chosen = idx.find_path(slug).unwrap();
        let chosen_tier = &variants.iter().find(|e| e.path == chosen).unwrap().tier;
        assert_eq!(tier_rank(chosen_tier), best, "{slug} resolved to a weaker variant");
        checked += 1;
    }
    assert!(checked > 0, "the test drivers hold variants of one device");
    assert_eq!(idx.find_path("box"), Some(Path::new(FIXTURES).join("acme/box.vendor.fw-2.json").as_path()),
        "the vendor's documentation wins over an unreviewed contribution");
}

#[test]
fn tool_failure_is_a_result_and_unknown_tool_a_protocol_error() {
    let idx = fixtures();
    let opts = test_opts();
    let call = |name: &str| handle_request(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": name, "arguments": {"slug": "no-such-device"}}}), &opts, &idx, None).unwrap();
    let failed = call("list_routes");
    assert_eq!(failed["result"]["isError"], json!(true));
    assert!(failed["result"]["content"][0]["text"].as_str().unwrap().contains("no-such-device"));
    let unknown = call("no_such_tool");
    assert_eq!(unknown["error"]["code"], json!(-32602));
    let listed: Vec<String> = tools_list(&test_opts())["tools"].as_array().unwrap().iter()
        .map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(listed.len(), tool_names(&test_opts()).len());
    assert!(tool_names(&test_opts()).iter().all(|n| listed.iter().any(|l| l == n)));
}

#[test]
fn restore_refuses_without_explicit_confirmation() {
    let idx = fixtures();
    let opts = test_opts();
    let err = call_tool("restore_presets", &json!({"out_port": "0", "file": "x.syx"}), &opts, &idx, None).unwrap_err();
    assert!(err.contains("confirm_overwrite"), "{err}");
}

#[test]
fn port_names_and_identities_resolve_to_their_driver() {
    let cat = hardware_candidates(&fixtures());
    for (port, want) in [("minilogue MIDI 1", "korg/minilogue"), ("Bass Station II", "novation/bass_station_ii"),
                         ("Electra Controller Electra Port 1", "electra-one/electra-one")] {
        let best = match_port(&cat, port, &[]);
        assert_eq!(best.first().map(|c| c["id"].clone()), Some(json!(want)), "{port}: {best:?}");
    }
    // An Identity Reply from the maker puts its driver first.
    let acme = crate::detect::Identity { manufacturer: vec![0, 33, 9], family: 1, model: 1, version: [2, 0, 0, 0] };
    let best = match_port(&cat, "Box", &[acme.reply()]);
    assert_eq!(best[0]["slug"], "box", "{best:?}");
    // Its identity block names the firmware, which picks the variant for it.
    assert_eq!(best[0]["id"], "acme/box.vendor.fw-2", "{best:?}");
    assert_eq!((best[0]["firmware"].as_str(), &best[0]["recognised_by"]), (Some("2.0.0.0"), &json!(["identity reply"])));
    let nameless = match_port(&cat, "USB MIDI Interface", &[acme.reply()]);
    assert_eq!(nameless[0]["id"], "acme/box.vendor.fw-2", "behind a port named after nothing: {nameless:?}");
    let other_fw = crate::detect::Identity { version: [3, 0, 0, 0], ..acme.clone() };
    assert!(match_port(&cat, "Box", &[other_fw.reply()])[0]["score"].as_u64() < best[0]["score"].as_u64());
    assert!(best.iter().all(|c| c.get("file").is_none()), "no path of the machine that matched them");
}

#[test]
fn init_advertises_tools_capability() {
    let r = init_result();
    assert!(r.get("capabilities").and_then(|c| c.get("tools")).is_some());
    assert_eq!(r.get("protocolVersion").and_then(Value::as_str), Some(PROTOCOL_VERSION));
}

#[test]
fn handle_initialize() {
    let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
    let opts = test_opts();
    let resp = handle_request(&req, &opts, &empty_index(), None).unwrap();
    assert_eq!(resp.get("id"), Some(&json!(1)));
    assert!(resp.get("result").is_some());
}

#[test]
fn handle_initialized_notification_returns_nothing() {
    let req = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let opts = test_opts();
    assert!(handle_request(&req, &opts, &empty_index(), None).is_none());
}

#[test]
fn handle_unknown_method_returns_error() {
    let req = json!({"jsonrpc":"2.0","id":99,"method":"foo/bar","params":{}});
    let opts = test_opts();
    let resp = handle_request(&req, &opts, &empty_index(), None).unwrap();
    assert!(resp.get("error").is_some());
}

/// A stand-in for the hosted server, on a free local port: it delivers
/// accounts by its challenge (answering 401 to an id it did not issue, like
/// the real one), hands out the drivers under `root` one at a time (a file and
/// its notes, a device's variants, the drivers matching MIDI ports), answers
/// the catalogue tools, gives a fixed trust list, and answers every forwarded
/// tool call with the tool's name. The forwarded calls it received are in the
/// returned list.
fn fake_server(root: &Path) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (root, seen) = (root.to_path_buf(), calls.clone());
    std::thread::spawn(move || {
        let index = DeviceIndex::build(&root);
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
                "/v1/detect" if req.method() == &tiny_http::Method::Get => (200, probes_json(&detect_probes(&index)).to_string()),
                "/v1/detect" => {
                    let asked: Value = serde_json::from_str(&body).unwrap();
                    let cat = hardware_candidates(&index);
                    let ports: Vec<Value> = asked["ports"].as_array().unwrap().iter().map(|p| {
                        let replies: Vec<Vec<u8>> = p["replies_hex"].as_array().into_iter().flatten()
                            .map(|h| crate::librarian::parse_hex(h.as_str().unwrap()).unwrap()).collect();
                        json!({ "port": p["port"], "candidates": match_port(&cat, p["port"].as_str().unwrap(), &replies) })
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
    (base, calls)
}

#[test]
fn a_call_names_the_driver_it_needs() {
    let call = |name: &str, args: Value| json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": name, "arguments": args}});
    assert_eq!(driver_wanted(&call("list_routes", json!({"slug": "minilogue"}))), Some("minilogue"));
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
    let (base, calls) = fake_server(Path::new(FIXTURES));
    let link = Link { client: crate::online::Online::keeping_account_in(&base, Some(tmp.join("user"))), ..Link::new("unused") };

    // By slug, its best variant; with its notes and the driver it takes its MIDI from.
    let fetched = crate::catalogue::fetch(&link.client, "microfreak_tools", &cache).unwrap();
    assert_eq!(fetched, cache.join("arturia/microfreak.vendor.fw-microfreak-tools-0.1.0.json"));
    assert!(fetched.with_extension("md").is_file() && cache.join("arturia/microfreak.json").is_file());
    assert_eq!(Device::load(&fetched).unwrap().device.osc_prefix, "/microfreak_tools");
    assert_eq!(crate::catalogue::fetch(&link.client, "box", &cache).unwrap(), cache.join("acme/box.vendor.fw-2.json"));
    assert!(crate::catalogue::fetch(&link.client, "acme/none", &cache).unwrap_err().to_string().contains("no driver acme/none"));
    assert!(link.client.fetch_driver("../secret", &cache).is_err());
    assert_eq!(DeviceIndex::build(&cache).entries.len(), 3, "only what was asked for is on this machine");

    // The server lists and matches the drivers; what is cached here is not the catalogue.
    let opts = McpOptions { online: true, devices_dir: None, ..test_opts() };
    let index = DeviceIndex::build(&cache);
    let listed = call_tool("list_devices", &json!({"query": "minilogue"}), &opts, &index, Some(&link)).unwrap();
    assert_eq!(listed["structuredContent"]["devices"][0]["id"], "korg/minilogue", "{listed}");
    let matched = link.client.detect(&[json!({"port": "minilogue MIDI 1"})]).unwrap();
    assert_eq!(matched[0]["candidates"][0]["id"], "korg/minilogue", "{matched:?}");
    let acme = crate::detect::Identity { manufacturer: vec![0, 33, 9], family: 1, model: 1, version: [2, 0, 0, 0] };
    let matched = link.client.detect(&[json!({"port": "USB MIDI", "replies_hex": [hex_bytes(&acme.reply())]})]).unwrap();
    assert_eq!(matched[0]["candidates"][0]["firmware"], "2.0.0.0", "{matched:?}");
    assert_eq!(link.client.probes().unwrap(), detect_probes(&DeviceIndex::build(Path::new(FIXTURES))));

    // Reports go out under the account kept on this machine.
    let user = link.user(false).unwrap();
    assert_eq!(std::fs::read_to_string(tmp.join("user")).unwrap(), user, "the account is kept");
    let reported = call_tool("report_result", &json!({"device": "korg/minilogue", "outcome": "works", "firmware": "2.10"}),
        &opts, &index, Some(&link)).unwrap();
    assert_eq!(reported["content"][0]["text"], "report_result", "{reported}");
    assert!(tool_names(&opts).contains(&"submit_fix") && !tool_names(&test_opts()).contains(&"submit_fix"));

    std::fs::write(tmp.join("user"), "0".repeat(32)).unwrap();
    let relinked = Link { client: crate::online::Online::keeping_account_in(&base, Some(tmp.join("user"))), ..Link::new("unused") };
    let trust = call_tool("get_trust", &json!({"device": "korg/minilogue"}), &opts, &index, Some(&relinked)).unwrap();
    assert_eq!(trust["content"][0]["text"], "get_trust", "an account the server forgot is replaced: {trust}");
    assert_ne!(std::fs::read_to_string(tmp.join("user")).unwrap(), "0".repeat(32));
    assert_eq!(*calls.lock().unwrap(), ["report_result", "get_trust"]);
}
