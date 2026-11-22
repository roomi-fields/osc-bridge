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
fn a_lock_file_beside_a_kept_driver_is_never_taken_for_a_driver() {
    let dir = tempfile::Builder::new().prefix("osc-bridge-walk-").tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("acme")).unwrap();
    for name in ["box.json", "box.json.etag", "box.json.lock", "box.md", "box.md.lock"] {
        std::fs::write(dir.path().join("acme").join(name), "x").unwrap();
    }
    let mut seen = Vec::new();
    walk_json(dir.path(), &mut |p| seen.push(p.file_name().unwrap().to_string_lossy().into_owned())).unwrap();
    assert_eq!(seen, ["box.json"]);
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
    let routes = catalogue_tool("list_routes", &json!({"device": "minilogue"}), &idx, &|_| None).unwrap().unwrap();
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
fn tools_list_has_the_local_tools_including_the_three_link_tools() {
    let t = tools_list(&test_opts());
    let arr = t.get("tools").unwrap().as_array().unwrap();
    assert_eq!(arr.len(), 16);
    let names: Vec<&str> = arr.iter().map(|t| t.get("name").unwrap().as_str().unwrap()).collect();
    for expected in ["list_devices", "get_device_docs", "list_routes", "export_device", "send", "get_status",
                     "detect_hardware", "backup_presets", "restore_presets",
                     "add_link", "remove_link", "list_links",
                     "list_roles", "set_by_role", "get_by_role", "install_export"] {
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
        "params": {"name": name, "arguments": {"device": "no-such-device"}}}), &opts, &idx, None).unwrap();
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
    let err = call_tool("restore_presets", &json!({"device": "acme/box", "out_port": "0", "file": "x.syx"}), &opts, &idx, None).unwrap_err();
    assert!(err.contains("confirm_overwrite"), "{err}");
}

#[test]
fn port_names_and_identities_resolve_to_their_driver() {
    let cat = hardware_candidates(&fixtures());
    for (port, want) in [("minilogue MIDI 1", "korg/minilogue"), ("Bass Station II", "novation/bass_station_ii"),
                         ("Electra Controller Electra Port 1", "electra-one/electra-one")] {
        let best = match_port(&cat, port, None, &[]);
        assert_eq!(best.first().map(|c| c["id"].clone()), Some(json!(want)), "{port}: {best:?}");
    }
    // An Identity Reply from the maker puts its driver first.
    let acme = crate::detect::Identity { manufacturer: vec![0, 33, 9], family: 1, model: 1, version: [2, 0, 0, 0] };
    let best = match_port(&cat, "Box", None, &[acme.reply()]);
    assert_eq!(best[0]["slug"], "box", "{best:?}");
    // Its identity block names the firmware, which picks the variant for it.
    assert_eq!(best[0]["id"], "acme/box.vendor.fw-2", "{best:?}");
    assert!(best[0].get("device").is_none() && best[0].get("osc_prefix").is_none(), "the published form of a candidate: {best:?}");
    assert_eq!(best[0]["firmware"].as_str(), Some("2.0.0.0"));
    assert!(best[0]["proofs"].as_array().unwrap().contains(&json!({ "nature": "strong", "kind": "identity reply" })), "{best:?}");
    assert!(best[0].get("recognised_by").is_none(), "{best:?}");
    let nameless = match_port(&cat, "USB MIDI Interface", None, &[acme.reply()]);
    assert_eq!(nameless[0]["id"],"acme/box.vendor.fw-2", "behind a port named after nothing: {nameless:?}");
    let other_fw = crate::detect::Identity { version: [3, 0, 0, 0], ..acme.clone() };
    assert!(match_port(&cat, "Box", None, &[other_fw.reply()])[0]["score"].as_u64() < best[0]["score"].as_u64());
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
                        json!({ "port": p["port"], "candidates": match_port(&cat, p["port"].as_str().unwrap(), None, &replies) })
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
    let (base, calls) = fake_server(Path::new(FIXTURES));
    let link = Link { client: crate::online::Online::keeping_account_in(&base, Some(tmp.join("user"))), ..Link::new("unused") };

    // By slug, its best variant; with its notes and the driver it takes its MIDI from.
    let fetched = crate::catalogue::fetch(&link.client, "microfreak_tools", &cache).unwrap();
    assert_eq!(fetched, cache.join("arturia/microfreak.vendor.fw-microfreak-tools-0.1.0.json"));
    assert!(fetched.with_extension("md").is_file() && cache.join("arturia/microfreak.json").is_file());
    assert_eq!(crate::device::load(&fetched).unwrap().device.osc_prefix, "/microfreak_tools");
    assert_eq!(crate::catalogue::fetch(&link.client, "box", &cache).unwrap(), cache.join("acme/box.vendor.fw-2.json"));
    assert!(crate::catalogue::fetch(&link.client, "acme/none", &cache).unwrap_err().to_string().contains("no driver acme/none"));
    assert!(link.client.fetch_driver("../secret", &cache).is_err());
    assert_eq!(DeviceIndex::build(&cache).entries.len(), 3, "only what was asked for is on this machine");

    // The server lists and matches the drivers; what is cached here is not the catalogue.
    let opts = McpOptions { online: true, devices_dir: None, ..test_opts() };
    let index = DeviceIndex::build(&cache);
    let listed = call_tool("list_devices", &json!({"query": "minilogue"}), &opts, &index, Some(&link)).unwrap();
    assert_eq!(listed["structuredContent"]["devices"][0]["device"], "korg/minilogue", "{listed}");
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

/// A stand-in orchestrator on a free UDP port: it answers the first message
/// it receives with `replies` (sent back to the asker) and hands that message
/// to the test.
fn fake_orchestrator(replies: Vec<rosc::OscMessage>) -> (SocketAddr, std::sync::mpsc::Receiver<rosc::OscMessage>) {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        let Ok((n, from)) = sock.recv_from(&mut buf) else { return };
        if let Ok((_, rosc::OscPacket::Message(m))) = rosc::decoder::decode_udp(&buf[..n]) { let _ = tx.send(m); }
        for r in replies {
            let _ = sock.send_to(&rosc::encoder::encode(&rosc::OscPacket::Message(r)).unwrap(), from);
        }
    });
    (addr, rx)
}

