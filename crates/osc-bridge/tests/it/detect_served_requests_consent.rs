//! The requests the service serves (`GET /v1/detect`) go through the same consent plan as the
//! local detection: a maker request waits for the person's consent for its port, the universal
//! identity request always goes and is never held back. A local fake service, no real MIDI port.

use crate::common::temp;
use osc_bridge::capture::{Capture, FORMAT_VERSION};
use osc_bridge::mcp::hex_bytes;
use osc_bridge::recognition::{send_plan, Probe};
use osc_bridge::synthub::online::Online;
use serde_json::{json, Value};

const MYSTERY: &str = "Mystery Box:Mystery Box 20:0";
const MINILAB: &str = "Minilab3:Minilab3 MIDI 40:0";
const UNIVERSAL: [u8; 6] = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];
const OTHER_DEVICE: [u8; 6] = [0xF0, 0x7E, 0x00, 0x06, 0x01, 0xF7];
const MAKER: [u8; 4] = [0xF0, 0x42, 0x01, 0xF7];

/// The probes a fake service lists, read back by the client.
fn served(requests: &[&[u8]]) -> Vec<Probe> {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let probes: Vec<Value> = requests.iter()
        .map(|r| json!({ "request_hex": hex_bytes(r), "usb": [{ "vendor_id": 0x1c75, "product_id": 0x020b }] }))
        .collect();
    let payload = json!({ "probes": probes }).to_string();
    let handle = std::thread::spawn(move || {
        let req = server.recv().unwrap();
        let _ = req.respond(tiny_http::Response::from_string(payload));
    });
    let dir = temp("served-consent");
    let account = dir.path().join("config/user");
    std::fs::create_dir_all(account.parent().unwrap()).unwrap();
    std::fs::write(&account, "0123456789abcdef0123456789abcdef\n").unwrap();
    let online = Online::keeping_account_in(&base, Some(account));
    let probes = online.probes().unwrap();
    handle.join().unwrap();
    probes
}

fn capture() -> Capture {
    Capture::parse(&json!({
        "format_version": FORMAT_VERSION,
        "outputs": [MINILAB, MYSTERY],
        "inputs": [MINILAB, MYSTERY],
        "cards": " 6 [Minilab3       ]: USB-Audio - Minilab3\n                      Arturia Minilab3 at usb-0000:00:14.0-2, full speed\n",
        "usbids": [{"card": 6, "text": "1c75:020b\n"}], "clients": "",
        "requests": [], "received": [],
    }).to_string()).unwrap()
}

fn report(probes: &[Probe], consent: &[&str]) -> Vec<Value> {
    let dir = temp("served-consent");
    let consent: Vec<String> = consent.iter().map(|s| s.to_string()).collect();
    osc_bridge::mcp::detect_capture_held(dir.path(), &capture(), probes, &consent, osc_bridge::synthub::catalogue::tier_rank).unwrap()
}

fn held_of(devices: &[Value], port: &str) -> Vec<String> {
    let d = devices.iter().find(|d| d["port"] == port).unwrap_or_else(|| panic!("{port} in {devices:?}"));
    d["held_back"].as_array().unwrap_or_else(|| panic!("held_back of {port}: {d}")).iter()
        .map(|h| h.as_str().unwrap().to_string()).collect()
}

fn ports() -> Vec<(String, Option<(u16, u16)>)> {
    vec![("Minilab3".to_string(), Some((0x1c75, 0x020b))), ("Mystery".to_string(), None)]
}

#[test]
fn a_maker_request_served_by_the_service_is_held_back_without_consent_and_named() {
    let probes = served(&[&MAKER]);
    let devices = report(&probes, &[]);
    assert_eq!(held_of(&devices, MYSTERY), vec![hex_bytes(&MAKER)]);
    assert_eq!(held_of(&devices, MINILAB), vec![hex_bytes(&MAKER)], "its usb id sends nothing either");
    for sends in send_plan(&ports(), &probes, &[]) {
        assert_eq!(sends.sent, vec![UNIVERSAL.to_vec()], "{}", sends.port);
        assert_eq!(sends.held, vec![MAKER.to_vec()]);
    }
}

#[test]
fn a_maker_request_served_by_the_service_goes_only_to_the_consented_port() {
    let probes = served(&[&MAKER]);
    let devices = report(&probes, &["mystery"]);
    assert!(held_of(&devices, MYSTERY).is_empty());
    assert_eq!(held_of(&devices, MINILAB), vec![hex_bytes(&MAKER)]);
    let plan = send_plan(&ports(), &probes, &[1]);
    assert_eq!(plan[1].sent, vec![UNIVERSAL.to_vec(), MAKER.to_vec()]);
    assert_eq!(plan[0].sent, vec![UNIVERSAL.to_vec()]);
}

#[test]
fn the_universal_identity_request_served_once_or_twice_is_sent_once_and_never_held_back() {
    for served_requests in [vec![&UNIVERSAL[..], &MAKER[..]], vec![&UNIVERSAL[..], &MAKER[..], &UNIVERSAL[..]]] {
        let probes = served(&served_requests);
        for consent in [&[][..], &["mystery"][..]] {
            let devices = report(&probes, consent);
            for port in [MINILAB, MYSTERY] {
                assert!(!held_of(&devices, port).contains(&hex_bytes(&UNIVERSAL)), "{port} held the universal request");
            }
            let indexes: Vec<usize> = if consent.is_empty() { vec![] } else { vec![1] };
            for sends in send_plan(&ports(), &probes, &indexes) {
                assert_eq!(sends.sent.iter().filter(|s| **s == UNIVERSAL).count(), 1, "{}", sends.port);
                assert!(!sends.held.contains(&UNIVERSAL.to_vec()), "{}", sends.port);
            }
        }
    }
}

#[test]
fn the_identity_request_of_another_device_id_is_a_maker_request_held_back_without_consent() {
    let probes = served(&[&UNIVERSAL, &OTHER_DEVICE]);
    let devices = report(&probes, &[]);
    for port in [MINILAB, MYSTERY] {
        assert!(held_of(&devices, port).contains(&hex_bytes(&OTHER_DEVICE)), "{port}");
        assert!(!held_of(&devices, port).contains(&hex_bytes(&UNIVERSAL)), "{port}");
    }
    let plan = send_plan(&ports(), &probes, &[]);
    assert!(plan.iter().all(|p| p.held == vec![OTHER_DEVICE.to_vec()] && p.sent == vec![UNIVERSAL.to_vec()]));
    let consented = send_plan(&ports(), &probes, &[0, 1]);
    assert!(consented.iter().all(|p| p.sent == vec![UNIVERSAL.to_vec(), OTHER_DEVICE.to_vec()]));
}
