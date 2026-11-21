//! The learn mode's report: what it says of a device by what was looked at (the service's
//! matches, the local catalogue), and the ports of the system it leaves out.

use osc_bridge::detect::{is_system_port, learn_units, Learned, Unit};
use osc_bridge::mcp::{learn_report, learn_service_matches, DeviceIndex, ServiceMatches};
use serde_json::{json, Value};
use std::time::Duration;

fn catalogue() -> DeviceIndex {
    DeviceIndex::build(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/devices"))
}

fn unit(port: &str) -> Unit {
    Unit { inputs: vec![port.into()], outputs: vec![port.into()], in_indexes: vec![0], out_indexes: vec![0], usb: None }
}

fn learned(port: &str) -> Learned {
    let words = osc_bridge::detect::port_words(port);
    Learned {
        fingerprint: json!({ "$schema": "https://w3id.org/osd/0.6/fingerprint.schema.json", "observed": "2026-11-21",
            "via": "native", "ports": { "inputs": [words], "outputs": [words] },
            "controls": [{ "type": "cc", "channel": 1, "number": 74 }] }),
        measured: json!({}),
    }
}

fn pairs(ports: &[&str]) -> Vec<(Unit, Learned)> {
    ports.iter().map(|p| (unit(p), learned(p))).collect()
}

fn report(ports: &[&str], service: &ServiceMatches) -> Value {
    learn_report(&pairs(ports), &catalogue(), Duration::from_millis(5000), service)
}

fn device(r: &Value, at: usize) -> &Value { &r["structuredContent"]["devices"][at] }

fn count(r: &Value) -> usize { r["structuredContent"]["devices"].as_array().unwrap().len() }

fn candidate(id: &str, name: &str) -> Value { json!({ "device": id, "name": name, "score": 90 }) }

const UNKNOWN: &str = "Zork Thing 1";
const KNOWN_LOCALLY: &str = "Acme Box 1";

// ---- the ports of the system are no device ----

#[test]
fn the_midi_through_client_of_alsa_is_a_port_of_the_system_whatever_its_numbers_or_case() {
    for label in ["Midi Through:Midi Through Port-0 14:0", "midi through:Midi Through Port-0 20:3",
                  "MIDI THROUGH:Midi Through Port-1 14:1", "Midi Through"] {
        assert!(is_system_port(label), "{label}");
    }
}

#[test]
fn a_real_device_or_a_name_only_containing_midi_through_is_not_a_port_of_the_system() {
    for label in ["Minilab3:Minilab3 MIDI 24:0", "Zork Thing 1", "Acme:Midi Through Box 20:0",
                  "Through:Midi Through Port-0 14:0", ""] {
        assert!(!is_system_port(label), "{label}");
    }
}

#[test]
fn the_report_leaves_out_a_port_of_the_system_and_keeps_a_real_device_beside_it() {
    let through = "Midi Through:Midi Through Port-0 14:0";
    for service in [ServiceMatches::NotLinked, ServiceMatches::Unreachable,
                    ServiceMatches::Answered(vec![vec![], vec![]])] {
        let r = report(&[through, UNKNOWN], &service);
        assert_eq!(count(&r), 1, "{r}");
        assert_eq!(device(&r, 0)["port"], UNKNOWN, "{r}");
        assert!(!r.to_string().contains("Midi Through"), "{r}");
    }
}

#[test]
fn the_learn_mode_listens_to_no_port_of_the_system() {
    let through = unit("Midi Through:Midi Through Port-0 14:0");
    let learned = learn_units(&[through], Duration::from_millis(1000)).unwrap();
    assert!(learned.is_empty());
}

// ---- the service names a driver ----

#[test]
fn a_device_the_service_names_a_driver_for_carries_that_driver_and_no_next_step() {
    let service = ServiceMatches::Answered(vec![vec![candidate("arturia/minilab3", "Arturia MiniLab 3")]]);
    let r = report(&[UNKNOWN], &service);
    assert_eq!(device(&r, 0)["driver"], json!({ "id": "arturia/minilab3", "name": "Arturia MiniLab 3" }), "{r}");
    assert!(device(&r, 0).get("next_step").is_none(), "{r}");
    assert!(!r["content"][0]["text"].as_str().unwrap().contains("next_step"), "{r}");
}

#[test]
fn the_service_matches_follow_the_devices_in_order() {
    let service = ServiceMatches::Answered(vec![vec![], vec![candidate("a/one", "One")]]);
    let r = report(&[UNKNOWN, "Quux Board 1"], &service);
    assert!(device(&r, 0).get("driver").is_none() && device(&r, 0).get("next_step").is_some(), "{r}");
    assert_eq!(device(&r, 1)["driver"]["id"], "a/one", "{r}");
    assert!(device(&r, 1).get("next_step").is_none(), "{r}");
}

#[test]
fn the_first_candidate_of_the_service_is_the_driver() {
    let service = ServiceMatches::Answered(vec![vec![candidate("a/best", "Best"), candidate("a/other", "Other")]]);
    let r = report(&[UNKNOWN], &service);
    assert_eq!(device(&r, 0)["driver"], json!({ "id": "a/best", "name": "Best" }), "{r}");
}

#[test]
fn the_service_driver_wins_over_the_local_one_when_both_recognise_the_device() {
    let service = ServiceMatches::Answered(vec![vec![candidate("service/box", "Box from the service")]]);
    let r = report(&[KNOWN_LOCALLY], &service);
    assert_eq!(device(&r, 0)["driver"], json!({ "id": "service/box", "name": "Box from the service" }), "{r}");
    assert!(device(&r, 0).get("next_step").is_none(), "{r}");
}

// ---- the local catalogue names a driver ----

#[test]
fn a_device_the_local_catalogue_recognises_carries_its_local_driver_linked_or_not() {
    let index = catalogue();
    // The port name also matches the vendor-doc variant, which the local ranking puts first
    // (the same choice `detect_hardware` makes).
    let entry = index.find("acme/box.vendor.fw-2").expect("fixture driver acme/box.vendor.fw-2");
    for service in [ServiceMatches::NotLinked, ServiceMatches::Unreachable, ServiceMatches::Answered(vec![vec![]])] {
        let r = report(&[KNOWN_LOCALLY], &service);
        assert_eq!(device(&r, 0)["driver"], json!({ "id": entry.id, "name": entry.name }), "{r}");
        assert!(device(&r, 0).get("next_step").is_none(), "{r}");
    }
}

// ---- the next step is worded by what was looked at ----

#[test]
fn linked_and_unknown_the_next_step_says_neither_the_local_catalogue_nor_synthhub_knows_it() {
    let r = report(&[UNKNOWN], &ServiceMatches::Answered(vec![vec![]]));
    let step = device(&r, 0)["next_step"].as_str().unwrap_or_else(|| panic!("{r}")).to_string();
    let low = step.to_lowercase();
    assert!(low.contains("neither the local catalogue nor synthhub"), "{step}");
    assert!(low.contains("name") && step.contains("report_mapping") && step.contains("submit_driver"), "{step}");
    assert!(device(&r, 0).get("driver").is_none(), "{r}");
}

#[test]
fn not_linked_the_next_step_says_only_the_local_catalogue_was_looked_at() {
    let r = report(&[UNKNOWN], &ServiceMatches::NotLinked);
    let step = device(&r, 0)["next_step"].as_str().unwrap_or_else(|| panic!("{r}")).to_string();
    let low = step.to_lowercase();
    assert!(low.contains("only the local catalogue"), "{step}");
    assert!(!low.contains("neither"), "{step}");
    assert!(low.contains("name") && step.contains("report_mapping") && step.contains("submit_driver"), "{step}");
}

#[test]
fn an_unreachable_service_still_reports_with_a_notice_and_the_offline_next_step() {
    let r = report(&[UNKNOWN], &ServiceMatches::Unreachable);
    assert_eq!(count(&r), 1, "{r}");
    let notice = r["structuredContent"]["notice"].as_str().unwrap().to_lowercase();
    assert!(notice.contains("did not answer"), "{notice}");
    assert!(r["content"][0]["text"].as_str().unwrap().to_lowercase().contains("did not answer"), "{r}");
    let step = device(&r, 0)["next_step"].as_str().unwrap_or_else(|| panic!("{r}")).to_lowercase();
    assert!(step.contains("only the local catalogue") && !step.contains("neither"), "{step}");
}

#[test]
fn a_service_that_answered_adds_no_unreachable_notice() {
    for service in [ServiceMatches::NotLinked, ServiceMatches::Answered(vec![vec![]])] {
        let r = report(&[UNKNOWN], &service);
        assert!(!r["structuredContent"]["notice"].as_str().unwrap().to_lowercase().contains("did not answer"), "{r}");
    }
}

// ---- asking the service ----

#[test]
fn not_linked_no_service_is_asked() {
    assert!(matches!(learn_service_matches(None, &pairs(&[UNKNOWN])), ServiceMatches::NotLinked));
}

#[test]
fn a_service_that_cannot_be_reached_is_told_unreachable_and_does_not_fail() {
    let client = osc_bridge::online::Online::keeping_account_in("http://127.0.0.1:1", None);
    let got = learn_service_matches(Some(&client), &pairs(&[UNKNOWN]));
    assert!(matches!(got, ServiceMatches::Unreachable));
}

#[test]
fn a_service_that_answers_is_asked_each_device_and_its_candidates_come_back_in_order_without_the_system_port() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let seen = bodies.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let mut body = String::new();
            std::io::Read::read_to_string(req.as_reader(), &mut body).unwrap();
            seen.lock().unwrap().push((req.url().to_string(), body));
            let answer = json!({ "ports": [
                { "candidates": [candidate("arturia/minilab3", "Arturia MiniLab 3"), candidate("a/other", "Other")] },
                { "candidates": [] } ] });
            let _ = req.respond(tiny_http::Response::from_string(answer.to_string()));
        }
    });
    let dir = crate::common::temp("learn-service-answers");
    let account = dir.path().join("user");
    std::fs::write(&account, "0123456789abcdef0123456789abcdef\n").unwrap();
    let client = osc_bridge::online::Online::keeping_account_in(&base, Some(account));

    let mut learned = pairs(&["Midi Through:Midi Through Port-0 14:0", "Minilab3:Minilab3 MIDI 24:0", UNKNOWN]);
    learned[1].1.fingerprint["identity_reply"] = json!("F0 7E 00 06 02 00 20 6B F7");
    let got = learn_service_matches(Some(&client), &learned);

    let ServiceMatches::Answered(candidates) = got else { panic!("{got:?}") };
    assert_eq!(candidates.len(), 2, "{candidates:?}");
    assert_eq!(candidates[0][0]["device"], "arturia/minilab3");
    assert_eq!(candidates[0][1]["device"], "a/other");
    assert!(candidates[1].is_empty());

    let got = bodies.lock().unwrap().clone();
    let detects: Vec<_> = got.iter().filter(|(u, _)| u == "/v1/detect").collect();
    assert_eq!(detects.len(), 1, "{got:?}");
    let sent: Value = serde_json::from_str(&detects[0].1).unwrap();
    let ports = sent["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 2, "{sent}");
    assert_eq!(ports[0]["port"], unit("Minilab3:Minilab3 MIDI 24:0").label(), "{sent}");
    assert_eq!(ports[0]["replies_hex"], json!(["F0 7E 00 06 02 00 20 6B F7"]), "{sent}");
    assert_eq!(ports[1]["port"], unit(UNKNOWN).label(), "{sent}");
    assert_eq!(ports[1]["replies_hex"], json!([]), "{sent}");
}