fn osc(addr: &str, args: Vec<rosc::OscType>) -> rosc::OscMessage {
    rosc::OscMessage { addr: addr.into(), args }
}

fn ostr(s: &str) -> rosc::OscType { rosc::OscType::String(s.into()) }

#[test]
fn add_link_sends_the_source_the_target_and_the_scaling_to_the_orchestrator_and_gives_its_reply() {
    let (addr, got) = fake_orchestrator(vec![osc("/bridge/link/added", vec![ostr("/ctl/knob"), ostr("/synth/cutoff")])]);
    let args = json!({"from": "/ctl/knob", "to": "/synth/cutoff", "map": {"from": [0, 127], "to": [0, 1]},
        "target": addr.to_string()});
    let res = call_tool("add_link", &args, &test_opts(), &empty_index(), None).unwrap();
    let sent = got.recv_timeout(Duration::from_secs(5)).unwrap();
    use rosc::OscType::Float;
    assert_eq!(sent.addr, "/bridge/link/add");
    assert_eq!(sent.args, vec![ostr("/ctl/knob"), ostr("/synth/cutoff"), Float(0.0), Float(127.0), Float(0.0), Float(1.0)]);
    let sc = &res["structuredContent"];
    assert_eq!(sc["replied"], json!(true));
    assert_eq!(sc["target"], json!(addr.to_string()));
    assert_eq!(sc["replies"][0]["addr"], json!("/bridge/link/added"));
    assert_eq!(sc["replies"][0]["args"], json!(["/ctl/knob", "/synth/cutoff"]));
}

#[test]
fn add_link_without_scaling_sends_only_the_two_addresses() {
    let (addr, got) = fake_orchestrator(vec![osc("/bridge/link/added", vec![])]);
    let args = json!({"from": "/a", "to": "/b", "target": addr.to_string()});
    call_tool("add_link", &args, &test_opts(), &empty_index(), None).unwrap();
    let sent = got.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(sent.args, vec![ostr("/a"), ostr("/b")]);
}

#[test]
fn add_link_gives_the_orchestrators_refusal_as_it_is() {
    let (addr, _got) = fake_orchestrator(vec![osc("/bridge/link/error", vec![ostr("no device owns /ghost/x")])]);
    let args = json!({"from": "/ghost/x", "to": "/b", "target": addr.to_string()});
    let res = call_tool("add_link", &args, &test_opts(), &empty_index(), None).unwrap();
    let sc = &res["structuredContent"];
    assert_eq!(sc["replied"], json!(true));
    assert_eq!(sc["replies"][0]["addr"], json!("/bridge/link/error"));
    assert_eq!(sc["replies"][0]["args"][0], json!("no device owns /ghost/x"));
}

#[test]
fn add_link_without_a_source_or_a_target_is_a_tool_error() {
    let opts = test_opts();
    assert!(call_tool("add_link", &json!({"to": "/b"}), &opts, &empty_index(), None).unwrap_err().contains("from"));
    assert!(call_tool("add_link", &json!({"from": "/a"}), &opts, &empty_index(), None).unwrap_err().contains("to"));
}

#[test]
fn remove_link_sends_the_source_and_the_target_and_gives_the_reply() {
    let (addr, got) = fake_orchestrator(vec![osc("/bridge/link/removed", vec![ostr("/a"), ostr("/b")])]);
    let args = json!({"from": "/a", "to": "/b", "target": addr.to_string()});
    let res = call_tool("remove_link", &args, &test_opts(), &empty_index(), None).unwrap();
    let sent = got.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!((sent.addr.as_str(), sent.args), ("/bridge/link/remove", vec![ostr("/a"), ostr("/b")]));
    assert_eq!(res["structuredContent"]["replies"][0]["addr"], json!("/bridge/link/removed"));
}

#[test]
fn list_links_gives_every_reply_of_the_orchestrator() {
    use rosc::OscType::{Float, Int};
    let (addr, got) = fake_orchestrator(vec![
        osc("/bridge/link/list/item", vec![ostr("/a"), ostr("/b"), ostr("file")]),
        osc("/bridge/link/list/item", vec![ostr("/c"), ostr("/d"), ostr("live"), Float(0.0), Float(127.0), Float(0.0), Float(1.0)]),
        osc("/bridge/link/list/end", vec![Int(2)]),
    ]);
    let res = call_tool("list_links", &json!({"target": addr.to_string()}), &test_opts(), &empty_index(), None).unwrap();
    assert_eq!(got.recv_timeout(Duration::from_secs(5)).unwrap().addr, "/bridge/link/list");
    let replies = res["structuredContent"]["replies"].as_array().unwrap();
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[1]["args"][2], json!("live"));
    assert_eq!(replies[2]["addr"], json!("/bridge/link/list/end"));
}

#[test]
fn the_link_tools_say_no_orchestrator_answers_without_changing_anything() {
    // A bound port nobody reads: the tool times out and says so.
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target = silent.local_addr().unwrap().to_string();
    let opts = test_opts();
    for (tool, args) in [("add_link", json!({"from": "/a", "to": "/b", "target": target})),
                         ("remove_link", json!({"from": "/a", "to": "/b", "target": target})),
                         ("list_links", json!({"target": target}))] {
        let res = call_tool(tool, &args, &opts, &empty_index(), None).unwrap();
        let sc = &res["structuredContent"];
        assert_eq!(sc["replied"], json!(false), "{tool}");
        assert_eq!(sc["replies"], json!([]), "{tool}");
        assert!(res["content"][0]["text"].as_str().unwrap().contains("No reply"), "{tool}: {res}");
    }
}

