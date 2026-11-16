use super::*;
use crate::device::full_osc_path;

fn load(id: &str) -> osd::Device {
    crate::device::load(format!("tests/fixtures/devices/{id}.json")).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn node<'a>(ns: &'a Value, path: &str) -> &'a Value {
    path.split('/').filter(|s| !s.is_empty()).fold(ns, |n, seg| &n["CONTENTS"][seg])
}

#[test]
fn a_driver_with_banks_lists_the_program_recall() {
    let dev = load("acme/box.vendor.fw-2");
    let ns = namespace(std::slice::from_ref(&dev));
    let n = node(&ns, "/box/program/recall");
    assert_eq!((n["TYPE"].as_str(), n["ACCESS"].as_i64()), (Some("si"), Some(2)));
    assert_eq!(n["RANGE"], json!([{ "VALS": ["User", "Factory"] }, { "MIN": 1, "MAX": 64 }]));
    assert_eq!(n["OVERLOADS"][0]["TYPE"], "sii", "a channel may follow");
    assert!(n["DESCRIPTION"].as_str().unwrap().contains("stored sound by bank"), "{n}");
    let plain = namespace(&[load("korg/minilogue")]);
    assert!(node(&plain, "/minilogue/program/recall").is_null(), "no banks, no recall");
    assert_eq!(node(&plain, "/minilogue/identity")["ACCESS"], 2, "every MIDI device answers /identity");
    let dump = node(&ns, "/box/program/dump");
    assert_eq!((dump["TYPE"].as_str(), dump["OVERLOADS"][0]["TYPE"].as_str()), (Some("si"), Some("sii")));
    assert_eq!(node(&ns, "/box/identity/reply")["TYPE"], "ss");
}

#[test]
fn a_cc_and_nrpn_synth_lists_its_controls_with_types_ranges_and_access() {
    let dev = load("novation/bass_station_ii");
    let ns = namespace(std::slice::from_ref(&dev));
    assert_eq!(ns["FULL_PATH"], "/");
    assert!(ns["DESCRIPTION"].as_str().unwrap().contains("CC-BY-SA-4.0"), "credit and licence at the root");
    assert_eq!(ns["SOURCES"]["sources"][0]["license"], "CC-BY-SA-4.0");
    let cc = dev.cc_params.as_ref().unwrap();
    for e in &cc.entries {
        let n = node(&ns, &full_osc_path(&dev, &e.osc));
        let range = e.range.unwrap_or([0, 127]);
        assert_eq!(n["FULL_PATH"], full_osc_path(&dev, &e.osc));
        assert_eq!(n["ACCESS"], 2, "{}: the bridge only sends it", e.osc);
        assert_eq!(n["TYPE"], "i");
        assert_eq!(n["RANGE"][0], json!({ "MIN": range[0], "MAX": range[1] }));
        assert_eq!(n["OVERLOADS"][0]["TYPE"], "ii", "a channel may follow");
    }
    let nrpn = cc.entries.iter().find(|e| e.nrpn_msb.is_some() && e.nrpn_lsb.is_some()).unwrap();
    let text = node(&ns, &full_osc_path(&dev, &nrpn.osc))["DESCRIPTION"].as_str().unwrap().to_string();
    assert!(text.contains(&format!("NRPN {}:{}", nrpn.nrpn_msb.unwrap(), nrpn.nrpn_lsb.unwrap())), "{text}");
    let on = node(&ns, &full_osc_path(&dev, "/note/on"));
    assert_eq!((on["TYPE"].as_str(), on["ACCESS"].as_i64()), (Some("ii"), Some(3)), "sent to, and its MIDI in sent back");
    assert_eq!(node(&ns, &full_osc_path(&dev, "/cc/74"))["TYPE"], "i");
    assert_eq!(node(&ns, "/bridge/status")["TYPE"], "");
}

