//! Links made while the orchestrator runs, through its control addresses
//! `/bridge/link/add`, `/bridge/link/remove` and `/bridge/link/list`.
//!
//! The wire contract these tests hold:
//!   - `/bridge/link/add <from> <to> [in_lo in_hi out_lo out_hi]` answers
//!     `/bridge/link/added <from> <to>`, or `/bridge/link/error <text>`.
//!   - `/bridge/link/remove <from> <to>` answers `/bridge/link/removed <from> <to>`,
//!     or `/bridge/link/error <text>`.
//!   - `/bridge/link/list` answers one `/bridge/link/list/item <from> <to> <origin>
//!     [in_lo in_hi out_lo out_hi]` per link (origin `file` or `live`), then
//!     `/bridge/link/list/end <count>`.
//!
//! Topology: a `/ctl` device and a `/synth` device (both OSC transport, each
//! with its own fake target). The file holds one link, `/ctl/knob -> /synth/vol`,
//! and a sentinel link `/ctl/sentinel -> /synth/sentinel`: the sentinel's
//! forward reaching the synth's target proves every message sent before it has
//! been handled, so an absence is judged without guessing a delay.

use osc_bridge::orchestrator::Orchestrator;
use rosc::{OscMessage, OscPacket, OscType};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const DEADLINE: Duration = Duration::from_secs(10);

struct Rig {
    orch: SocketAddr,
    synth_target: UdpSocket,
    _dir: tempfile::TempDir,
}

fn free_udp() -> u16 {
    UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn free_tcp() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn device_json(prefix: &str, port: u16, commands: &[&str]) -> String {
    let cmds: Vec<String> = commands.iter().map(|c| format!(
        r#"{{ "osc": "/{c}", "args": [{{ "name": "v", "type": "float" }}],
             "forward": {{ "path": "/fx/{c}", "args": ["{{v}}"] }} }}"#)).collect();
    format!(r#"{{
        "device": {{ "name": "Test {prefix}", "vendor": "Test", "osc_prefix": "{prefix}", "kind": "software",
            "transport": {{ "kind": "osc", "host": "127.0.0.1", "port": {port} }} }},
        "commands": [{}]
    }}"#, cmds.join(","))
}