#[test]
fn the_link_tools_default_to_the_default_target() {
    let (addr, got) = fake_orchestrator(vec![osc("/bridge/link/list/end", vec![rosc::OscType::Int(0)])]);
    let opts = McpOptions { default_target: addr, ..test_opts() };
    call_tool("list_links", &json!({}), &opts, &empty_index(), None).unwrap();
    assert_eq!(got.recv_timeout(Duration::from_secs(5)).unwrap().addr, "/bridge/link/list");
}

#[test]
fn export_device_returns_what_the_format_lost_beside_the_document() {
    let idx = fixtures();
    for (format, tag) in [("midnam", "<MIDINameDocument>"), ("oscquery", "\"FULL_PATH\"")] {
        let out = catalogue_tool("export_device", &json!({"device": "arturia/microfreak.vendor.fw-microfreak-tools-0.1.0", "format": format}), &idx, &|_| None)
            .unwrap().unwrap();
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(tag), "{format}: the content stays the document");
        assert!(!text.contains("/_sources/0/firmware"), "{format}: the document holds no list of losses");
        let lost = out["structuredContent"]["lost"].as_array().unwrap_or_else(|| panic!("{format}: no lost list: {out}"));
        assert!(lost.iter().any(|l| l["place"] == "/_sources/0/firmware" && l["reason"].as_str().is_some_and(|r| !r.is_empty())), "{format}: {lost:?}");
        assert_eq!(out["structuredContent"]["media_type"], if format == "midnam" { "application/xml" } else { "application/json" });
        assert_eq!(out["structuredContent"]["bytes"], text.len());
    }
}

#[test]
fn export_device_for_an_osc_only_device_and_an_unknown_format_keep_their_messages() {
    let idx = fixtures();
    let err = catalogue_tool("export_device", &json!({"device": "bitwig/bitwig-studio.third-party-osc.fw-5", "format": "midnam"}), &idx, &|_| None)
        .unwrap().unwrap_err();
    assert!(err.contains("speaks OSC, not MIDI: it has no MIDI names"), "{err}");
    let err = catalogue_tool("export_device", &json!({"device": "minilogue", "format": "ctrlr"}), &idx, &|_| None)
        .unwrap().unwrap_err();
    assert!(err.contains("unknown format 'ctrlr' (one of: "), "{err}");
    assert!(err.contains("oscquery") && err.contains("midnam") && err.contains("deluge"), "{err}");
}

/// A stand-in bridge on a free UDP port: it hands the first packet it receives to the test
/// (every message of it, a bundle flattened), then answers like osc-bridge does: `faults`
/// first, then `/bridge/done` echoing the arguments of the `/bridge/sync` it was asked with,
/// when the packet carried one.
fn fake_bridge(faults: Vec<rosc::OscMessage>) -> (SocketAddr, std::sync::mpsc::Receiver<Vec<rosc::OscMessage>>) {
    fn flatten(pkt: rosc::OscPacket, out: &mut Vec<rosc::OscMessage>) {
        match pkt {
            rosc::OscPacket::Message(m) => out.push(m),
            rosc::OscPacket::Bundle(b) => for p in b.content { flatten(p, out); },
        }
    }
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        let Ok((n, from)) = sock.recv_from(&mut buf) else { return };
        let mut got = Vec::new();
        if let Ok((_, pkt)) = rosc::decoder::decode_udp(&buf[..n]) { flatten(pkt, &mut got); }
        let sync = got.iter().find(|m| m.addr == "/bridge/sync").cloned();
        let _ = tx.send(got);
        let mut replies = faults;
        if let Some(s) = sync { replies.push(osc("/bridge/done", s.args)); }
        for r in replies {
            let _ = sock.send_to(&rosc::encoder::encode(&rosc::OscPacket::Message(r)).unwrap(), from);
        }
    });
    (addr, rx)
}

fn send_args(addr: SocketAddr) -> Value {
    json!({"addr": "/synth/cutoff", "args": [64], "target": addr.to_string()})
}

#[test]
fn send_asks_the_bridge_for_its_acknowledgement_in_the_packet_of_the_message() {
    let (addr, got) = fake_bridge(vec![]);
    call_tool("send", &send_args(addr), &test_opts(), &empty_index(), None).unwrap();
    let seen = got.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].addr, "/synth/cutoff");
    assert_eq!(seen[0].args, vec![rosc::OscType::Int(64)]);
    assert_eq!(seen[1].addr, "/bridge/sync");
    assert_eq!(seen[1].args.len(), 1, "one id: {seen:?}");
}

#[test]
fn send_gives_sent_true_when_the_bridge_says_the_message_is_done() {
    let (addr, _got) = fake_bridge(vec![]);
    let res = call_tool("send", &send_args(addr), &test_opts(), &empty_index(), None).unwrap();
    assert_ne!(res["isError"], json!(true), "{res}");
    let sc = &res["structuredContent"];
    assert_eq!(sc["sent"], json!(true), "{res}");
    assert_eq!(sc["address"], json!("/synth/cutoff"));
    assert_eq!(sc["target"], json!(addr.to_string()));
}