#[test]
fn a_drum_machine_lists_its_triggers_and_a_firmware_credits_the_midi_it_shares() {
    let dev = load("elektron/model_samples");
    let ns = namespace(std::slice::from_ref(&dev));
    let t = &dev.triggers.as_ref().unwrap().entries[0];
    let n = node(&ns, &full_osc_path(&dev, &t.osc));
    assert_eq!((n["TYPE"].as_str(), n["ACCESS"].as_i64()), (Some("i"), Some(2)));
    assert_eq!(n["RANGE"][0], json!({ "MIN": 1, "MAX": 127 }), "velocity");
    assert!(n["DESCRIPTION"].as_str().unwrap().starts_with(&t.sound));

    let fw = load("arturia/microfreak.vendor.fw-microfreak-tools-0.1.0");
    let ns = namespace(std::slice::from_ref(&fw));
    let text = ns["DESCRIPTION"].as_str().unwrap();
    assert!(text.contains("Apache-2.0") && text.contains("MIDI shared from arturia/microfreak") && text.contains("CC-BY-SA-4.0"), "{text}");
    assert_eq!(ns["SOURCES"]["midi_of"], "arturia/microfreak");
    let e = &fw.cc_params.as_ref().unwrap().entries[0];
    assert_eq!(node(&ns, &full_osc_path(&fw, &e.osc))["ACCESS"], 2, "the shared routes under its own prefix");
}

#[test]
fn templates_are_expanded_and_what_the_bridge_sends_is_read_only() {
    let dev = load("bitwig/bitwig-studio.third-party-osc.fw-5");
    let ns = namespace(std::slice::from_ref(&dev));
    for n in 1..=8 {
        let vol = node(&ns, &full_osc_path(&dev, &format!("/track/{n}/volume")));
        assert_eq!(vol["ACCESS"], 3, "sent to, and relayed back: {vol}");
    }
    assert!(node(&ns, &full_osc_path(&dev, "/track/9/volume")).is_null());
    let synth = load("novation/bass_station_ii");
    let ns = namespace(std::slice::from_ref(&synth));
    if matches!(&synth.midi_in, Some(osd::MidiIn::Types(t)) if t.contains(&osd::MidiType::Cc)) {
        assert_eq!(node(&ns, &full_osc_path(&synth, "/cc/74"))["ACCESS"], 3);
    }
    assert_eq!(node(&ns, &full_osc_path(&synth, "/sysex/raw"))["ACCESS"], 1);
}

#[test]
fn requests_get_a_node_an_attribute_or_the_host() {
    let dev = load("novation/bass_station_ii");
    let ns = namespace(std::slice::from_ref(&dev));
    let host = host_info(std::slice::from_ref(&dev), "127.0.0.1:7777".parse().unwrap(), None);
    let on = full_osc_path(&dev, "/note/on");
    assert_eq!(answer(&ns, &host, "/").1.unwrap()["FULL_PATH"], "/");
    assert_eq!(answer(&ns, &host, &on).1.unwrap()["FULL_PATH"], on);
    assert_eq!(answer(&ns, &host, &format!("{on}?TYPE#x")).1.unwrap(), json!({ "TYPE": "ii" }));
    assert_eq!(answer(&ns, &host, &format!("{on}?CONTENTS")).0, 204);
    assert_eq!(answer(&ns, &host, &format!("{on}?VALUE")).0, 400);
    assert_eq!(answer(&ns, &host, "/nothing/here").0, 404);
    let h = answer(&ns, &host, "/anything?HOST_INFO").1.unwrap();
    assert_eq!((h["OSC_PORT"].as_u64(), h["OSC_IP"].as_str(), h["OSC_TRANSPORT"].as_str()),
               (Some(7777), Some("127.0.0.1"), Some("UDP")));
}

#[test]
fn the_listener_answers_over_http() {
    let dev = load("novation/bass_station_ii");
    let local = serve("127.0.0.1:0", std::slice::from_ref(&dev), "127.0.0.1:7777".parse().unwrap(), None).unwrap();
    let get = |target: &str| -> String {
        let mut s = TcpStream::connect(local).unwrap();
        write!(s, "GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    };
    let root = get("/");
    assert!(root.starts_with("HTTP/1.1 200") && root.contains("application/json"), "{root}");
    let body: Value = serde_json::from_str(root.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["FULL_PATH"], "/");
    assert!(get("/?HOST_INFO").contains("\"OSC_PORT\":7777"));
    assert!(get("/no/such/node").starts_with("HTTP/1.1 404"));
}
