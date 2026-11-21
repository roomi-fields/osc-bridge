//! An agent sets and reads a sound by the role of a control (`filter/cutoff`), on whatever
//! driver is loaded, through three MCP tools: `list_roles`, `set_by_role` and `get_by_role`.
//! A role is the OSD 0.6 path of a control, its parts then its setting; the value an agent
//! gives is from 0 to 1 and osc-bridge scales it on the range and steps `osd` gives. The
//! drivers are in-memory 0.6 and 0.5 drivers of two makers; the bridge is a UDP stand-in
//! that records what it receives, acknowledges it as the real one does and answers a read.

use crate::common::{files, temp};
use rosc::{decoder, encoder, OscBundle, OscMessage, OscPacket, OscType};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const D06: &str = "https://w3id.org/osd/0.6/device.schema.json";
const D05: &str = crate::common::D05;

// ---- the stand-in bridge ----------------------------------------------------

/// What the stand-in bridge received: each message's address and arguments, in order.
type Seen = Arc<Mutex<Vec<(String, Vec<OscType>)>>>;

/// A bridge on a free local port: records every message but its own `/bridge/*` ones, answers
/// `/bridge/done <id>` to a `/bridge/sync <id>`, and, for an address it was told to answer,
/// then sends back one message (a device's reply) to whoever asked.
struct FakeBridge {
    addr: SocketAddr,
    seen: Seen,
    stop: Arc<AtomicBool>,
}

impl FakeBridge {
    /// `answers`: request address -> (address of the reply, its raw integer value).
    fn start(answers: &[(&str, &str, i32)]) -> Self {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let addr = sock.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let answers: HashMap<String, (String, i32)> =
            answers.iter().map(|(q, a, v)| (q.to_string(), (a.to_string(), *v))).collect();
        let (seen2, stop2) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while !stop2.load(Ordering::Relaxed) {
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                let Ok((_, pkt)) = decoder::decode_udp(&buf[..n]) else { continue };
                let mut msgs = Vec::new();
                flatten(pkt, &mut msgs);
                let mut sync = None;
                let mut reply = None;
                for m in msgs {
                    if m.addr == "/bridge/sync" {
                        sync = m.args.first().cloned();
                    } else if !m.addr.starts_with("/bridge/") {
                        if let Some(a) = answers.get(&m.addr) { reply = Some(a.clone()); }
                        seen2.lock().unwrap().push((m.addr, m.args));
                    }
                }
                let send = |m: OscMessage| {
                    let bytes = encoder::encode(&OscPacket::Message(m)).unwrap();
                    let _ = sock.send_to(&bytes, from);
                };
                if let Some(id) = sync {
                    send(OscMessage { addr: "/bridge/done".into(), args: vec![id] });
                }
                if let Some((address, raw)) = reply {
                    std::thread::sleep(Duration::from_millis(30));
                    send(OscMessage { addr: address, args: vec![OscType::Int(raw)] });
                }
            }
        });
        FakeBridge { addr, seen, stop }
    }

    fn received(&self) -> Vec<(String, Vec<OscType>)> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for FakeBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn flatten(pkt: OscPacket, out: &mut Vec<OscMessage>) {
    match pkt {
        OscPacket::Message(m) => out.push(m),
        OscPacket::Bundle(OscBundle { content, .. }) => content.into_iter().for_each(|p| flatten(p, out)),
    }
}

// ---- the drivers --------------------------------------------------------------

fn cc(osc: &str, cc: u8, priority: u8, role: &[&str], setting: &str, extra: &str) -> String {
    let parts: Vec<Value> = role.iter().map(|p| match p.split_once('#') {
        Some((part, i)) => json!({"part": part, "index": i.parse::<i64>().unwrap()}),
        None => json!({"part": p}),
    }).collect();
    format!(r#"{{"osc":"{osc}","cc":{cc},"priority":{priority},"role":{},{extra}"_":"x"}}"#,
        json!({"parts": parts, "setting": setting}))
}