/// Start an orchestrator on the two-device topology; `extra` is more TOML
/// (further `[[routes]]` or `[osc]` keys are not accepted there: routes only).
fn start(extra_routes: &str, ws_port: Option<u16>) -> Rig {
    let synth_target = UdpSocket::bind("127.0.0.1:0").unwrap();
    let synth_port = synth_target.local_addr().unwrap().port();
    let ctl_target = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ctl_port = ctl_target.local_addr().unwrap().port();
    let orch_port = free_udp();
    let dir = crate::common::temp("live-links");
    let synth = dir.path().join("synth.json");
    let ctl = dir.path().join("ctl.json");
    std::fs::write(&synth, device_json("/synth", synth_port,
        &["vol", "sentinel", "cutoff", "a", "b", "c", "d", "e", "f"])).unwrap();
    std::fs::write(&ctl, device_json("/ctl", ctl_port, &["knob", "sentinel", "wheel"])).unwrap();
    let ws_line = ws_port.map(|p| format!("ws_bind = \"127.0.0.1:{p}\"\n")).unwrap_or_default();
    let toml = dir.path().join("bridge.toml");
    std::fs::write(&toml, format!(r#"
[osc]
bind = "127.0.0.1:{orch_port}"
{ws_line}
[[devices]]
spec = "{}"

[[devices]]
spec = "{}"

[[routes]]
from = "/ctl/knob"
to = "/synth/vol"

[[routes]]
from = "/ctl/sentinel"
to = "/synth/sentinel"
{extra_routes}
"#, ctl.display().to_string().replace('\\', "\\\\"), synth.display().to_string().replace('\\', "\\\\"))).unwrap();
    let t = toml.clone();
    std::thread::spawn(move || { let _ = Orchestrator::run(&t); });
    // Keep the ctl target alive for the test's length without reading it.
    std::mem::forget(ctl_target);
    let rig = Rig { orch: format!("127.0.0.1:{orch_port}").parse().unwrap(), synth_target, _dir: dir };
    wait_ready(&rig);
    rig
}

fn wait_ready(rig: &Rig) {
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    client.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let bytes = encode("/bridge/status", vec![]);
    let end = Instant::now() + DEADLINE;
    let mut buf = [0u8; 65536];
    while Instant::now() < end {
        client.send_to(&bytes, rig.orch).unwrap();
        if client.recv_from(&mut buf).is_ok() { return; }
    }
    panic!("orchestrator never answered /bridge/status");
}

fn encode(addr: &str, args: Vec<OscType>) -> Vec<u8> {
    rosc::encoder::encode(&OscPacket::Message(OscMessage { addr: addr.into(), args })).unwrap()
}

fn decode(bytes: &[u8]) -> Vec<OscMessage> {
    fn flat(p: OscPacket, out: &mut Vec<OscMessage>) {
        match p {
            OscPacket::Message(m) => out.push(m),
            OscPacket::Bundle(b) => for q in b.content { flat(q, out); },
        }
    }
    let mut out = Vec::new();
    flat(rosc::decoder::decode_udp(bytes).unwrap().1, &mut out);
    out
}

fn s(x: &str) -> OscType { OscType::String(x.into()) }
fn f(x: f32) -> OscType { OscType::Float(x) }

/// Ask the control address; read the replies until one of `last` arrives.
fn ask(rig: &Rig, addr: &str, args: Vec<OscType>, last: &[&str]) -> Vec<OscMessage> {
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    client.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    client.send_to(&encode(addr, args), rig.orch).unwrap();
    let end = Instant::now() + DEADLINE;
    let mut got = Vec::new();
    let mut buf = [0u8; 65536];
    while Instant::now() < end {
        if let Ok((n, _)) = client.recv_from(&mut buf) {
            for m in decode(&buf[..n]) {
                let done = last.contains(&m.addr.as_str());
                got.push(m);
                if done { return got; }
            }
        }
    }
    panic!("no final reply to {addr}: got {got:?}");
}

fn add(rig: &Rig, from: &str, to: &str, map: Option<[f32; 4]>) -> OscMessage {
    let mut args = vec![s(from), s(to)];
    if let Some(m) = map { args.extend(m.iter().map(|x| f(*x))); }
    ask(rig, "/bridge/link/add", args, &["/bridge/link/added", "/bridge/link/error"]).pop().unwrap()
}

fn remove(rig: &Rig, from: &str, to: &str) -> OscMessage {
    ask(rig, "/bridge/link/remove", vec![s(from), s(to)], &["/bridge/link/removed", "/bridge/link/error"]).pop().unwrap()
}

/// `(from, to, origin, scaling args)` of every listed link.
fn list(rig: &Rig) -> Vec<(String, String, String, Vec<OscType>)> {
    let replies = ask(rig, "/bridge/link/list", vec![], &["/bridge/link/list/end"]);
    let end = replies.last().unwrap();
    let items: Vec<_> = replies.iter().filter(|m| m.addr == "/bridge/link/list/item").map(|m| {
        let txt = |i: usize| match &m.args[i] { OscType::String(x) => x.clone(), o => panic!("not a string: {o:?}") };
        (txt(0), txt(1), txt(2), m.args[3..].to_vec())
    }).collect();
    assert_eq!(end.args, vec![OscType::Int(items.len() as i32)], "the end reply counts the items");
    items
}

fn text(m: &OscMessage) -> String {
    m.args.iter().filter_map(|a| if let OscType::String(x) = a { Some(x.clone()) } else { None })
        .collect::<Vec<_>>().join(" ")
}

fn send(rig: &Rig, addr: &str, args: Vec<OscType>) {
    UdpSocket::bind("127.0.0.1:0").unwrap().send_to(&encode(addr, args), rig.orch).unwrap();
}

/// The messages the synth's target receives after `addr args` is sent, up to
/// the sentinel's forward (which proves everything before it was handled).
fn forwards_after(rig: &Rig, addr: &str, args: Vec<OscType>) -> Vec<OscMessage> {
    // Drain anything older.
    rig.synth_target.set_read_timeout(Some(Duration::from_millis(1))).unwrap();
    let mut buf = [0u8; 65536];
    while rig.synth_target.recv_from(&mut buf).is_ok() {}
    rig.synth_target.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    send(rig, addr, args);
    send(rig, "/ctl/sentinel", vec![f(1.0)]);
    let end = Instant::now() + DEADLINE;
    let mut got = Vec::new();
    while Instant::now() < end {
        if let Ok((n, _)) = rig.synth_target.recv_from(&mut buf) {
            for m in decode(&buf[..n]) {
                let done = m.addr == "/fx/sentinel";
                got.push(m);
                if done { return got; }
            }
        }
    }
    panic!("the sentinel never arrived: {got:?}");
}

fn forward_of<'a>(got: &'a [OscMessage], addr: &str) -> Option<&'a OscMessage> {
    got.iter().find(|m| m.addr == addr)
}

