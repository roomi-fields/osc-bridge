//! The report of a detection whose devices the service names (`POST /v1/detect`, the `devices`
//! of each port): one answer per device with its state, proofs and card, the run command, and the
//! entries asked again alone when the service groups ports that the report keeps apart.

use crate::common::{temp, FakeHardware};
use osc_bridge::capture::{Capture, FORMAT_VERSION};
use osc_bridge::detect::Probed;
use osc_bridge::mcp::{detect_tool, format_detection, DeviceIndex, McpOptions};
use osc_bridge::recognition::Identity;
use osc_bridge::synthub::catalogue::tier_rank;
use osc_bridge::synthub::learn::{learn_report, learn_service_matches, Learned, Unit};
use osc_bridge::synthub::online::Online;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BOX_A: &str = "Box:Box A 20:0";
const BOX_B: &str = "Box:Box B 20:1";
const PAD: &str = "Pad:Pad 24:0";
const ZED: &str = "Zed:Zed 28:0";

/// A card the local catalogue of the fixtures knows (source `contributed`).
const KNOWN: &str = "acme/box";
/// A card it does not know.
const UNKNOWN_CARD: &str = "polyend/synth";

const RECOGNISED: &str = "recognised";
const PROBABLE: &str = "probable";
const POSSIBLE: &str = "possible, to confirm";
const BEING_CONFIRMED: &str = "being confirmed: not enough people agree yet, to confirm with the person";
const IN_DISAGREEMENT: &str = "in disagreement: several devices are named for it, to confirm with the person";

// ---- the stand-in for the service -----------------------------------------------------------

struct Service {
    client: Online,
    /// The ports of each detect request, in the order they came.
    asked: Arc<Mutex<Vec<Vec<String>>>>,
    _dir: tempfile::TempDir,
}

impl Service {
    fn requests(&self) -> Vec<Vec<String>> {
        self.asked.lock().unwrap().clone()
    }
}

/// A service whose answer for a port is `answer(port, ports of the request)`.
fn service_by_request(answer: impl Fn(&str, &[String]) -> Value + Send + 'static) -> Service {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let asked = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let seen = asked.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let mut body = String::new();
            std::io::Read::read_to_string(req.as_reader(), &mut body).unwrap();
            let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let ports: Vec<String> = body["ports"].as_array().into_iter().flatten()
                .map(|p| p["port"].as_str().unwrap().to_string()).collect();
            let answers: Vec<Value> = ports.iter().map(|p| {
                let mut a = answer(p, &ports);
                a["port"] = json!(p);
                a
            }).collect();
            seen.lock().unwrap().push(ports);
            let _ = req.respond(tiny_http::Response::from_string(json!({ "ports": answers }).to_string()));
        }
    });
    let dir = temp("service-devices");
    let account = dir.path().join("user");
    std::fs::write(&account, "0123456789abcdef0123456789abcdef\n").unwrap();
    Service { client: Online::keeping_account_in(&base, Some(account)), asked, _dir: dir }
}