fn driver06(name: &str, vendor: &str, prefix: &str, cc_entries: &[String], rest: &str) -> String {
    format!(r#"{{"$schema":"{D06}","role_vocabulary":"1",
        "device":{{"name":"{name}","vendor":"{vendor}","osc_prefix":"{prefix}"}},
        "cc_params":{{"channel":0,"entries":[{}]}}{rest}}}"#, cc_entries.join(","))
}

/// Acme: seven-bit controls. The same role as Zeta's cutoff, in 0 to 127.
fn acme() -> String {
    driver06("Synth", "Acme", "/acme", &[
        cc("/filter/cutoff", 74, 1, &["filter"], "cutoff", r#""range":[0,127],"default":100,"#),
        cc("/filter/resonance", 71, 2, &["filter"], "resonance", r#""range":[0,127],"default":10,"#),
        cc("/fenv/attack", 20, 3, &["filter", "envelope"], "attack-time", r#""range":[0,127],"default":0,"#),
        cc("/aenv/attack", 16, 4, &["amplifier", "envelope"], "attack-time", r#""range":[0,127],"default":5,"#),
        cc("/vco2/pitch", 36, 5, &["oscillator#2"], "pitch", r#""range":[0,127],"#),
    ], r#","sysex":{"header":[240,67],"footer":[247]},
        "params":{"value_type":"u7","set_frame":[{"arg":"p"},{"arg":"val"}],"get_frame":[{"arg":"p"}],
        "entries":[{"osc":"/out/pan","pr":0,"p":9,"c":0,"r":0,"range":[0,127],"priority":2,
            "role":{"parts":[{"part":"output"}],"setting":"pan"}}]},
        "replies":[{"match_frame":[240,67,9,{"capture":"v"},247],"emit_osc":"/acme/out/pan","emit_args":[{"capture":"v"}]}]"#)
}

/// Zeta: another maker, the cutoff on a 14-bit control, the resonance on five steps.
fn zeta() -> String {
    driver06("Synth", "Zeta", "/zeta", &[
        cc("/flt/freq", 20, 1, &["filter"], "cutoff", r#""cc_lsb":52,"range":[0,16383],"default":8000,"#),
        cc("/flt/res", 21, 2, &["filter"], "resonance", r#""range":[0,100],"step_count":5,"default":50,"#),
    ], "")
}

/// A 0.5 driver: controls without role, default or priority.
fn old05() -> String {
    format!(r#"{{"$schema":"{D05}","device":{{"name":"Old","vendor":"Acme","osc_prefix":"/old"}},
        "cc_params":{{"channel":0,"entries":[{{"osc":"/filter/cutoff","cc":74,"range":[0,127]}}]}}}}"#)
}

fn catalogue() -> tempfile::TempDir {
    let root = temp("roles");
    files(root.path(), &[("acme/synth.json", &acme()), ("zeta/synth.json", &zeta()), ("acme/old.json", &old05())]);
    root
}

// ---- the session ----------------------------------------------------------------

/// The result of one tool call, over one offline session on the catalogue, the bridge at `target`.
fn call(root: &tempfile::TempDir, target: SocketAddr, tool: &str, args: Value) -> Value {
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: Some(root.path().to_path_buf()),
        default_target: target,
        online: false,
        private: vec![],
    };
    let input = format!("{}\n", crate::common::tool_call(1, tool, args));
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, "http://127.0.0.1:1", None, input.as_bytes(), &mut out).unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().next().unwrap()).unwrap();
    reply["result"].clone()
}

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

fn text(r: &Value) -> String {
    r["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")
}

fn roles(r: &Value) -> Vec<String> {
    r["structuredContent"]["roles"].as_array().unwrap_or_else(|| panic!("no roles in {r}"))
        .iter().map(|e| e["role"].as_str().unwrap().to_string()).collect()
}

fn one_int(received: &[(String, Vec<OscType>)], addr: &str) -> i32 {
    let hits: Vec<_> = received.iter().filter(|(a, _)| a == addr).collect();
    assert_eq!(hits.len(), 1, "{addr} in {received:?}");
    match hits[0].1.as_slice() {
        [OscType::Int(i)] => *i,
        [OscType::Float(f)] => f.round() as i32,
        other => panic!("{other:?}"),
    }
}

// ---- reading the roles through osd ---------------------------------------------

#[test]
fn a_driver_loads_with_each_controls_role_default_range_steps_and_priority_as_osd_gives_them() {
    let root = catalogue();
    let dev = osc_bridge::device::load(root.path().join("zeta/synth.json")).unwrap();
    let res = &dev.cc_params.as_ref().unwrap().entries[1];
    assert_eq!(res.default, Some(50));
    assert_eq!(res.range, Some([0, 100]));
    assert_eq!(res.step_count, Some(5));
    assert_eq!(res.priority, Some(2));
    assert_eq!(res.role.as_ref().unwrap().setting, "resonance");
}

#[test]
fn a_driver_of_the_earlier_version_loads_and_is_served_as_before() {
    let root = catalogue();
    let dev = osc_bridge::device::load(root.path().join("acme/old.json")).unwrap();
    let e = &dev.cc_params.as_ref().unwrap().entries[0];
    assert!(e.role.is_none() && e.default.is_none() && e.priority.is_none() && e.step_count.is_none());
    let bridge = FakeBridge::start(&[]);
    let routes = call(&root, bridge.addr, "list_routes", json!({"device": "old"}));
    assert!(!is_error(&routes), "{routes}");
    assert_eq!(routes["structuredContent"]["cc_params"][0]["address"], "/old/filter/cutoff");
}

// ---- list_roles --------------------------------------------------------------------

#[test]
fn list_roles_gives_the_roles_most_important_first_with_address_range_default_and_steps() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    let r = call(&root, bridge.addr, "list_roles", json!({"device": "acme/synth"}));
    assert!(!is_error(&r), "{r}");
    assert_eq!(roles(&r), ["filter/cutoff", "filter/resonance", "output/pan", "filter/envelope/attack-time",
        "amplifier/envelope/attack-time", "oscillator/2/pitch"]);
    let first = &r["structuredContent"]["roles"][0];
    assert_eq!(first["address"], "/acme/filter/cutoff");
    assert_eq!(first["range"], json!([0, 127]));
    assert_eq!(first["default"], 100);
    let z = call(&root, bridge.addr, "list_roles", json!({"device": "zeta/synth"}));
    assert_eq!(z["structuredContent"]["roles"][0]["range"], json!([0, 16383]));
    assert_eq!(z["structuredContent"]["roles"][1]["step_count"], 5);
    assert_eq!(z["structuredContent"]["roles"][1]["address"], "/zeta/flt/res");
}

#[test]
fn list_roles_keeps_only_the_first_roles_up_to_the_limit() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    let r = call(&root, bridge.addr, "list_roles", json!({"device": "acme/synth", "limit": 2}));
    assert_eq!(roles(&r), ["filter/cutoff", "filter/resonance"]);
}

#[test]
fn list_roles_of_a_driver_without_roles_is_an_empty_list() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    let r = call(&root, bridge.addr, "list_roles", json!({"device": "acme/old"}));
    assert!(!is_error(&r), "{r}");
    assert!(roles(&r).is_empty());
}

// ---- set_by_role ---------------------------------------------------------------------

#[test]
fn set_by_role_sends_the_controls_message_with_the_value_scaled_on_its_range() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    let r = call(&root, bridge.addr, "set_by_role", json!({"device": "acme/synth", "role": "filter/cutoff", "value": 0.5}));
    assert!(!is_error(&r), "{r}");
    assert_eq!(r["structuredContent"]["sent"], true);
    assert_eq!(one_int(&bridge.received(), "/acme/filter/cutoff"), 64);
    let r = call(&root, bridge.addr, "set_by_role", json!({"device": "acme/synth", "role": "filter/cutoff", "value": 1}));
    assert!(!is_error(&r), "{r}");
    assert!(bridge.received().iter().any(|(a, v)| a == "/acme/filter/cutoff" && v == &[OscType::Int(127)]));
    call(&root, bridge.addr, "set_by_role", json!({"device": "acme/synth", "role": "filter/cutoff", "value": 0}));
    assert!(bridge.received().iter().any(|(a, v)| a == "/acme/filter/cutoff" && v == &[OscType::Int(0)]));
}