fn float_at(m: &OscMessage) -> f32 {
    match m.args[0] { OscType::Float(x) => x, ref o => panic!("expected float, got {o:?}") }
}

#[test]
fn a_link_added_live_is_followed_by_the_next_message_with_its_scaling() {
    let rig = start("", None);
    let r = add(&rig, "/ctl/knob", "/synth/cutoff", Some([0.0, 127.0, 0.0, 1.0]));
    assert_eq!(r.addr, "/bridge/link/added", "{r:?}");
    let got = forwards_after(&rig, "/ctl/knob", vec![f(80.0)]);
    let cutoff = forward_of(&got, "/fx/cutoff").unwrap_or_else(|| panic!("no /fx/cutoff in {got:?}"));
    assert!((float_at(cutoff) - 0.6299).abs() < 0.01, "80 of 0..127 scaled to 0..1: {cutoff:?}");
    assert!(forward_of(&got, "/fx/vol").is_some(), "the file's link still follows: {got:?}");
}

#[test]
fn a_message_received_at_a_link_source_reaches_the_end_of_a_chain_of_two_links_once() {
    let rig = start("", None);
    // The file's link /ctl/knob -> /synth/vol, then a live link /synth/vol -> /synth/a.
    assert_eq!(add(&rig, "/synth/vol", "/synth/a", None).addr, "/bridge/link/added");
    let got = forwards_after(&rig, "/ctl/knob", vec![f(0.5)]);
    let reached = |addr: &str| got.iter().filter(|m| m.addr == addr).count();
    assert_eq!(reached("/fx/vol"), 1, "{got:?}");
    assert_eq!(reached("/fx/a"), 1, "the second link follows the first one's output: {got:?}");
}

#[test]
fn a_link_added_live_with_an_inverted_output_range_scales_inverted() {
    let rig = start("", None);
    assert_eq!(add(&rig, "/ctl/wheel", "/synth/cutoff", Some([0.0, 100.0, 1.0, 0.0])).addr, "/bridge/link/added");
    let got = forwards_after(&rig, "/ctl/wheel", vec![f(25.0)]);
    assert!((float_at(forward_of(&got, "/fx/cutoff").unwrap()) - 0.75).abs() < 0.01, "{got:?}");
}

#[test]
fn a_link_added_live_without_scaling_passes_the_argument_through() {
    let rig = start("", None);
    assert_eq!(add(&rig, "/ctl/wheel", "/synth/a", None).addr, "/bridge/link/added");
    let got = forwards_after(&rig, "/ctl/wheel", vec![f(0.25)]);
    assert!((float_at(forward_of(&got, "/fx/a").unwrap()) - 0.25).abs() < 1e-6, "{got:?}");
}