/// A service giving each port of `table` its answer, and no device for any other port.
fn service(table: Vec<(&'static str, Value)>) -> Service {
    service_by_request(move |port, _| table.iter().find(|(p, _)| *p == port).map(|(_, a)| a.clone())
        .unwrap_or_else(|| json!({ "devices": [] })))
}

fn devices(list: Vec<Value>) -> Value {
    json!({ "devices": list })
}

fn proof(nature: &str, kind: &str, port: &str) -> Value {
    json!({ "nature": nature, "kind": kind, "port": port })
}

fn entry(id: &str, probability: Option<f64>, proofs: Vec<Value>) -> Value {
    let mut d = json!({ "id": id, "name": format!("Name of {id}"), "proofs": proofs });
    if let Some(p) = probability {
        d["probability"] = json!(p);
    }
    d
}

fn strong(id: &str, probability: f64, port: &str) -> Value {
    entry(id, Some(probability), vec![proof("strong", "identity reply", port)])
}

fn probable(id: &str, probability: f64, port: &str) -> Value {
    entry(id, Some(probability), vec![proof("medium", "maker", port)])
}

fn possible(id: &str, port: &str) -> Value {
    entry(id, None, vec![proof("weak", "name", port)])
}

// ---- the detection, as the linked server runs it --------------------------------------------

fn fixtures() -> DeviceIndex {
    DeviceIndex::build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/devices"), tier_rank)
}

/// The report of `detect_hardware` over `ports`, the service matching them.
fn detect(service: &Service, ports: &[&str]) -> Vec<Value> {
    let mut hardware = FakeHardware::with_ports(ports);
    hardware.inputs = hardware.ports.clone();
    let options = McpOptions { devices_dir: None::<PathBuf>, default_target: "127.0.0.1:9000".parse().unwrap(),
        tier_order: tier_rank, detect_mode: None };
    let answers = |found: &[Probed]| osc_bridge::synthub::mcp::detect_answers(&service.client, found);
    let report = detect_tool(&json!({}), &fixtures(), &options, None, &answers, &hardware.opener()).unwrap();
    report["structuredContent"]["devices"].as_array().unwrap().clone()
}

fn candidates(device: &Value) -> &Vec<Value> {
    device["candidates"].as_array().unwrap_or_else(|| panic!("no candidates: {device}"))
}

fn ids(device: &Value) -> Vec<&str> {
    candidates(device).iter().map(|c| c["device"].as_str().unwrap_or_else(|| panic!("no device: {c}"))).collect()
}

fn entry_of<'a>(report: &'a [Value], port: &str) -> &'a Value {
    report.iter().find(|d| d["ports"].as_array().unwrap().iter().any(|p| p["port"] == port))
        .unwrap_or_else(|| panic!("{port} in {report:?}"))
}

/// The nature and kind of each proof of a candidate, sorted.
fn proofs(candidate: &Value) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = candidate["proofs"].as_array().unwrap_or_else(|| panic!("no proofs: {candidate}")).iter()
        .map(|p| (p["nature"].as_str().unwrap().to_string(), p["kind"].as_str().unwrap().to_string())).collect();
    all.sort();
    all
}

fn pair(nature: &str, kind: &str) -> (String, String) {
    (nature.to_string(), kind.to_string())
}

fn text(report: &[Value]) -> String {
    format_detection(report, true)
}

// ---- the devices of the service are what the report reads ------------------------------------

#[test]
fn a_device_the_service_lists_is_reported_once_with_its_state_proofs_and_card() {
    let s = service(vec![(BOX_A, devices(vec![strong(KNOWN, 0.97, BOX_A)]))]);
    let report = detect(&s, &[BOX_A]);
    assert_eq!(report.len(), 1, "{report:?}");
    assert_eq!(ids(&report[0]), [KNOWN], "{report:?}");
    let c = &candidates(&report[0])[0];
    assert_eq!(c["state_label"], RECOGNISED, "{c}");
    assert_eq!(proofs(c), [pair("strong", "identity reply")], "{c}");
    assert!(c["file"].as_str().unwrap_or_default().ends_with("box.json"), "{c}");
    assert_eq!(report[0]["run_command"], "osc-bridge run --device acme/box --out-port 0 --in-port 0", "{report:?}");
    assert_eq!(s.requests(), [vec![BOX_A.to_string()]]);
}

#[test]
fn an_answer_carrying_only_the_former_candidates_list_names_no_device() {
    let former = json!({ "candidates": [{ "id": KNOWN, "slug": "box", "name": "Box", "vendor": "Acme",
        "source_tier": "official", "score": 150, "proofs": [{ "nature": "strong", "kind": "identity reply" }] }] });
    let s = service(vec![(BOX_A, former)]);
    let report = detect(&s, &[BOX_A]);
    assert!(candidates(&report[0]).is_empty(), "{report:?}");
    assert!(report[0]["run_command"].is_null(), "{report:?}");
}