#[test]
fn send_gives_an_error_result_with_the_address_and_the_reason_of_a_fault_the_bridge_sends_back() {
    let fault = osc("/bridge/fault", vec![ostr("/synth/cutoff"), ostr("missing arg value for /synth/cutoff")]);
    let (addr, _got) = fake_bridge(vec![fault]);
    let res = call_tool("send", &send_args(addr), &test_opts(), &empty_index(), None).unwrap();
    assert_eq!(res["isError"], json!(true), "{res}");
    let text = res["content"][0]["text"].as_str().unwrap();
    for part in ["/synth/cutoff", "missing arg value for /synth/cutoff"] {
        assert!(text.contains(part), "{part} missing from {text}");
    }
    let sc = &res["structuredContent"];
    assert_eq!(sc["address"], json!("/synth/cutoff"));
    assert_eq!(sc["reason"], json!("missing arg value for /synth/cutoff"));
    assert_ne!(sc["sent"], json!(true), "a message that did not leave is not sent: {res}");
}

#[test]
fn send_says_the_bridge_does_not_answer_at_that_address_when_nothing_replies() {
    // A bound port nobody reads: the tool times out and says so.
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = silent.local_addr().unwrap();
    let outcome = call_tool("send", &send_args(addr), &test_opts(), &empty_index(), None);
    let (is_error, text) = match &outcome {
        Err(e) => (true, e.clone()),
        Ok(v) => (v["isError"] == json!(true), v["content"][0]["text"].as_str().unwrap_or_default().to_string()),
    };
    assert!(is_error, "silence is not a sent message: {outcome:?}");
    assert!(text.contains(&addr.to_string()), "the address that did not answer: {text}");
}

#[test]
fn the_local_tools_that_name_a_device_take_device_and_no_slug() {
    let schemas = tool_schemas();
    let tools = schemas["tools"].as_array().unwrap();
    let tool = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap_or_else(|| panic!("no tool {n}"));
    for name in ["get_device_docs", "list_routes"] {
        let t = tool(name);
        assert!(t["inputSchema"]["properties"]["device"].is_object(), "{name} takes device: {t}");
        assert_eq!(t["inputSchema"]["required"], json!(["device"]), "{name}");
    }
    assert_eq!(tool("get_device_docs")["outputSchema"]["required"], json!(["device", "has_docs", "markdown"]));
    assert!(tool("get_device_docs")["outputSchema"]["properties"]["device"].is_object());
    assert!(tool("get_device_docs")["outputSchema"]["properties"].get("slug").is_none());
    for t in tools {
        assert!(t["inputSchema"]["properties"].get("slug").is_none(), "{} still takes slug", t["name"]);
        let described = serde_json::to_string(t).unwrap();
        assert!(!described.contains("or slug"), "{} says 'or slug': {described}", t["name"]);
    }
    let entry = &tool("list_devices")["outputSchema"]["properties"]["devices"]["items"];
    assert!(entry["properties"]["device"].is_object(), "{entry}");
    assert!(entry["properties"].get("slug").is_none() && entry["properties"].get("id").is_none(), "{entry}");
    assert!(entry["properties"]["osc_prefix"].is_object(), "{entry}");
    let req: Vec<&str> = entry["required"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(req.contains(&"osc_prefix") && !req.contains(&"slug"), "{req:?}");
}

#[test]
fn a_call_that_passes_slug_is_refused_with_an_error_naming_device_before_any_device_is_read() {
    let idx = fixtures();
    let opts = test_opts();
    let tools = ["get_device_docs", "list_routes", "export_device", "list_roles", "get_by_role", "set_by_role"];
    for tool in tools {
        for args in [json!({"slug": "minilogue", "format": "midnam"}), json!({"slug": "minilogue", "device": "minilogue", "format": "midnam"})] {
            let err = call_tool(tool, &args, &opts, &idx, None).unwrap_err();
            assert!(err.contains("slug") && err.contains("device"), "{tool} {args}: {err}");
            assert!(err.contains("no longer"), "{tool}: {err}");
        }
    }
    // Refused whatever the device is, even one the catalogue does not know.
    let err = catalogue_tool("list_routes", &json!({"slug": "no-such", "device": "no-such"}), &idx, &|_| None).unwrap().unwrap_err();
    assert!(err.contains("slug") && err.contains("device") && !err.contains("no device"), "{err}");
}

#[test]
fn a_call_without_device_says_device_is_missing() {
    let idx = fixtures();
    for tool in ["get_device_docs", "list_routes"] {
        let err = catalogue_tool(tool, &json!({}), &idx, &|_| None).unwrap().unwrap_err();
        assert!(err.contains("device") && !err.contains("slug"), "{tool}: {err}");
    }
}

#[test]
fn an_unknown_device_is_named_as_a_device() {
    let idx = fixtures();
    let err = list_routes(&idx, "no-such-device").unwrap_err();
    assert!(err.contains("no-such-device") && err.contains("device") && !err.contains("slug"), "{err}");
}

#[test]
fn get_device_docs_gives_back_the_device_as_the_caller_named_it() {
    let idx = fixtures();
    let docs = catalogue_tool("get_device_docs", &json!({"device": "electra1"}), &idx, &|_| None).unwrap().unwrap();
    let sc = &docs["structuredContent"];
    assert_eq!(sc["device"], "electra1", "{sc}");
    assert_eq!(sc["has_docs"], json!(true));
    assert!(sc["markdown"].as_str().is_some_and(|m| !m.is_empty()));
    assert!(sc.get("slug").is_none(), "{sc}");
    let none = catalogue_tool("get_device_docs", &json!({"device": "box"}), &idx, &|_| None).unwrap().unwrap();
    assert_eq!(none["structuredContent"]["device"], "box");
    assert!(none["structuredContent"]["markdown"].as_str().unwrap().contains("'box'"));
}

#[test]
fn list_devices_names_each_device_by_its_catalogue_id_and_its_prefix_by_osc_prefix() {
    let idx = fixtures();
    let res = list_devices(&idx, &DeviceFilter::default(), &|_| None).unwrap();
    let devices = res["structuredContent"]["devices"].as_array().unwrap();
    assert!(!devices.is_empty());
    for (d, e) in devices.iter().zip(&idx.entries) {
        assert_eq!(d["device"], json!(e.id), "{d}");
        assert_eq!(d["osc_prefix"], json!(e.slug), "{d}");
        assert!(d.get("id").is_none() && d.get("slug").is_none(), "no second name for the device: {d}");
    }
    // What list_devices gives is what list_routes takes back.
    let first = devices.iter().find(|d| d["device"] == "korg/minilogue").expect("minilogue listed");
    let back = catalogue_tool("list_routes", &json!({"device": first["osc_prefix"]}), &idx, &|_| None).unwrap();
    assert!(back.is_ok(), "{back:?}");
}

#[test]
fn a_port_with_the_usb_ids_of_a_driver_is_recognised_by_usb() {
    let text = r#"{"$schema":"https://w3id.org/osd/0.5/device.schema.json","device":{"name":"Box","osc_prefix":"/box"},
        "identity":{"usb":[{"vendor_id":4660,"product_id":22136}]}}"#;
    let identity = match osd::read(text, osd::Kind::Device).unwrap() {
        osd::Descriptor::Device(d) => d.identity.unwrap(),
        _ => unreachable!(),
    };
    let cat = vec![crate::detect::Candidate { id: "acme/box".into(), slug: "box".into(), name: "Box".into(),
        vendor: "Acme".into(), file: "devices/box.json".into(), tier: "pencilresearch".into(),
        manufacturer_id: vec![], identity: Some(identity), firmware: None, midi_of: None }];
    let best = match_port(&cat, "USB MIDI Interface", Some((4660, 22136)), &[]);
    assert_eq!(best[0]["id"], "acme/box", "{best:?}");
    assert!(best[0]["proofs"].as_array().unwrap().contains(&json!({ "nature": "strong", "kind": "usb id" })), "{best:?}");
    assert!(match_port(&cat, "USB MIDI Interface", None, &[]).is_empty());
    assert!(match_port(&cat, "USB MIDI Interface", Some((1, 2)), &[]).is_empty());
}