#[test]
fn set_by_role_finds_a_role_of_several_parts_and_a_numbered_part() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    for (role, addr, raw) in [("filter/envelope/attack-time", "/acme/fenv/attack", 127),
                              ("amplifier/envelope/attack-time", "/acme/aenv/attack", 127),
                              ("oscillator/2/pitch", "/acme/vco2/pitch", 127)] {
        let r = call(&root, bridge.addr, "set_by_role", json!({"device": "acme/synth", "role": role, "value": 1.0}));
        assert!(!is_error(&r), "{role}: {r}");
        assert_eq!(one_int(&bridge.received(), addr), raw, "{role}");
    }
}

#[test]
fn the_same_set_by_role_request_reaches_two_makers_each_at_its_own_address_and_raw_value() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    for device in ["acme/synth", "zeta/synth"] {
        let r = call(&root, bridge.addr, "set_by_role", json!({"device": device, "role": "filter/cutoff", "value": 0.5}));
        assert!(!is_error(&r), "{device}: {r}");
    }
    let got = bridge.received();
    assert_eq!(one_int(&got, "/acme/filter/cutoff"), 64);
    assert_eq!(one_int(&got, "/zeta/flt/freq"), 8192, "the middle of 0 to 16383, in Zeta's own range");
    assert!(got.iter().all(|(a, _)| a != "/zeta/filter/cutoff" && a != "/acme/flt/freq"), "{got:?}");
}