#[test]
fn a_device_the_service_names_by_the_name_alone_proposes_no_run_command() {
    let s = service(vec![(BOX_A, devices(vec![possible(KNOWN, BOX_A)]))]);
    let report = detect(&s, &[BOX_A]);
    assert_eq!(ids(&report[0]), [KNOWN], "{report:?}");
    assert_eq!(candidates(&report[0])[0]["state_label"], POSSIBLE, "{report:?}");
    assert!(report[0]["run_command"].is_null(), "{report:?}");
    assert!(!text(&report).contains("run:"), "{report:?}");
}

#[test]
fn a_probable_device_proposes_no_run_command() {
    let s = service(vec![(BOX_A, devices(vec![probable(KNOWN, 0.99, BOX_A)]))]);
    let report = detect(&s, &[BOX_A]);
    assert_eq!(ids(&report[0]), [KNOWN], "{report:?}");
    assert!(report[0]["run_command"].is_null(), "{report:?}");
}

#[test]
fn a_port_the_service_groups_with_another_port_of_its_device_does_not_repeat_the_device() {
    let s = service(vec![
        (BOX_A, devices(vec![strong(KNOWN, 0.9, BOX_A)])),
        (BOX_B, json!({ "devices": [], "shared_with": BOX_A })),
    ]);
    let report = detect(&s, &[BOX_A, BOX_B]);
    assert_eq!(report.len(), 1, "{report:?}");
    assert_eq!(ids(&report[0]), [KNOWN], "{report:?}");
    assert_eq!(text(&report).matches("match:").count(), 1, "{report:?}");
    assert_eq!(s.requests().len(), 1, "a shared_with inside its entry asks nothing again: {:?}", s.requests());
}

// ---- the state of each device ----------------------------------------------------------------

#[test]
fn each_state_is_written_in_state_label_and_in_the_text_of_the_command() {
    let s = service(vec![
        (BOX_A, devices(vec![strong(KNOWN, 0.97, BOX_A)])),
        (PAD, devices(vec![probable("acme/pad", 0.55, PAD)])),
        (ZED, devices(vec![possible("acme/zed", ZED)])),
    ]);
    let report = detect(&s, &[BOX_A, PAD, ZED]);
    for (port, label) in [(BOX_A, RECOGNISED), (PAD, PROBABLE), (ZED, POSSIBLE)] {
        let one = entry_of(&report, port);
        assert_eq!(candidates(one)[0]["state_label"], label, "{one}");
        let said = format_detection(std::slice::from_ref(one), true);
        assert!(said.contains(label), "{label}: {said}");
        for other in [RECOGNISED, PROBABLE, POSSIBLE].into_iter().filter(|o| *o != label) {
            assert!(!said.contains(other), "{other} for a {label} device: {said}");
        }
    }
}

#[test]
fn a_state_label_the_service_gives_on_a_device_stays_as_given_with_its_confirmation_fields() {
    for (device, label, disputed) in [(strong(KNOWN, 0.9, BOX_A), BEING_CONFIRMED, false),
                                      (probable(KNOWN, 0.4, BOX_A), IN_DISAGREEMENT, true)] {
        let mut device = device;
        device["confirmations"] = json!(2);
        device["disputed"] = json!(disputed);
        device["state"] = json!("confirming");
        device["state_label"] = json!(label);
        let s = service(vec![(BOX_A, devices(vec![device]))]);
        let report = detect(&s, &[BOX_A]);
        let c = &candidates(&report[0])[0];
        assert_eq!(c["state_label"], label, "{c}");
        assert_eq!(c["state"], "confirming", "{c}");
        assert_eq!(c["confirmations"], 2, "{c}");
        assert_eq!(c["disputed"], disputed, "{c}");
        assert!(text(&report).contains(label), "{}", text(&report));
    }
}