fn usb_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.trim_start().starts_with("usb:")).collect()
}

#[test]
fn detect_prints_the_usb_ids_under_the_port_before_the_identity() {
    let devices = [json!({
        "port": "Minilab3 MIDI", "out_port": 1, "in_port": 2,
        "usb": { "vendor_id": 0x1c75, "product_id": 0x020b },
        "identity": { "manufacturer": "Arturia", "manufacturer_id": "00 20 6B", "family": 1, "model": 2, "firmware": "1.0" },
        "candidates": [{ "device": "arturia-minilab-3", "file": "arturia/minilab-3.json", "source_tier": "official", "score": 9 }]
    })];
    let lines: Vec<String> = format_detection(&devices, true).lines().map(String::from).collect();
    assert!(lines[0].starts_with("Minilab3 MIDI"), "{lines:?}");
    assert_eq!(lines[1], "  usb: 1c75:020b");
    assert!(lines[2].starts_with("  identity:"), "{lines:?}");
}

#[test]
fn detect_pads_the_usb_ids_to_four_lower_case_hex_digits() {
    let devices = [
        json!({ "port": "A", "usb": { "vendor_id": 0x1397, "product_id": 0x12cf } }),
        json!({ "port": "B", "usb": { "vendor_id": 0x1fc9, "product_id": 0x82cf } }),
        json!({ "port": "C", "usb": { "vendor_id": 0x0a, "product_id": 0xABC } }),
        json!({ "port": "D", "usb": { "vendor_id": 0, "product_id": 0xffff } }),
    ];
    let text = format_detection(&devices, false);
    assert_eq!(usb_lines(&text), ["  usb: 1397:12cf", "  usb: 1fc9:82cf", "  usb: 000a:0abc", "  usb: 0000:ffff"]);
}

#[test]
fn detect_prints_the_usb_ids_of_a_port_with_no_identity_and_no_match() {
    let devices = [json!({ "port": "Mystery", "out_port": 0, "in_port": 0, "usb": { "vendor_id": 0x1234, "product_id": 0x5678 } })];
    for probe in [false, true] {
        let text = format_detection(&devices, probe);
        assert_eq!(usb_lines(&text), ["  usb: 1234:5678"], "probe={probe}");
        assert!(!text.contains("match:"));
    }
    let probed = format_detection(&devices, true);
    let lines: Vec<&str> = probed.lines().collect();
    assert_eq!(lines[1], "  usb: 1234:5678");
    assert_eq!(lines[2], "  identity: no reply");
}

#[test]
fn detect_prints_no_usb_line_for_a_port_without_usb_ids() {
    let devices = [json!({ "port": "Virtual", "out_port": 3, "in_port": 3 })];
    assert!(usb_lines(&format_detection(&devices, true)).is_empty());
}

#[test]
fn detect_prints_no_usb_line_and_does_not_panic_on_malformed_usb_ids() {
    let bad = [
        json!({ "port": "a", "usb": { "vendor_id": 0x10000, "product_id": 1 } }),
        json!({ "port": "b", "usb": { "vendor_id": 1, "product_id": 65536 } }),
        json!({ "port": "c", "usb": { "vendor_id": -1, "product_id": 1 } }),
        json!({ "port": "d", "usb": { "vendor_id": "1c75", "product_id": 1 } }),
        json!({ "port": "e", "usb": { "vendor_id": 1.5, "product_id": 1 } }),
        json!({ "port": "f", "usb": { "vendor_id": 1 } }),
        json!({ "port": "g", "usb": {} }),
        json!({ "port": "h", "usb": null }),
        json!({ "port": "i", "usb": "1c75:020b" }),
    ];
    let text = format_detection(&bad, true);
    assert!(usb_lines(&text).is_empty(), "{text}");
    for p in ["a", "b", "c", "d", "e", "f", "g", "h", "i"] {
        assert!(text.lines().any(|l| l.starts_with(p)), "port {p} missing");
    }
}