#[test]
fn set_by_role_snaps_the_value_to_the_steps_of_the_control() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    // Five steps on 0 to 100: 0, 25, 50, 75, 100.
    for (value, raw) in [(0.6, 50), (0.9, 100), (0.1, 0)] {
        let r = call(&root, bridge.addr, "set_by_role", json!({"device": "zeta/synth", "role": "filter/resonance", "value": value}));
        assert!(!is_error(&r), "{r}");
        assert!(bridge.received().iter().any(|(a, v)| a == "/zeta/flt/res" && v == &[OscType::Int(raw)]),
            "{value} -> {raw}: {:?}", bridge.received());
        bridge.seen.lock().unwrap().clear();
    }
}

#[test]
fn set_by_role_refuses_a_value_outside_zero_to_one_naming_the_interval_and_sends_nothing() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    for value in [json!(1.5), json!(-0.1)] {
        let r = call(&root, bridge.addr, "set_by_role", json!({"device": "acme/synth", "role": "filter/cutoff", "value": value}));
        assert!(is_error(&r), "{value}: {r}");
        assert!(text(&r).contains("0 to 1") || text(&r).contains("0 and 1") || text(&r).contains("[0, 1]"), "{}", text(&r));
    }
    assert!(bridge.received().is_empty(), "{:?}", bridge.received());
}

#[test]
fn set_by_role_of_an_unknown_role_names_it_and_gives_the_nearest_roles_and_sends_nothing() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    let r = call(&root, bridge.addr, "set_by_role", json!({"device": "acme/synth", "role": "filter/drive", "value": 0.5}));
    assert!(is_error(&r), "{r}");
    assert!(text(&r).contains("filter/drive"), "{}", text(&r));
    let nearest: Vec<String> = r["structuredContent"]["nearest"].as_array().unwrap_or_else(|| panic!("no nearest in {r}"))
        .iter().map(|v| v.as_str().unwrap().to_string()).collect();
    // The roles sharing the path `filter` first, by priority, then the others.
    assert_eq!(nearest[..3], ["filter/cutoff", "filter/resonance", "filter/envelope/attack-time"]);
    assert!(nearest.contains(&"amplifier/envelope/attack-time".to_string()));
    assert!(bridge.received().is_empty());
}