#[test]
fn the_offline_report_labels_a_card_with_a_strong_proof_recognised() {
    let reply = Identity { manufacturer: vec![0, 33, 9], family: 1, model: 1, version: [2, 0, 0, 0] }.reply();
    let capture = Capture::parse(&json!({
        "format_version": FORMAT_VERSION, "outputs": [BOX_A], "inputs": [BOX_A], "cards": "", "usbids": [], "clients": "",
        "requests": [], "received": [{ "port": BOX_A, "bytes": reply, "ms": 5 }],
    }).to_string()).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/devices");
    let report = osc_bridge::mcp::detect_capture(&root, &capture, tier_rank).unwrap();
    assert_eq!(candidates(&report[0])[0]["state_label"], RECOGNISED, "{report:?}");
    assert!(text(&report).contains(RECOGNISED), "{}", text(&report));
}

// ---- what the report carries of a device of the service ---------------------------------------

#[test]
fn a_device_of_the_service_carries_its_id_proofs_probability_and_firmware_with_the_local_card_when_known() {
    let mut known = strong(KNOWN, 0.91, BOX_A);
    known["firmware"] = json!("2.4.1");
    let s = service(vec![(BOX_A, devices(vec![known])), (PAD, devices(vec![probable(UNKNOWN_CARD, 0.35, PAD)]))]);
    let report = detect(&s, &[BOX_A, PAD]);

    let c = &candidates(entry_of(&report, BOX_A))[0];
    assert_eq!(c["device"], KNOWN, "{c}");
    assert_eq!(c["probability"], json!(0.91), "{c}");
    assert_eq!(c["firmware"], "2.4.1", "{c}");
    assert_eq!(c["proofs"], json!([proof("strong", "identity reply", BOX_A)]), "{c}");
    assert!(c["file"].as_str().unwrap_or_default().ends_with("box.json"), "{c}");
    assert_eq!(c["source_tier"], "contributed", "the source of the local card, for its credit: {c}");
    assert!(c.get("score").is_none(), "{c}");

    let c = &candidates(entry_of(&report, PAD))[0];
    assert_eq!(c["device"], UNKNOWN_CARD, "{c}");
    assert_eq!(c["probability"], json!(0.35), "{c}");
    for absent in ["file", "source_tier", "score", "firmware"] {
        assert!(c.get(absent).is_none(), "{absent}: {c}");
    }
}

#[test]
fn a_possible_device_of_the_service_has_no_probability_key() {
    let s = service(vec![(BOX_A, devices(vec![possible(KNOWN, BOX_A)]))]);
    let report = detect(&s, &[BOX_A]);
    let c = &candidates(&report[0])[0];
    assert_eq!(c["device"], KNOWN, "{c}");
    assert!(c.get("probability").is_none(), "{c}");
}

#[test]
fn the_command_prints_the_probability_and_the_firmware_and_no_score_null_or_empty_field() {
    let mut known = strong(KNOWN, 0.91, BOX_A);
    known["firmware"] = json!("2.4.1");
    let s = service(vec![
        (BOX_A, devices(vec![known])),
        (PAD, devices(vec![probable(UNKNOWN_CARD, 0.35, PAD)])),
        (ZED, devices(vec![possible("acme/zed", ZED)])),
    ]);
    let report = detect(&s, &[BOX_A, PAD, ZED]);
    let said = |port: &str| format_detection(std::slice::from_ref(entry_of(&report, port)), true);
    let box_text = said(BOX_A);
    assert!(box_text.contains("0.91") || box_text.contains("91%") || box_text.contains("91 %"), "{box_text}");
    assert!(box_text.contains("firmware 2.4.1"), "{box_text}");
    assert!(box_text.contains("because: identity reply (strong)"), "{box_text}");
    let pad_text = said(PAD);
    assert!(pad_text.contains("0.35") || pad_text.contains("35%") || pad_text.contains("35 %"), "{pad_text}");
    assert!(pad_text.contains(UNKNOWN_CARD), "{pad_text}");
    assert!(said(ZED).contains("acme/zed"), "{}", said(ZED));
    let all = text(&report);
    for refused in ["score", "null", "()", "(,", ", )", "?"] {
        assert!(!all.contains(refused), "{refused:?} in {all}");
    }
}