#[test]
fn a_link_removed_live_is_no_longer_followed() {
    let rig = start("", None);
    assert_eq!(add(&rig, "/ctl/wheel", "/synth/cutoff", None).addr, "/bridge/link/added");
    assert!(forward_of(&forwards_after(&rig, "/ctl/wheel", vec![f(0.5)]), "/fx/cutoff").is_some());
    let r = remove(&rig, "/ctl/wheel", "/synth/cutoff");
    assert_eq!(r.addr, "/bridge/link/removed", "{r:?}");
    let got = forwards_after(&rig, "/ctl/wheel", vec![f(0.5)]);
    assert!(forward_of(&got, "/fx/cutoff").is_none(), "the removed link must not follow: {got:?}");
}

#[test]
fn a_link_of_the_configuration_file_can_be_removed_too() {
    let rig = start("", None);
    assert_eq!(remove(&rig, "/ctl/knob", "/synth/vol").addr, "/bridge/link/removed");
    let got = forwards_after(&rig, "/ctl/knob", vec![f(0.5)]);
    assert!(forward_of(&got, "/fx/vol").is_none(), "{got:?}");
}

#[test]
fn the_list_gives_file_and_live_links_with_origin_source_target_and_scaling() {
    let rig = start("", None);
    assert_eq!(add(&rig, "/ctl/knob", "/synth/cutoff", Some([0.0, 127.0, 0.0, 1.0])).addr, "/bridge/link/added");
    let links = list(&rig);
    let file = links.iter().find(|l| l.1 == "/synth/vol").expect("the file's link is listed");
    assert_eq!((file.0.as_str(), file.2.as_str()), ("/ctl/knob", "file"));
    assert!(file.3.is_empty(), "no scaling: {:?}", file.3);
    let live = links.iter().find(|l| l.1 == "/synth/cutoff").expect("the live link is listed");
    assert_eq!((live.0.as_str(), live.2.as_str()), ("/ctl/knob", "live"));
    assert_eq!(live.3, vec![f(0.0), f(127.0), f(0.0), f(1.0)]);
}

#[test]
fn the_list_of_a_removed_link_no_longer_has_it() {
    let rig = start("", None);
    add(&rig, "/ctl/wheel", "/synth/cutoff", None);
    assert_eq!(remove(&rig, "/ctl/wheel", "/synth/cutoff").addr, "/bridge/link/removed");
    assert!(!list(&rig).iter().any(|l| l.0 == "/ctl/wheel"));
}

#[test]
fn a_file_link_with_an_address_no_driver_has_is_listed_as_it_is_without_refusal() {
    let rig = start("\n[[routes]]\nfrom = \"/nowhere/x\"\nto = \"/nowhere/y\"\n", None);
    let links = list(&rig);
    let l = links.iter().find(|l| l.0 == "/nowhere/x").expect("listed");
    assert_eq!((l.1.as_str(), l.2.as_str()), ("/nowhere/y", "file"));
}

#[test]
fn a_link_naming_an_unknown_device_is_refused_naming_it_and_nothing_changes() {
    let rig = start("", None);
    let before = list(&rig);
    let r = add(&rig, "/ghost/knob", "/synth/cutoff", None);
    assert_eq!(r.addr, "/bridge/link/error");
    assert!(text(&r).contains("/ghost/knob"), "the error names the address: {r:?}");
    let r = add(&rig, "/ctl/knob", "/ghost/cutoff", None);
    assert_eq!(r.addr, "/bridge/link/error");
    assert!(text(&r).contains("/ghost/cutoff"), "{r:?}");
    assert_eq!(list(&rig), before);
}

#[test]
fn a_link_to_an_address_its_driver_does_not_have_is_refused_naming_it_and_nothing_changes() {
    let rig = start("", None);
    let before = list(&rig);
    let r = add(&rig, "/ctl/knob", "/synth/resonance", None);
    assert_eq!(r.addr, "/bridge/link/error");
    assert!(text(&r).contains("/synth/resonance"), "{r:?}");
    let r = add(&rig, "/ctl/missing", "/synth/cutoff", None);
    assert_eq!(r.addr, "/bridge/link/error");
    assert!(text(&r).contains("/ctl/missing"), "{r:?}");
    assert_eq!(list(&rig), before);
    let got = forwards_after(&rig, "/ctl/knob", vec![f(0.5)]);
    assert!(forward_of(&got, "/fx/resonance").is_none());
}