// ---- detect_hardware: the USB ids go out, a device without a card comes back being confirmed ----

const BEING_CONFIRMED: &str = "being confirmed: not enough people agree yet, to confirm with the person";
const IN_DISAGREEMENT: &str = "in disagreement: several devices are named for it, to confirm with the person";

/// A service that answers each detect request with `answer(port)` and keeps the requests.
fn detect_service(answer: fn(&Value) -> Value) -> (crate::online::Online, std::sync::Arc<std::sync::Mutex<Vec<Value>>>, tempfile::TempDir) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let seen = bodies.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
            let asked: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            seen.lock().unwrap().push(asked.clone());
            let ports: Vec<Value> = asked["ports"].as_array().into_iter().flatten().map(answer).collect();
            let _ = req.respond(tiny_http::Response::from_string(json!({ "ports": ports }).to_string()));
        }
    });
    let dir = tempfile::Builder::new().prefix("osc-bridge-detect-").tempdir().unwrap();
    let account = dir.path().join("user");
    std::fs::write(&account, "0123456789abcdef0123456789abcdef\n").unwrap();
    (crate::online::Online::keeping_account_in(&base, Some(account)), bodies, dir)
}

fn probed(name: &str, usb: Option<(u16, u16)>) -> crate::detect::Probed {
    crate::detect::Probed { out_index: Some(0), out_name: Some(name.into()), in_index: Some(0), in_name: Some(name.into()),
        identity: None, usb, replies: Vec::new(), client_kind: crate::detect::ClientKind::Unknown }
}

fn report_devices(server: &crate::online::Online, found: Vec<crate::detect::Probed>) -> Vec<Value> {
    let r = detection_report(&fixtures(), Some(server), true, true, found, Vec::new(), &[]).unwrap();
    r["structuredContent"]["devices"].as_array().unwrap().clone()
}

fn bcr_unlisted(_: &Value) -> Value {
    json!({ "candidates": [], "unlisted": [{ "vendor": "Behringer", "model": "BCR32", "firmware": "1.0.3",
        "confirmations": 1, "disputed": false, "state": "confirming", "state_label": BEING_CONFIRMED }] })
}

#[test]
fn the_detection_sends_the_usb_ids_of_a_port_that_has_them_as_numbers_and_none_otherwise() {
    let (client, bodies, _dir) = detect_service(|_| json!({ "candidates": [] }));
    report_devices(&client, vec![probed("BCR32", Some((0x1397, 0x12cf))), probed("Virtual", None)]);
    let sent = bodies.lock().unwrap().clone();
    let ports = sent[0]["ports"].as_array().unwrap();
    assert_eq!(ports[0]["usb"], json!({ "vendor_id": 5015, "product_id": 4815 }), "{sent:?}");
    assert!(ports[1].get("usb").is_none(), "{sent:?}");
    assert_eq!(ports[0]["replies_hex"], json!([]));
}

#[test]
fn the_detection_returns_a_device_without_a_card_being_confirmed_with_all_its_fields() {
    let (client, _bodies, _dir) = detect_service(bcr_unlisted);
    let devices = report_devices(&client, vec![probed("BCR32", Some((0x1397, 0x12cf)))]);
    let u = &devices[0]["unlisted"][0];
    assert_eq!((u["vendor"].clone(), u["model"].clone(), u["firmware"].clone()), (json!("Behringer"), json!("BCR32"), json!("1.0.3")), "{devices:?}");
    assert_eq!(u["confirmations"], 1);
    assert_eq!(u["state"], "confirming");
    assert_eq!(u["state_label"], BEING_CONFIRMED);
    assert_eq!(devices[0]["candidates"], json!([]));
    assert!(devices[0]["run_command"].is_null(), "proposed, never run: {devices:?}");
}

#[test]
fn the_detection_returns_every_device_without_a_card_and_their_dispute() {
    let (client, _bodies, _dir) = detect_service(|_| json!({ "candidates": [], "unlisted": [
        { "vendor": "Behringer", "model": "BCR32", "firmware": "1.0.3", "confirmations": 2, "disputed": true,
          "state": "confirming", "state_label": IN_DISAGREEMENT },
        { "vendor": "Behringer", "model": "BCR2000", "firmware": "1.0.3", "confirmations": 1, "disputed": true,
          "state": "confirming", "state_label": IN_DISAGREEMENT }] }));
    let devices = report_devices(&client, vec![probed("BCR32", None)]);
    let all = devices[0]["unlisted"].as_array().unwrap();
    assert_eq!(all.len(), 2, "{devices:?}");
    assert!(all.iter().all(|u| u["disputed"] == true && u["state_label"] == IN_DISAGREEMENT));
    assert_eq!((all[0]["confirmations"].clone(), all[1]["confirmations"].clone()), (json!(2), json!(1)));
}

#[test]
fn the_detection_reports_catalogue_candidates_and_never_also_a_device_without_a_card() {
    let (client, _bodies, _dir) = detect_service(|_| json!({
        "candidates": [{ "id": "acme/box", "slug": "box", "name": "Box", "vendor": "Acme", "source_tier": "official", "score": 90 }],
        "unlisted": [{ "vendor": "Behringer", "model": "BCR32", "firmware": "1.0.3", "confirmations": 1, "disputed": false }] }));
    let devices = report_devices(&client, vec![probed("Box", None)]);
    assert_eq!(devices[0]["candidates"][0]["device"], "acme/box", "{devices:?}");
    assert!(devices[0].get("unlisted").is_none(), "{devices:?}");
}