#[test]
fn the_run_command_and_the_ports_of_the_entry_are_those_of_the_port_the_service_names_in_the_strong_proof() {
    let s = service(vec![
        (BOX_A, devices(vec![strong(KNOWN, 0.9, BOX_B)])),
        (BOX_B, json!({ "devices": [], "shared_with": BOX_A })),
    ]);
    let report = detect(&s, &[BOX_A, BOX_B]);
    assert_eq!(candidates(&report[0])[0]["proofs"][0]["port"], BOX_B, "{report:?}");
    assert_eq!(report[0]["port"], BOX_B, "{report:?}");
    assert_eq!(report[0]["run_command"], "osc-bridge run --device acme/box --out-port 1 --in-port 1", "{report:?}");
}

// ---- one id named on several ports -------------------------------------------------------------

#[test]
fn a_device_named_on_two_ports_appears_once_with_each_proof_once_and_the_greatest_probability() {
    for (first, second) in [(0.4, 0.7), (0.7, 0.4)] {
        let on = |p: f64, port: &str| entry(KNOWN, Some(p), vec![proof("medium", "maker", port), proof("weak", "name", port)]);
        let s = service(vec![(BOX_A, devices(vec![on(first, BOX_A)])), (BOX_B, devices(vec![on(second, BOX_B)]))]);
        let report = detect(&s, &[BOX_A, BOX_B]);
        assert_eq!(ids(&report[0]), [KNOWN], "{report:?}");
        let c = &candidates(&report[0])[0];
        assert_eq!(proofs(c), [pair("medium", "maker"), pair("weak", "name")], "{c}");
        assert_eq!(c["probability"], json!(0.7), "the greatest, never the sum: {c}");
        assert_eq!(c["state_label"], PROBABLE, "{c}");
    }
}

#[test]
fn a_strong_proof_on_one_port_makes_the_merged_device_recognised_whatever_the_other_port_says() {
    for (weaker, greatest) in [(possible(KNOWN, BOX_A), 0.8), (probable(KNOWN, 0.95, BOX_A), 0.95)] {
        let s = service(vec![(BOX_A, devices(vec![weaker])), (BOX_B, devices(vec![strong(KNOWN, 0.8, BOX_B)]))]);
        let report = detect(&s, &[BOX_A, BOX_B]);
        assert_eq!(ids(&report[0]), [KNOWN], "{report:?}");
        let c = &candidates(&report[0])[0];
        assert_eq!(c["state_label"], RECOGNISED, "{c}");
        assert_eq!(c["probability"], json!(greatest), "{c}");
        assert_eq!(proofs(c).len(), 2, "{c}");
        assert_eq!(report[0]["port"], BOX_B, "{report:?}");
    }
}

#[test]
fn a_probable_port_and_a_name_only_port_of_one_device_make_it_probable() {
    let s = service(vec![(BOX_A, devices(vec![possible(KNOWN, BOX_A)])), (BOX_B, devices(vec![probable(KNOWN, 0.6, BOX_B)]))]);
    let report = detect(&s, &[BOX_A, BOX_B]);
    assert_eq!(ids(&report[0]), [KNOWN], "{report:?}");
    let c = &candidates(&report[0])[0];
    assert_eq!(c["state_label"], PROBABLE, "{c}");
    assert_eq!(c["probability"], json!(0.6), "{c}");
}

#[test]
fn devices_are_ranked_by_decreasing_probability_the_possible_ones_last() {
    let s = service(vec![
        (BOX_A, devices(vec![possible("acme/name-only", BOX_A), probable("acme/low", 0.2, BOX_A)])),
        (BOX_B, devices(vec![probable("acme/high", 0.8, BOX_B)])),
    ]);
    let report = detect(&s, &[BOX_A, BOX_B]);
    assert_eq!(ids(&report[0]), ["acme/high", "acme/low", "acme/name-only"], "{report:?}");
}

