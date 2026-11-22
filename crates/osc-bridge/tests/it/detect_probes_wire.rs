//! The probes the service lists (`GET /v1/detect`) say their maker: what `probes_json` writes,
//! `Online::probes` reads back to the same probes.

use crate::common::temp;
use osc_bridge::detect::Probe;
use osc_bridge::mcp::probes_json;
use osc_bridge::online::Online;
use serde_json::{json, Value};

fn probe(tag: u8, ports: &[&str], usb: &[(u16, u16)], names: &[&str]) -> Probe {
    Probe {
        request: vec![0xF0, 0x00, tag, 0xF7],
        ports: ports.iter().map(|s| s.to_string()).collect(),
        usb: usb.to_vec(),
        match_ports: names.iter().map(|s| s.to_string()).collect(),
    }
}

/// A service answering `GET /v1/detect` with `body`; the probes a client reads from it.
fn read_back(body: &Value) -> Vec<Probe> {
    try_read_back(body).unwrap()
}

/// The same, with the client's refusal kept.
fn try_read_back(body: &Value) -> anyhow::Result<Vec<Probe>> {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let payload = body.to_string();
    let handle = std::thread::spawn(move || {
        let req = server.recv().unwrap();
        let _ = req.respond(tiny_http::Response::from_string(payload));
    });
    let dir = temp("probes-wire");
    // An account already kept: the client asks for no id first.
    let account = dir.path().join("config/user");
    std::fs::create_dir_all(account.parent().unwrap()).unwrap();
    std::fs::write(&account, "0123456789abcdef0123456789abcdef\n").unwrap();
    let online = Online::keeping_account_in(&base, Some(account));
    let probes = online.probes();
    handle.join().unwrap();
    probes
}

#[test]
fn a_request_of_the_service_that_is_not_one_short_sysex_message_is_refused_and_told() {
    for (hex, why) in [
        ("F0 7D 01", "no F7"),
        ("7D 01 F7", "no F0"),
        ("F0 7D 90 3C F7", "a status byte inside"),
        ("F0 7D F7 F0 7D F7", "two messages"),
        ("F0 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F 10 11 12 13 14 15 16 17 18 19 1A 1B 1C 1D 1E 1F 20 F7", "33 bytes"),
    ] {
        let body = json!({ "probes": [{ "request_hex": "F0 7D 01 F7" }, { "request_hex": hex }] });
        let err = try_read_back(&body).expect_err(why).to_string();
        assert!(err.contains(hex), "{why}: the refusal tells the bytes: {err}");
    }
}

#[test]
fn a_request_of_the_service_of_32_bytes_is_read_back() {
    let hex = format!("F0 {}F7", "01 ".repeat(30));
    let probes = read_back(&json!({ "probes": [{ "request_hex": hex }] }));
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].request.len(), 32);
}

#[test]
fn probes_json_writes_usb_ids_and_port_names_of_a_probe() {
    let p = probe(1, &["Arturia"], &[(0x1c75, 0x020b), (0x1c75, 0x0001)], &["Minilab3", "MiniLab"]);
    let v = probes_json(&[p]);
    let o = &v["probes"][0];
    assert_eq!(o["request_hex"], "F0 00 01 F7");
    assert_eq!(o["ports"], json!(["Arturia"]));
    assert_eq!(o["usb"], json!([{ "vendor_id": 0x1c75, "product_id": 0x020b }, { "vendor_id": 0x1c75, "product_id": 1 }]));
    assert_eq!(o["match_ports"], json!(["Minilab3", "MiniLab"]));
}

#[test]
fn probes_json_writes_each_key_for_itself() {
    let v = probes_json(&[probe(1, &[], &[(1, 2)], &[]), probe(2, &[], &[], &["Box"]), probe(3, &[], &[], &[])]);
    let ps = v["probes"].as_array().unwrap();
    assert!(ps[0].get("usb").is_some() && ps[0].get("match_ports").is_none(), "usb only: {}", ps[0]);
    assert!(ps[1].get("match_ports").is_some() && ps[1].get("usb").is_none(), "names only: {}", ps[1]);
    assert!(ps[2].get("usb").is_none() && ps[2].get("match_ports").is_none(), "neither: {}", ps[2]);
}

#[test]
fn online_probes_reads_back_what_probes_json_writes() {
    let sent = vec![
        probe(1, &["Arturia"], &[(0x1c75, 0x020b), (0x0944, 0x0115)], &["Minilab3"]),
        probe(2, &[], &[(0x1c75, 0x0001)], &[]),
        probe(3, &["Box"], &[], &["Box"]),
        probe(4, &[], &[], &[]),
    ];
    assert_eq!(read_back(&probes_json(&sent)), sent);
}