const POLYSYNTH: fn(&Value) -> Value = |_| json!({ "candidates": [
    { "id": "polyend/synth", "slug": "polysynth", "name": "Synth", "vendor": "Polyend", "source_tier": "vendor-doc", "score": 35 }],
    "devices": [] });

#[test]
fn a_catalogue_candidate_of_the_service_is_reported_as_a_candidate_under_its_catalogue_id_and_never_as_a_device_without_a_card() {
    let (client, _bodies, _dir) = detect_service(POLYSYNTH);
    let devices = report_devices(&client, vec![probed("FLUID Synth", None)]);
    let cands = devices[0]["candidates"].as_array().unwrap();
    assert_eq!(cands.len(), 1, "{devices:?}");
    assert_eq!(cands[0]["device"], "polyend/synth", "{devices:?}");
    assert_eq!(cands[0]["name"], "Synth", "{devices:?}");
    assert!(devices[0].get("unlisted").is_none(), "{devices:?}");
}

#[test]
fn the_command_prints_a_catalogue_candidate_of_the_service_by_its_name_and_id_with_no_empty_field() {
    let (client, _bodies, _dir) = detect_service(POLYSYNTH);
    let devices = report_devices(&client, vec![probed("FLUID Synth", None)]);
    let text = format_detection(&devices, false);
    assert!(text.contains("polyend/synth") && text.contains("Synth"), "{text}");
    assert!(!text.contains("unlisted") && !text.contains('?') && !text.contains("null"), "{text}");
}

#[test]
fn the_command_prints_a_device_without_a_card_without_the_field_it_lacks() {
    let devices = [json!({ "port": "P", "out_port": 0, "in_port": 0, "candidates": [],
        "unlisted": [{ "vendor": "Arturia" }, { "model": "Thing", "firmware": "2", "confirmations": 3 }] })];
    let text = format_detection(&devices, false);
    assert!(text.contains("Arturia") && text.contains("Thing"), "{text}");
    assert!(!text.contains('?') && !text.contains("null"), "{text}");
}

#[test]
fn the_command_does_not_print_an_entry_without_vendor_or_model_as_a_device() {
    let devices = [json!({ "port": "P", "out_port": 0, "in_port": 0, "candidates": [],
        "unlisted": [{ "firmware": "2", "confirmations": 3 }] })];
    let text = format_detection(&devices, false);
    assert!(!text.contains("unlisted") && !text.contains("firmware 2"), "{text}");
}

#[test]
fn the_command_prints_the_bcr_32_with_its_maker_model_firmware_confirmations_and_state_label() {
    let (client, _bodies, _dir) = detect_service(bcr_unlisted);
    let devices = report_devices(&client, vec![probed("BCR32", None)]);
    let text = format_detection(&devices, false);
    assert!(text.contains("Behringer BCR32, firmware 1.0.3, 1 confirmation(s)") && text.contains(BEING_CONFIRMED), "{text}");
}

#[test]
fn the_detection_reports_a_catalogue_candidate_the_service_gives_as_being_confirmed_so() {
    let (client, _bodies, _dir) = detect_service(|_| json!({
        "candidates": [{ "id": "acme/box", "slug": "box", "name": "Box", "vendor": "Acme", "source_tier": "official", "score": 90 }],
        "learned": [{ "id": "acme/box", "confirmations": 2, "disputed": false, "state": "confirming", "state_label": BEING_CONFIRMED }] }));
    let devices = report_devices(&client, vec![probed("Box", None)]);
    let c = &devices[0]["candidates"][0];
    assert_eq!(c["state"], "confirming", "{devices:?}");
    assert_eq!(c["state_label"], BEING_CONFIRMED);
    assert_eq!(c["confirmations"], 2);
}

#[test]
fn the_detection_without_a_service_reports_no_device_being_confirmed() {
    let r = detection_report(&fixtures(), None, true, true, vec![probed("BCR32", Some((0x1397, 0x12cf)))], Vec::new(), &[]).unwrap();
    assert!(r["structuredContent"]["devices"][0].get("unlisted").is_none(), "{r}");
}

#[test]
fn a_device_being_confirmed_still_has_every_maker_request_held_back_without_consent() {
    use crate::capture::{Capture, FORMAT_VERSION};
    const MYSTERY: &str = "Mystery Box:Mystery Box 20:0";
    let capture = Capture::parse(&json!({
        "format_version": FORMAT_VERSION, "outputs": [MYSTERY], "inputs": [MYSTERY],
        "cards": "", "usbids": [], "clients": "", "requests": [], "received": [],
    }).to_string()).unwrap();
    let probes = vec![crate::detect::Probe { request: vec![0xF0, 0x11, 0xF7], ports: vec![], usb: vec![(0x1c75, 0x020b)], match_ports: vec![] },
                      crate::detect::Probe { request: vec![0xF0, 0x12, 0xF7], ports: vec![], usb: vec![], match_ports: vec![] }];
    let plan = held_plan(&capture, &probes, &[]).unwrap();
    let (client, _bodies, _dir) = detect_service(bcr_unlisted);
    let with = detection_report(&fixtures(), Some(&client), true, true, crate::detect::replay(&capture), Vec::new(), &plan).unwrap();
    let without = detection_report(&fixtures(), None, true, true, crate::detect::replay(&capture), Vec::new(), &plan).unwrap();
    let held = &with["structuredContent"]["devices"][0]["held_back"];
    assert_eq!(held, &json!([hex_bytes(&[0xF0, 0x11, 0xF7]), hex_bytes(&[0xF0, 0x12, 0xF7])]), "{with}");
    assert_eq!(held, &without["structuredContent"]["devices"][0]["held_back"]);
    assert!(with["structuredContent"]["devices"][0].get("unlisted").is_some(), "{with}");
}