#[test]
fn once_a_device_has_a_strong_proof_the_devices_without_one_are_not_listed() {
    let s = service(vec![(BOX_A, devices(vec![
        probable("acme/likelier", 0.99, BOX_A), strong(KNOWN, 0.5, BOX_A), possible("acme/name-only", BOX_A),
    ]))]);
    let report = detect(&s, &[BOX_A]);
    assert_eq!(ids(&report[0]), [KNOWN], "{report:?}");
    assert!(report[0]["run_command"].as_str().unwrap_or_default().contains("--device acme/box "), "{report:?}");
}

// ---- the run command ---------------------------------------------------------------------------

#[test]
fn two_devices_of_the_service_with_a_strong_proof_propose_no_command_whatever_their_probabilities() {
    let same_port = vec![(BOX_A, devices(vec![strong(KNOWN, 0.99, BOX_A), strong(UNKNOWN_CARD, 0.10, BOX_A)]))];
    let two_ports = vec![(BOX_A, devices(vec![strong(KNOWN, 0.91, BOX_A)])), (BOX_B, devices(vec![strong(UNKNOWN_CARD, 0.90, BOX_B)]))];
    for table in [same_port, two_ports] {
        let s = service(table);
        let report = detect(&s, &[BOX_A, BOX_B]);
        assert_eq!(ids(&report[0]), [KNOWN, UNKNOWN_CARD], "{report:?}");
        for c in candidates(&report[0]) {
            assert_eq!(proofs(c), [pair("strong", "identity reply")], "{c}");
        }
        assert!(report[0]["run_command"].is_null(), "{report:?}");
        let said = text(&report);
        assert!(!said.contains("run:"), "{said}");
        assert_eq!(said.matches("because: identity reply (strong)").count(), 2, "{said}");
    }
}

// ---- ports the service groups across two entries -------------------------------------------------

fn sorted(requests: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut all: Vec<Vec<String>> = requests.iter().map(|r| { let mut r = r.clone(); r.sort(); r }).collect();
    all.sort();
    all
}

fn names(ports: &[&str]) -> Vec<String> {
    ports.iter().map(|p| p.to_string()).collect()
}

/// A service that groups `PAD` with `BOX_B` when asked both, and answers each device's ports
/// asked alone with that device: `pad` is what it says of `PAD` alone.
fn grouping_box_and_pad(pad: Value) -> Service {
    service_by_request(move |port, asked| {
        let grouped = asked.iter().any(|p| p == PAD) && asked.iter().any(|p| p == BOX_B);
        match port {
            BOX_A if grouped => devices(vec![strong("acme/both", 0.9, BOX_A), strong("acme/pad", 0.9, PAD)]),
            BOX_A => devices(vec![strong(KNOWN, 0.9, BOX_A)]),
            PAD if grouped => json!({ "devices": [], "shared_with": BOX_A }),
            PAD => pad.clone(),
            ZED => devices(vec![strong("acme/zed", 0.8, ZED)]),
            _ => json!({ "devices": [], "shared_with": BOX_A }),
        }
    })
}

#[test]
fn entries_grouped_by_the_service_are_each_asked_again_alone_and_show_their_own_answer() {
    let s = grouping_box_and_pad(devices(vec![strong("acme/pad", 0.7, PAD)]));
    let report = detect(&s, &[BOX_A, BOX_B, PAD, ZED]);
    assert_eq!(report.len(), 3, "{report:?}");
    assert_eq!(ids(entry_of(&report, BOX_A)), [KNOWN], "{report:?}");
    assert_eq!(ids(entry_of(&report, PAD)), ["acme/pad"], "{report:?}");
    assert_eq!(ids(entry_of(&report, ZED)), ["acme/zed"], "{report:?}");
    assert_eq!(entry_of(&report, BOX_A)["run_command"], "osc-bridge run --device acme/box --out-port 0 --in-port 0", "{report:?}");
    assert_eq!(entry_of(&report, PAD)["run_command"], "osc-bridge run --device acme/pad --out-port 2 --in-port 2", "{report:?}");
    assert!(candidates(entry_of(&report, PAD))[0].get("file").is_none(), "{report:?}");

    let requests = s.requests();
    assert_eq!(requests.len(), 3, "the first request, then each concerned entry alone: {requests:?}");
    assert_eq!(requests[0], names(&[BOX_A, BOX_B, PAD, ZED]));
    assert_eq!(sorted(&requests[1..]), sorted(&[names(&[BOX_A, BOX_B]), names(&[PAD])]), "{requests:?}");
}

