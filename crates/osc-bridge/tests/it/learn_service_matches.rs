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

fn candidate(id: &str, name: &str) -> Value {
    json!({ "id": id, "slug": id.rsplit('/').next().unwrap_or(id), "name": name, "vendor": "Vendor", "source_tier": "official", "score": 90 })
}

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
    assert_eq!(candidates[0][0]["id"], "arturia/minilab3");
    assert_eq!(candidates[0][1]["id"], "a/other");
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

// ---- the USB ids go out, a device without a card comes back being confirmed ----

const BEING_CONFIRMED: &str = "being confirmed: not enough people agree yet, to confirm with the person";
const IN_DISAGREEMENT: &str = "in disagreement: several devices are named for it, to confirm with the person";

fn unit_with_usb(port: &str, usb: Option<(u16, u16)>) -> Unit {
    Unit { usb, ..unit(port) }
}

/// A service that gives `answer` to each detect request and keeps the requests it got.
fn service_answering(answer: Value) -> (osc_bridge::online::Online, std::sync::Arc<std::sync::Mutex<Vec<Value>>>, tempfile::TempDir) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let seen = bodies.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let mut body = String::new();
            std::io::Read::read_to_string(req.as_reader(), &mut body).unwrap();
            if req.url() == "/v1/detect" {
                seen.lock().unwrap().push(serde_json::from_str(&body).unwrap());
            }
            let _ = req.respond(tiny_http::Response::from_string(answer.to_string()));
        }
    });
    let dir = crate::common::temp("learn-service-usb");
    let account = dir.path().join("user");
    std::fs::write(&account, "0123456789abcdef0123456789abcdef\n").unwrap();
    (osc_bridge::online::Online::keeping_account_in(&base, Some(account)), bodies, dir)
}

fn unlisted(confirmations: u64, state: Option<&str>, label: Option<&str>, disputed: bool) -> Value {
    let mut u = json!({ "vendor": "Behringer", "model": "BCR32", "firmware": "1.0.3",
        "confirmations": confirmations, "disputed": disputed });
    if let Some(s) = state { u["state"] = json!(s); }
    if let Some(l) = label { u["state_label"] = json!(l); }
    u
}

fn learn_with(answer_port: Value) -> Value {
    let (client, _bodies, _dir) = service_answering(json!({ "ports": [answer_port] }));
    let learned = pairs(&[UNKNOWN]);
    let service = learn_service_matches(Some(&client), &learned);
    learn_report(&learned, &catalogue(), Duration::from_millis(5000), &service)
}

#[test]
fn the_learn_mode_sends_the_usb_ids_of_a_port_that_has_them_as_numbers_and_none_otherwise() {
    let (client, bodies, _dir) = service_answering(json!({ "ports": [{ "candidates": [] }, { "candidates": [] }] }));
    let learned = vec![
        (unit_with_usb("Behringer BCR32:BCR32 MIDI 20:0", Some((0x1397, 0x12cf))), learned("Behringer BCR32:BCR32 MIDI 20:0")),
        (unit_with_usb(UNKNOWN, None), learned(UNKNOWN)),
    ];
    let _ = learn_service_matches(Some(&client), &learned);
    let sent = bodies.lock().unwrap().clone();
    let ports = sent[0]["ports"].as_array().unwrap();
    assert_eq!(ports[0]["usb"], json!({ "vendor_id": 5015, "product_id": 4815 }), "{sent:?}");
    assert!(ports[1].get("usb").is_none(), "{sent:?}");
    assert_eq!(ports[0]["replies_hex"], json!([]), "the rest of the request is unchanged");
}

#[test]
fn a_device_without_a_card_is_reported_being_confirmed_with_its_maker_model_firmware_and_confirmations() {
    let r = learn_with(json!({ "candidates": [], "unlisted": [unlisted(1, Some("confirming"), Some(BEING_CONFIRMED), false)] }));
    let u = &device(&r, 0)["unlisted"][0];
    assert_eq!(u["vendor"], "Behringer", "{r}");
    assert_eq!(u["model"], "BCR32", "{r}");
    assert_eq!(u["firmware"], "1.0.3", "{r}");
    assert_eq!(u["confirmations"], 1, "{r}");
    assert_eq!(u["state"], "confirming", "{r}");
    assert_eq!(u["state_label"], BEING_CONFIRMED, "{r}");
    assert!(device(&r, 0).get("driver").is_none(), "no card, no driver: {r}");
}

#[test]
fn a_device_being_confirmed_is_asked_to_be_confirmed_not_named_again() {
    let r = learn_with(json!({ "candidates": [], "unlisted": [unlisted(1, Some("confirming"), Some(BEING_CONFIRMED), false)] }));
    let step = device(&r, 0)["next_step"].as_str().unwrap_or_else(|| panic!("{r}")).to_lowercase();
    assert!(step.contains("confirm"), "{step}");
    assert!(step.contains("behringer") && step.contains("bcr32") && step.contains("1.0.3"), "{step}");
    assert!(!step.contains("ask the person to name it"), "{step}");
}