#[test]
fn the_command_prints_a_device_being_confirmed_with_its_maker_model_firmware_confirmations_and_state() {
    let devices = [json!({ "port": "BCR32", "out_port": 0, "in_port": 0, "candidates": [],
        "unlisted": [{ "vendor": "Behringer", "model": "BCR32", "firmware": "1.0.3", "confirmations": 4,
            "disputed": false, "state": "confirming", "state_label": BEING_CONFIRMED }] })];
    let text = format_detection(&devices, true);
    for want in ["Behringer", "BCR32", "1.0.3", "4", BEING_CONFIRMED] {
        assert!(text.contains(want), "{want} in {text}");
    }
}

#[test]
fn the_command_prints_every_device_without_a_card_and_the_dispute() {
    let devices = [json!({ "port": "BCR32", "out_port": 0, "in_port": 0, "candidates": [],
        "unlisted": [
          { "vendor": "Behringer", "model": "BCR32", "firmware": "1.0.3", "confirmations": 2, "disputed": true, "state": "confirming", "state_label": IN_DISAGREEMENT },
          { "vendor": "Zork", "model": "Thing", "firmware": "9", "confirmations": 3, "disputed": true, "state": "confirming", "state_label": IN_DISAGREEMENT }] })];
    let text = format_detection(&devices, false);
    assert!(text.contains("BCR32") && text.contains("Thing") && text.contains("Zork"), "{text}");
    assert!(text.matches(IN_DISAGREEMENT).count() >= 2, "{text}");
}

#[test]
fn the_command_prints_a_catalogue_candidate_being_confirmed_with_its_state() {
    let devices = [json!({ "port": "Box", "out_port": 0, "in_port": 0,
        "candidates": [{ "device": "acme/box", "file": "acme/box.json", "score": 9, "state": "confirming", "state_label": BEING_CONFIRMED }] })];
    assert!(format_detection(&devices, false).contains(BEING_CONFIRMED));
}

#[test]
fn the_command_prints_nothing_being_confirmed_for_a_device_without_it() {
    let devices = [json!({ "port": "Box", "out_port": 0, "in_port": 0, "candidates": [] })];
    let text = format_detection(&devices, false);
    assert!(!text.contains("confirm"), "{text}");
}

#[test]
fn the_command_says_the_proofs_of_a_candidate_after_because_each_with_its_nature() {
    let devices = [json!({ "port": "Box", "out_port": 0, "in_port": 0, "candidates": [{
        "device": "acme/box", "file": "acme/box.json", "source_tier": "official", "score": 150,
        "proofs": [{ "nature": "strong", "kind": "identity reply" }, { "nature": "weak", "kind": "name" }] }] })];
    let text = format_detection(&devices, false);
    assert!(text.contains("because: identity reply (strong), name (weak)"), "{text}");
}

#[test]
fn the_command_prints_a_candidate_of_a_name_alone_as_possible_to_confirm_with_no_run_command() {
    let devices = [json!({ "port": "Osmose", "out_port": 0, "in_port": 0, "run_command": null, "candidates": [{
        "device": "acme/osmose", "file": "acme/osmose.json", "source_tier": "official", "score": 38,
        "state": "possible", "state_label": "possible, to confirm",
        "proofs": [{ "nature": "weak", "kind": "name" }] }] })];
    let text = format_detection(&devices, false);
    assert!(text.contains("possible, to confirm") && text.contains("because: name (weak)"), "{text}");
    assert!(!text.contains("run:"), "{text}");
}

#[test]
fn the_command_prints_a_port_with_no_candidate_and_nothing_named_as_unknown_and_points_to_the_learn_mode() {
    // No word "confirm" in it: nothing is being confirmed for such a port.
    let devices = [json!({ "port": "Zork", "out_port": 0, "in_port": 0, "candidates": [] })];
    let text = format_detection(&devices, false);
    assert!(text.contains("unknown") && text.contains("--learn"), "{text}");
}

#[test]
fn a_name_alone_never_proposes_a_run_command_in_the_detection_of_a_port() {
    let r = detection_report(&fixtures(), None, true, true, vec![probed("minilogue MIDI 1", None)], Vec::new(), &[]).unwrap();
    let d = &r["structuredContent"]["devices"][0];
    assert_eq!(d["candidates"][0]["device"], "korg/minilogue", "{r}");
    assert_eq!(d["candidates"][0]["state_label"], "possible, to confirm", "{r}");
    assert!(d["run_command"].is_null(), "{r}");
}

#[test]
fn an_identity_reply_the_card_recognises_proposes_the_run_command_in_the_detection_of_a_port() {
    let acme = crate::detect::Identity { manufacturer: vec![0, 33, 9], family: 1, model: 1, version: [2, 0, 0, 0] };
    let mut port = probed("Box", None);
    port.replies = vec![acme.reply()];
    let r = detection_report(&fixtures(), None, true, true, vec![port], Vec::new(), &[]).unwrap();
    let d = &r["structuredContent"]["devices"][0];
    let file = d["candidates"][0]["file"].as_str().unwrap();
    assert!(file.contains("box.vendor.fw-2"), "{r}");
    assert!(d["run_command"].as_str().unwrap().contains(&format!("--device {file} ")), "{r}");
    assert!(format_detection(std::slice::from_ref(d), true).contains("because: identity reply (strong)"), "{r}");
}

#[test]
fn a_device_the_service_names_as_being_confirmed_still_proposes_no_run_command() {
    let (client, _bodies, _dir) = detect_service(bcr_unlisted);
    let devices = report_devices(&client, vec![probed("BCR32", None)]);
    assert!(devices[0]["run_command"].is_null(), "{devices:?}");
}