#[test]
fn an_entry_whose_own_answer_names_no_device_is_said_unknown() {
    let s = grouping_box_and_pad(devices(vec![]));
    let report = detect(&s, &[BOX_A, BOX_B, PAD]);
    let pad = entry_of(&report, PAD);
    assert!(candidates(pad).is_empty() && pad["run_command"].is_null(), "{pad}");
    let said = format_detection(std::slice::from_ref(pad), true);
    assert!(said.contains("unknown") && said.contains("--learn"), "{said}");
    assert_eq!(ids(entry_of(&report, BOX_A)), [KNOWN], "{report:?}");
}

#[test]
fn devices_the_service_groups_with_no_other_entry_are_asked_in_one_request() {
    let s = service(vec![
        (BOX_A, devices(vec![strong(KNOWN, 0.9, BOX_A)])),
        (PAD, devices(vec![strong("acme/pad", 0.9, PAD)])),
    ]);
    let report = detect(&s, &[BOX_A, PAD]);
    assert_eq!(ids(entry_of(&report, PAD)), ["acme/pad"], "{report:?}");
    assert_eq!(s.requests(), [names(&[BOX_A, PAD])]);
}

fn unit(port: &str) -> (Unit, Learned) {
    let words = osc_bridge::synthub::learn::port_words(port);
    (Unit { inputs: vec![port.into()], outputs: vec![port.into()], in_indexes: vec![0], out_indexes: vec![0], usb: None,
            client_kind: osc_bridge::detect::ClientKind::Unknown },
     Learned { fingerprint: json!({ "ports": { "inputs": [words], "outputs": [words] } }), measured: json!({}) })
}

#[test]
fn the_learn_mode_asks_again_alone_the_devices_the_service_groups_and_gives_each_its_own_driver() {
    let s = service_by_request(|port, asked| {
        let grouped = asked.len() > 1 && asked.iter().any(|p| p == PAD) && asked.iter().any(|p| p == BOX_A);
        match port {
            BOX_A if grouped => devices(vec![strong("acme/both", 0.9, BOX_A)]),
            BOX_A => devices(vec![strong(KNOWN, 0.9, BOX_A)]),
            PAD if grouped => json!({ "devices": [], "shared_with": BOX_A }),
            PAD => devices(vec![strong("acme/pad", 0.7, PAD)]),
            _ => devices(vec![strong("acme/zed", 0.8, ZED)]),
        }
    });
    let learned = vec![unit(BOX_A), unit(PAD), unit(ZED)];
    let matches = learn_service_matches(Some(&s.client), &learned);
    let report = learn_report(&learned, &fixtures(), Duration::from_millis(5000), &matches, tier_rank);
    let of = |at: usize| report["structuredContent"]["devices"][at].clone();
    assert_eq!(of(0)["driver"]["id"], KNOWN, "{report}");
    assert_eq!(of(1)["driver"]["id"], "acme/pad", "{report}");
    assert_eq!(of(2)["driver"]["id"], "acme/zed", "{report}");

    let requests = s.requests();
    assert_eq!(requests.len(), 3, "the first request, then each concerned device alone: {requests:?}");
    assert_eq!(requests[0], names(&[BOX_A, PAD, ZED]));
    assert_eq!(sorted(&requests[1..]), sorted(&[names(&[BOX_A]), names(&[PAD])]), "{requests:?}");
}