#[test]
fn removing_a_link_that_does_not_exist_is_refused_naming_it_and_nothing_changes() {
    let rig = start("", None);
    let before = list(&rig);
    let r = remove(&rig, "/ctl/wheel", "/synth/cutoff");
    assert_eq!(r.addr, "/bridge/link/error");
    let t = text(&r);
    assert!(t.contains("/ctl/wheel") && t.contains("/synth/cutoff"), "{r:?}");
    assert_eq!(list(&rig), before);
}

#[test]
fn a_link_that_would_close_a_loop_is_refused_naming_the_chain() {
    let rig = start("", None);
    assert_eq!(add(&rig, "/synth/a", "/synth/b", None).addr, "/bridge/link/added");
    let before = list(&rig);
    let r = add(&rig, "/synth/b", "/synth/a", None);
    assert_eq!(r.addr, "/bridge/link/error");
    let t = text(&r);
    assert!(t.contains("/synth/a") && t.contains("/synth/b"), "the chain is named: {r:?}");
    assert_eq!(list(&rig), before);
    // A link onto itself is a loop of one hop.
    assert_eq!(add(&rig, "/synth/c", "/synth/c", None).addr, "/bridge/link/error");
}

#[test]
fn a_chain_of_four_links_is_accepted_and_a_fifth_hop_is_refused_naming_the_chain() {
    let rig = start("", None);
    for (from, to) in [("a", "b"), ("b", "c"), ("c", "d"), ("d", "e")] {
        let r = add(&rig, &format!("/synth/{from}"), &format!("/synth/{to}"), None);
        assert_eq!(r.addr, "/bridge/link/added", "four hops hold: {r:?}");
    }
    let before = list(&rig);
    let r = add(&rig, "/synth/e", "/synth/f", None);
    assert_eq!(r.addr, "/bridge/link/error", "a fifth hop is refused");
    let t = text(&r);
    assert!(t.contains("/synth/a") && t.contains("/synth/f"), "the chain is named: {r:?}");
    assert_eq!(list(&rig), before);
    // Joining two chains that together pass four hops is refused as well.
    let r = add(&rig, "/synth/f", "/synth/a", None);
    assert_eq!(r.addr, "/bridge/link/error", "{r:?}");
}

#[test]
fn the_control_addresses_answer_a_websocket_client() {
    let ws_port = free_tcp();
    let rig = start("", Some(ws_port));
    let mut socket: WebSocket<MaybeTlsStream<TcpStream>> = {
        let end = Instant::now() + DEADLINE;
        loop {
            match tungstenite::connect(format!("ws://127.0.0.1:{ws_port}")) {
                Ok((s, _)) => break s,
                Err(e) if Instant::now() > end => panic!("WS connect: {e}"),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    };
    if let MaybeTlsStream::Plain(t) = socket.get_ref() { t.set_read_timeout(Some(Duration::from_secs(2))).unwrap(); }
    // Let the orchestrator register the client before the first question.
    std::thread::sleep(Duration::from_millis(100));
    socket.send(Message::Binary(encode("/bridge/link/add", vec![s("/ctl/wheel"), s("/synth/cutoff")]).into())).unwrap();
    let end = Instant::now() + DEADLINE;
    let mut seen = Vec::new();
    while Instant::now() < end {
        match socket.read() {
            Ok(Message::Binary(b)) => {
                let msgs = decode(&b);
                let added = msgs.iter().any(|m| m.addr == "/bridge/link/added");
                seen.extend(msgs);
                if added { break; }
            }
            Ok(_) => continue,
            Err(e) => panic!("WS read: {e}; seen {seen:?}"),
        }
    }
    assert!(seen.iter().any(|m| m.addr == "/bridge/link/added"), "{seen:?}");
    assert!(list(&rig).iter().any(|l| l.0 == "/ctl/wheel" && l.2 == "live"), "the link added over WS is in the table");
}