#[test]
fn a_role_two_controls_of_one_driver_carry_is_refused_naming_both_addresses_by_both_tools() {
    let twice = driver06("Twice", "Acme", "/twice", &[
        cc("/a/cut", 74, 1, &["filter"], "cutoff", r#""range":[0,127],"#),
        cc("/b/cut", 75, 2, &["filter"], "cutoff", r#""range":[0,127],"#),
    ], "");
    let root = temp("roles-twice");
    files(root.path(), &[("acme/twice.json", &twice)]);
    let bridge = FakeBridge::start(&[]);
    let s = call(&root, bridge.addr, "set_by_role", json!({"device": "acme/twice", "role": "filter/cutoff", "value": 0.5}));
    let g = call(&root, bridge.addr, "get_by_role", json!({"device": "acme/twice", "role": "filter/cutoff"}));
    for r in [s, g] {
        assert!(is_error(&r), "{r}");
        assert!(text(&r).contains("/twice/a/cut") && text(&r).contains("/twice/b/cut"), "{}", text(&r));
    }
    assert!(bridge.received().is_empty());
}

#[test]
fn set_by_role_refuses_a_control_whose_message_writes_the_devices_memory_naming_the_tool_that_confirms() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    let r = call(&root, bridge.addr, "set_by_role", json!({"device": "acme/synth", "role": "output/pan", "value": 0.5}));
    assert!(is_error(&r), "{r}");
    assert!(text(&r).contains("restore_presets") && text(&r).contains("confirm_overwrite"), "{}", text(&r));
    assert!(bridge.received().is_empty());
}

#[test]
fn set_by_role_carries_no_consent_argument_and_the_tools_are_published_with_their_arguments() {
    let tools = osc_bridge::mcp::tool_schemas();
    let find = |name: &str| tools["tools"].as_array().unwrap().iter().find(|t| t["name"] == name).cloned()
        .unwrap_or_else(|| panic!("no tool {name}"));
    let props = |t: &Value| t["inputSchema"]["properties"].as_object().unwrap().keys().cloned().collect::<std::collections::BTreeSet<_>>();
    let set = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(props(&find("list_roles")), set(&["device", "limit"]));
    assert_eq!(props(&find("set_by_role")), set(&["device", "role", "value", "target"]));
    assert_eq!(props(&find("get_by_role")), set(&["device", "role", "target"]));
    assert_eq!(find("set_by_role")["inputSchema"]["required"], json!(["device", "role", "value"]));
}

// ---- get_by_role ---------------------------------------------------------------------------

#[test]
fn get_by_role_sends_the_drivers_read_request_and_gives_the_reply_value_scaled_from_zero_to_one() {
    let root = catalogue();
    // The driver's read is `<prefix>/param/get` with the parameter's own numbers; its reply
    // pattern reports at the control's address.
    let bridge = FakeBridge::start(&[("/acme/param/get", "/acme/out/pan", 64)]);
    let r = call(&root, bridge.addr, "get_by_role", json!({"device": "acme/synth", "role": "output/pan"}));
    assert!(!is_error(&r), "{r}");
    let v = r["structuredContent"]["value"].as_f64().unwrap();
    assert!((v - 64.0 / 127.0).abs() < 1e-6, "{v}");
    let sent = bridge.received();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "/acme/param/get");
    assert_eq!(sent[0].1, [OscType::Int(0), OscType::Int(9), OscType::Int(0), OscType::Int(0)]);
}

#[test]
fn get_by_role_of_a_control_the_driver_declares_no_read_for_says_it_does_not_report_and_invents_nothing() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    let r = call(&root, bridge.addr, "get_by_role", json!({"device": "acme/synth", "role": "filter/cutoff"}));
    assert!(is_error(&r), "{r}");
    assert!(text(&r).contains("filter/cutoff") && text(&r).contains("report"), "{}", text(&r));
    assert!(r["structuredContent"]["value"].is_null());
    assert!(bridge.received().is_empty(), "{:?}", bridge.received());
}

#[test]
fn get_by_role_of_an_unknown_role_names_it_with_the_nearest_roles() {
    let root = catalogue();
    let bridge = FakeBridge::start(&[]);
    let r = call(&root, bridge.addr, "get_by_role", json!({"device": "acme/synth", "role": "filter/drive"}));
    assert!(is_error(&r), "{r}");
    assert!(text(&r).contains("filter/drive"));
    assert_eq!(r["structuredContent"]["nearest"][0], "filter/cutoff");
    assert!(bridge.received().is_empty());
}