#[test]
fn a_disputed_device_is_shown_as_disputed_with_every_device_named_and_none_given_as_sure() {
    let other = json!({ "vendor": "Behringer", "model": "BCR2000", "firmware": "1.0.3", "confirmations": 1,
        "disputed": true, "state": "confirming", "state_label": IN_DISAGREEMENT });
    let first = unlisted(1, Some("confirming"), Some(IN_DISAGREEMENT), true);
    let r = learn_with(json!({ "candidates": [], "unlisted": [first, other] }));
    let all = device(&r, 0)["unlisted"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(all.len(), 2, "{r}");
    assert!(all.iter().all(|u| u["disputed"] == true && u["state_label"] == IN_DISAGREEMENT), "{r}");
    assert_eq!(all[1]["model"], "BCR2000", "{r}");
    assert_eq!(all[1]["confirmations"], 1, "{r}");
}

#[test]
fn several_devices_without_a_card_are_all_reported_with_their_confirmations() {
    let a = unlisted(3, Some("confirming"), Some(BEING_CONFIRMED), false);
    let b = json!({ "vendor": "Zork", "model": "Thing", "firmware": "2", "confirmations": 7, "disputed": false });
    let r = learn_with(json!({ "candidates": [], "unlisted": [a, b] }));
    let all = device(&r, 0)["unlisted"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(all.len(), 2, "{r}");
    assert_eq!((all[0]["confirmations"].clone(), all[1]["confirmations"].clone()), (json!(3), json!(7)), "{r}");
    assert!(all[1].get("state").is_none(), "no state: confirmed: {r}");
}

#[test]
fn a_device_with_catalogue_candidates_is_never_also_reported_without_a_card() {
    let r = learn_with(json!({ "candidates": [candidate("arturia/minilab3", "Arturia MiniLab 3")],
        "unlisted": [unlisted(1, Some("confirming"), Some(BEING_CONFIRMED), false)] }));
    assert_eq!(device(&r, 0)["driver"]["id"], "arturia/minilab3", "{r}");
    assert!(device(&r, 0).get("unlisted").is_none(), "{r}");
}

#[test]
fn a_catalogue_device_the_service_gives_as_being_confirmed_is_reported_so() {
    let r = learn_with(json!({ "candidates": [candidate("arturia/minilab3", "Arturia MiniLab 3")],
        "learned": [{ "id": "arturia/minilab3", "confirmations": 1, "disputed": false,
            "state": "confirming", "state_label": BEING_CONFIRMED }] }));
    let driver = &device(&r, 0)["driver"];
    assert_eq!(driver["id"], "arturia/minilab3", "{r}");
    assert_eq!(driver["state"], "confirming", "{r}");
    assert_eq!(driver["state_label"], BEING_CONFIRMED, "{r}");
    assert_eq!(driver["confirmations"], 1, "{r}");
}

#[test]
fn a_catalogue_candidate_with_a_vendor_is_the_driver_and_never_a_device_without_a_card() {
    let r = learn_with(json!({ "candidates": [candidate("polyend/synth", "Synth")], "devices": [] }));
    assert_eq!(device(&r, 0)["driver"]["id"], "polyend/synth", "{r}");
    assert_eq!(device(&r, 0)["driver"]["name"], "Synth", "{r}");
    assert!(device(&r, 0).get("unlisted").is_none() && device(&r, 0).get("next_step").is_none(), "{r}");
    assert!(!r.to_string().contains("without a card") && !r.to_string().contains('?'), "{r}");
}

#[test]
fn the_next_step_names_a_device_without_a_card_only_when_the_service_lists_one_and_no_candidate_exists() {
    let r = learn_with(json!({ "candidates": [], "unlisted": [unlisted(1, Some("confirming"), Some(BEING_CONFIRMED), false)] }));
    assert!(device(&r, 0)["next_step"].as_str().unwrap_or_else(|| panic!("{r}")).contains("names this device without a card"), "{r}");
    let r = learn_with(json!({ "candidates": [], "unlisted": [] }));
    assert!(!r.to_string().contains("without a card"), "{r}");
    let r = learn_with(json!({ "candidates": [candidate("polyend/synth", "Synth")],
        "unlisted": [unlisted(1, Some("confirming"), Some(BEING_CONFIRMED), false)] }));
    assert!(!r.to_string().contains("without a card"), "{r}");
}

#[test]
fn a_device_without_a_card_missing_a_field_is_named_without_it_and_never_with_a_question_mark_or_null() {
    let r = learn_with(json!({ "candidates": [], "unlisted": [{ "model": "Thing", "firmware": "2" }, { "vendor": "Zork" }] }));
    let step = device(&r, 0)["next_step"].as_str().unwrap_or_else(|| panic!("{r}"));
    assert!(step.contains("Thing") && step.contains("Zork"), "{step}");
    assert!(!step.contains('?') && !step.contains("null"), "{step}");
}

#[test]
fn an_entry_of_the_unlisted_list_with_neither_vendor_nor_model_is_not_a_device() {
    let r = learn_with(json!({ "candidates": [], "unlisted": [{ "firmware": "2", "confirmations": 1 }] }));
    assert!(device(&r, 0).get("unlisted").is_none(), "{r}");
    assert!(!device(&r, 0)["next_step"].as_str().unwrap_or_else(|| panic!("{r}")).contains("without a card"), "{r}");
}

#[test]
fn a_catalogue_device_the_service_does_not_list_as_learned_carries_no_state() {
    let r = learn_with(json!({ "candidates": [candidate("arturia/minilab3", "Arturia MiniLab 3")] }));
    assert!(device(&r, 0)["driver"].get("state").is_none(), "{r}");
}

#[test]
fn an_unreachable_or_unlinked_service_reports_no_device_being_confirmed() {
    for service in [ServiceMatches::NotLinked, ServiceMatches::Unreachable] {
        let r = report(&[UNKNOWN], &service);
        assert!(device(&r, 0).get("unlisted").is_none(), "{r}");
        assert!(!r.to_string().contains("confirming"), "{r}");
    }
}
