use super::*;
use crate::recognition::IDENTITY_REQUEST;

mod consent;

#[test]
fn an_input_is_paired_with_its_device_output() {
    let outs = vec!["Midi Through Port-0".to_string(), "minilogue xd MIDI 2".to_string()];
    assert_eq!(output_for("minilogue xd MIDI 1", &outs).as_deref(), Some("minilogue xd MIDI 2"));
    assert_eq!(output_for("Midi Through Port-0", &outs).as_deref(), Some("Midi Through Port-0"));
    assert_eq!(output_for("TR-8S", &outs), None);
    assert_eq!(output_for("TR-8S", &outs[1..]).as_deref(), Some("minilogue xd MIDI 2"), "the only output");
}

const CARDS: &str = " 0 [PCH            ]: HDA-Intel - HDA Intel PCH
                      HDA Intel PCH at 0xf7e10000 irq 130
 1 [Minilab3       ]: USB-Audio - Minilab3
                      Arturia Minilab3 at usb-0000:00:14.0-2, full speed
 2 [Minilab3_1     ]: USB-Audio - Minilab3
                      Arturia Minilab3 at usb-0000:00:14.0-3, full speed
 3 [Pad            ]: USB-Audio - Pad
                      Acme Pad at usb-0000:00:14.0-4, full speed
";

fn cards() -> Vec<(String, u16, u16)> {
    usb_cards(CARDS, &[(1, "1c75:0288\n"), (2, "1c75:0288\n"), (3, "1234:5678\n")])
}

#[test]
fn usb_cards_lists_the_short_name_and_ids_of_each_usb_card() {
    assert_eq!(cards(), vec![
        ("Minilab3".to_string(), 0x1c75, 0x0288),
        ("Minilab3".to_string(), 0x1c75, 0x0288),
        ("Pad".to_string(), 0x1234, 0x5678),
    ]);
}

#[test]
fn usb_cards_leaves_out_a_card_without_usbid_and_ignores_unreadable_lines() {
    assert_eq!(usb_cards(CARDS, &[]), vec![], "no usbid file: not USB");
    let only_pad = usb_cards(CARDS, &[(3, "1234:5678")]);
    assert_eq!(only_pad, vec![("Pad".to_string(), 0x1234, 0x5678)]);
    assert_eq!(usb_cards("garbage\n\n [ : \n", &[(0, "zz")]), vec![]);
    assert_eq!(usb_cards(CARDS, &[(1, "not an id"), (3, "1234:5678")]), only_pad, "an unreadable usbid is skipped");
    assert_eq!(usb_cards("", &[]), vec![]);
}

#[test]
fn usb_cards_name_a_card_by_its_name_not_its_id() {
    let listing = " 2 [BCR32          ]: USB-Audio - Behringer BCR-32\n                      Behringer BCR-32 at usb-0000:00:14.0-1\n 5 [Foo            ]: USB-Audio - Foo - Bar - Baz\n 7 [Odd            ]: USB-Audio\n";
    let got = usb_cards(listing, &[(2, "1397:12cf"), (5, "0001:0002"), (7, "0003:0004")]);
    assert_eq!(got, vec![
        ("Behringer BCR-32".to_string(), 0x1397, 0x12cf),
        ("Foo - Bar - Baz".to_string(), 0x0001, 0x0002),
    ], "name = text after the first ' - '; a line without it is left out");
}

const BENCH: &str = " 2 [BCR32          ]: USB-Audio - Behringer BCR-32
                      Behringer Behringer BCR-32 at usb-0000:00:14.0-1, full speed
 3 [Controller     ]: USB-Audio - Electra Controller
                      Electra Electra Controller at usb-0000:00:14.0-2, full speed
 6 [Minilab3       ]: USB-Audio - Minilab3
                      Arturia Minilab3 at usb-0000:00:14.0-3, full speed
";

#[test]
fn every_usb_device_of_the_bench_listing_gets_its_ids_whatever_its_card_id() {
    let c = usb_cards(BENCH, &[(2, "1397:12cf\n"), (3, "1fc9:82cf\n"), (6, "1c75:020b\n")]);
    assert_eq!(usb_of_port("Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 24:0", &c), Some((0x1397, 0x12cf)));
    assert_eq!(usb_of_port("Electra Controller:Electra Controller Electra Port 28:0", &c), Some((0x1fc9, 0x82cf)));
    assert_eq!(usb_of_port("Minilab3:Minilab3 MIDI 40:0", &c), Some((0x1c75, 0x020b)));
    assert_eq!(usb_of_port("BCR32:BCR32 MIDI 1 24:0", &c), None, "the card id names nothing");
    assert_eq!(usb_of_port("Controller:x 1:0", &c), None, "the card id names nothing");
}

#[test]
fn cards_whose_names_squash_alike_share_one_name() {
    let listing = " 0 [A ]: USB-Audio - Mini Lab 3\n 1 [B ]: USB-Audio - MiniLab-3\n";
    let differ = usb_cards(listing, &[(0, "1:2"), (1, "3:4")]);
    assert_eq!(usb_of_port("Minilab3:Minilab3 MIDI 40:0", &differ), None);
    let same = usb_cards(listing, &[(0, "1:2"), (1, "1:2")]);
    assert_eq!(usb_of_port("Minilab3:Minilab3 MIDI 40:0", &same), Some((1, 2)));
}

#[test]
fn a_port_takes_the_ids_of_the_card_its_alsa_client_names() {
    let c = cards();
    assert_eq!(usb_of_port("Pad:Pad MIDI 1 28:0", &c), Some((0x1234, 0x5678)));
    assert_eq!(usb_of_port("pad:Pad MIDI 1 28:0", &c), Some((0x1234, 0x5678)), "squashed: case");
    assert_eq!(usb_of_port("P-a d:Pad MIDI 1 28:0", &c), Some((0x1234, 0x5678)), "squashed: punctuation");
}

#[test]
fn two_identical_devices_give_their_ports_the_shared_ids() {
    let c = cards();
    assert_eq!(usb_of_port("Minilab3:Minilab3 MIDI 1 24:0", &c), Some((0x1c75, 0x0288)));
    assert_eq!(usb_of_port("Minilab3:Minilab3 MIDI 1 32:0", &c), Some((0x1c75, 0x0288)));
}

#[test]
fn cards_of_one_name_with_different_ids_give_no_ids() {
    let c = vec![("Box".to_string(), 1, 2), ("Box".to_string(), 3, 4)];
    assert_eq!(usb_of_port("Box:Box MIDI 1 20:0", &c), None);
}

#[test]
fn a_port_that_matches_no_card_has_no_usb_ids() {
    let c = cards();
    assert_eq!(usb_of_port("Midi Through:Midi Through Port-0 14:0", &c), None);
    assert_eq!(usb_of_port("Minilab:Minilab MIDI 1 24:0", &c), None, "never guessed by likeness");
    assert_eq!(usb_of_port("Pad Pro:Pad Pro MIDI 1 24:0", &c), None);
    assert_eq!(usb_of_port("Pad", &[]), None);
    assert_eq!(usb_of_port("PCH:x 1:0", &c), None, "a card that is not USB");
}

#[test]
fn a_port_name_without_client_separator_is_read_whole() {
    assert_eq!(usb_of_port("Pad", &cards()), Some((0x1234, 0x5678)));
}

// ---- the learn mode: a fingerprint of a device that may not say who it is ----

/// A scripted unit: records what is sent to it (port, bytes), answers with what it was given.
struct Script { sent: Vec<(String, Vec<u8>)>, heard: Vec<Heard>, listened: Vec<Duration> }

impl Script {
    fn new(heard: Vec<Heard>) -> Self { Script { sent: vec![], heard, listened: vec![] } }
    fn sent_bytes(&self) -> Vec<Vec<u8>> { self.sent.iter().map(|(_, m)| m.clone()).collect() }
}

impl Wire for Script {
    fn send(&mut self, port: &str, msg: &[u8]) { self.sent.push((port.to_string(), msg.to_vec())); }
    fn heard(&mut self, listen: Duration) -> Vec<Heard> { self.listened.push(listen); self.heard.clone() }
}

const SYNTH: &str = "Test Synth:Test Synth MIDI 1 20:0";

fn at_on(port: &str, ms: u64, msg: &[u8]) -> Heard {
    Heard { port: port.to_string(), at: Duration::from_millis(ms), msg: msg.to_vec() }
}
fn at(ms: u64, msg: &[u8]) -> Heard { at_on(SYNTH, ms, msg) }

fn unit_of(inputs: &[&str], outputs: &[&str], usb: Option<(u16, u16)>) -> Unit {
    Unit { in_indexes: (0..inputs.len()).collect(), out_indexes: (0..outputs.len()).collect(),
           inputs: inputs.iter().map(|s| s.to_string()).collect(),
           outputs: outputs.iter().map(|s| s.to_string()).collect(), usb, client_kind: ClientKind::Unknown }
}

fn device(usb: Option<(u16, u16)>) -> Unit { unit_of(&[SYNTH], &[SYNTH], usb) }

const REPLY: [u8; 17] = [0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x00, 0x20, 0x6B, 0x02, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0xF7];

fn learn_on(heard: Vec<Heard>, usb: Option<(u16, u16)>) -> (Learned, Script) {
    let mut wire = Script::new(heard);
    let learned = learn(&device(usb), &mut wire, Duration::from_millis(5000), "2026-11-21");
    (learned, wire)
}

/// The fingerprint read back by `osd` as a 0.6 fingerprint: its schema's own check.
fn valid_fingerprint(fp: &serde_json::Value) {
    let text = fp.to_string();
    osd::read(&text, osd::Kind::Fingerprint).unwrap_or_else(|e| panic!("{e}\n{text}"));
}

fn synth_heard() -> Vec<Heard> {
    let mut h = vec![at(12, &REPLY)];
    for i in 0..5 { h.push(at(100 + i * 20, &[0xF8])); }
    for i in 0..3 { h.push(at(200 + i * 300, &[0xFE])); }
    h.push(at(900, &[0xB0, 74, 10]));
    h.push(at(910, &[0xB0, 74, 99]));
    h.push(at(920, &[0xB1, 7, 5]));
    h.push(at(930, &[0x90, 60, 100]));
    h.push(at(940, &[0x90, 60, 0]));
    h
}

#[test]
fn learn_sends_only_the_universal_identity_request_and_listens_for_the_time_given() {
    let (_, wire) = learn_on(synth_heard(), None);
    assert_eq!(wire.sent_bytes(), vec![IDENTITY_REQUEST.to_vec()]);
    assert_eq!(wire.listened, vec![Duration::from_millis(5000)]);
}

#[test]
fn learn_sends_the_identity_request_even_to_a_silent_device() {
    let (_, wire) = learn_on(vec![], None);
    assert_eq!(wire.sent_bytes(), vec![IDENTITY_REQUEST.to_vec()]);
}

#[test]
fn learn_returns_a_valid_fingerprint_of_a_test_synth_with_its_usb_ports_and_identity() {
    let (l, _) = learn_on(synth_heard(), Some((0x1234, 0x5678)));
    valid_fingerprint(&l.fingerprint);
    let fp = &l.fingerprint;
    assert_eq!(fp["$schema"], "https://w3id.org/osd/0.6/fingerprint.schema.json");
    assert_eq!(fp["observed"], "2026-11-21");
    assert_eq!(fp["via"], "native");
    assert_eq!(fp["usb"], serde_json::json!({ "vendor_id": 0x1234, "product_id": 0x5678 }));
    assert_eq!(fp["ports"]["inputs"], serde_json::json!(["test synth midi 1"]));
    assert_eq!(fp["ports"]["outputs"], serde_json::json!(["test synth midi 1"]));
    assert_eq!(fp["identity_reply"], "F0 7E 7F 06 02 00 20 6B 02 00 04 00 01 00 04 00 F7");
}

#[test]
fn learn_records_clock_active_sensing_and_how_a_note_ends_in_sends() {
    let (l, _) = learn_on(synth_heard(), None);
    assert_eq!(l.fingerprint["sends"], serde_json::json!({
        "clock": true, "active_sensing": true, "note_off": "note_on_velocity_0" }));
    let (l, _) = learn_on(vec![at(5, &[0x80, 60, 0])], None);
    assert_eq!(l.fingerprint["sends"], serde_json::json!({ "note_off": "note_off" }));
    valid_fingerprint(&l.fingerprint);
}

#[test]
fn learn_puts_a_clock_heard_alone_in_sends_without_any_identity() {
    let (l, _) = learn_on(vec![at(10, &[0xF8]), at(30, &[0xF8])], None);
    assert_eq!(l.fingerprint["sends"], serde_json::json!({ "clock": true }));
    assert!(l.fingerprint.get("identity_reply").is_none());
    assert!(l.measured.get("identity_reply_ms").is_none());
    valid_fingerprint(&l.fingerprint);
}

#[test]
fn learn_puts_an_active_sensing_heard_alone_in_sends() {
    let (l, _) = learn_on(vec![at(10, &[0xFE])], None);
    assert_eq!(l.fingerprint["sends"], serde_json::json!({ "active_sensing": true }));
}

#[test]
fn learn_on_a_silent_device_gives_only_what_is_known_never_a_guess() {
    let (l, _) = learn_on(vec![], None);
    valid_fingerprint(&l.fingerprint);
    let fp = l.fingerprint.as_object().unwrap();
    for absent in ["identity_reply", "sends", "controls", "usb", "maker_replies", "device", "names"] {
        assert!(!fp.contains_key(absent), "{absent} guessed: {fp:?}");
    }
    assert!(fp.contains_key("ports"));
    assert_eq!(l.measured, serde_json::json!({}));
}

#[test]
fn learn_keeps_the_measures_beside_the_fingerprint_never_inside() {
    let (l, _) = learn_on(synth_heard(), None);
    let fp = l.fingerprint.as_object().unwrap();
    assert!(!fp.contains_key("measured"));
    assert_eq!(l.measured["identity_reply_ms"].as_f64(), Some(12.0));
    assert_eq!(l.measured["periods_ms"]["clock"].as_f64(), Some(20.0));
    assert_eq!(l.measured["periods_ms"]["active_sensing"].as_f64(), Some(300.0));
}

#[test]
fn learn_gives_no_period_for_a_message_heard_once() {
    let (l, _) = learn_on(vec![at(10, &[0xF8]), at(50, &[0xFE])], None);
    assert!(l.measured.get("periods_ms").is_none(), "{:?}", l.measured);
}

#[test]
fn learn_controls_have_type_channel_and_number_without_value_once_each() {
    let (l, _) = learn_on(synth_heard(), None);
    assert_eq!(l.fingerprint["controls"], serde_json::json!([
        { "cc": 74, "channel": 0 }, { "cc": 7, "channel": 1 }]));
    valid_fingerprint(&l.fingerprint);
}

#[test]
fn learn_controls_name_a_parameter_by_its_nrpn_selectors() {
    let h = vec![at(1, &[0xB2, 99, 1]), at(2, &[0xB2, 98, 2]), at(3, &[0xB2, 6, 64]), at(4, &[0xB2, 38, 3])];
    let (l, _) = learn_on(h, None);
    assert_eq!(l.fingerprint["controls"], serde_json::json!([{ "nrpn_msb": 1, "nrpn_lsb": 2, "channel": 2 }]));
    valid_fingerprint(&l.fingerprint);
}

#[test]
fn learn_controls_keep_32_distinct_in_arrival_order_and_drop_the_33rd() {
    let mut h: Vec<Heard> = (1..=33u8).map(|cc| at(cc as u64, &[0xB0, cc, 1])).collect();
    h.push(at(40, &[0xB0, 1, 2]));
    let (l, _) = learn_on(h, None);
    let controls = l.fingerprint["controls"].as_array().unwrap();
    assert_eq!(controls.len(), 32);
    assert_eq!(controls[0]["cc"], 1);
    assert_eq!(controls[31]["cc"], 32);
    assert!(controls.iter().all(|c| c["cc"] != 33));
    valid_fingerprint(&l.fingerprint);
}

#[test]
fn learn_controls_count_a_control_moved_twice_once() {
    let (l, _) = learn_on(vec![at(1, &[0xB0, 1, 5]), at(2, &[0xB0, 1, 6])], None);
    assert_eq!(l.fingerprint["controls"].as_array().unwrap().len(), 1);
}

#[test]
fn learn_listen_time_defaults_to_5000_ms_and_accepts_1000_to_30000() {
    assert_eq!(learn_listen(None), Ok(Duration::from_millis(5000)));
    assert_eq!(learn_listen(Some(1000)), Ok(Duration::from_millis(1000)));
    assert_eq!(learn_listen(Some(30000)), Ok(Duration::from_millis(30000)));
}

#[test]
fn learn_listen_time_outside_the_interval_is_refused_naming_it() {
    for bad in [0, 999, 30001, 100000] {
        let e = learn_listen(Some(bad)).unwrap_err();
        assert!(e.contains("1000") && e.contains("30000"), "{e}");
    }
}

#[test]
fn learn_notice_tells_the_person_the_time_and_to_move_the_controls() {
    let n = learn_notice(Duration::from_millis(5000));
    assert!(n.contains("5 seconds"), "{n}");
    assert!(n.contains("move"), "{n}");
    assert!(learn_notice(Duration::from_millis(12000)).contains("12 seconds"));
}

// ---- the words of a port name -------------------------------------------------

#[test]
fn port_words_keep_what_follows_the_first_colon_in_lower_case_without_the_final_numbers() {
    assert_eq!(port_words("Minilab3:Minilab3 MIDI 40:0"), "minilab3 midi");
    assert_eq!(port_words("Minilab3:Minilab3 MIDI 7:12"), "minilab3 midi");
}

#[test]
fn port_words_cut_between_words_on_any_punctuation() {
    assert_eq!(port_words("Minilab3:Minilab3 MCU/HUI 40:2"), "minilab3 mcu hui");
    assert_eq!(port_words("Midi Through:Midi Through Port-0 14:0"), "midi through port 0");
    assert_eq!(port_words("Box:Box_In  (A)   9:1"), "box in a");
}

#[test]
fn port_words_keep_the_digits_of_a_word() {
    assert_eq!(port_words("Minilab3:Minilab3 DIN THRU 40:1"), "minilab3 din thru");
    assert_eq!(port_words("Behringer BCR-32:Behringer BCR-32 BCR-32 ZAQ 24:1"), "behringer bcr 32 zaq");
}

#[test]
fn port_words_write_a_repeated_run_of_words_once() {
    assert_eq!(port_words("Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 24:0"), "behringer bcr 32 ctrl");
    assert_eq!(port_words("Electra Controller:Electra Controller Electra Port 28:0"), "electra controller electra port");
    assert_eq!(port_words("Electra Controller:Electra Controller Electra CTRL 28:2"), "electra controller electra ctrl");
    assert_eq!(port_words("A:Beat Beat 3:0"), "beat");
}

#[test]
fn port_words_of_a_name_without_colon_or_numbers_are_the_whole_name_lower_cased() {
    assert_eq!(port_words("USB MIDI Interface"), "usb midi interface");
}

// ---- one unit per device ------------------------------------------------------

fn capture_of(ports: &[&str], cards: &str) -> Capture {
    let v = serde_json::json!({ "format_version": crate::capture::FORMAT_VERSION,
        "outputs": ports, "inputs": ports, "cards": cards, "usbids": [], "clients": "", "requests": [], "received": [] });
    Capture::parse(&v.to_string()).unwrap()
}

#[test]
fn units_group_the_ports_of_one_alsa_client_number_into_one_device() {
    let c = capture_of(&["Minilab3:Minilab3 MIDI 40:0", "Minilab3:Minilab3 DIN THRU 40:1",
        "Minilab3:Minilab3 MCU/HUI 40:2", "Minilab3:Minilab3 ALV 40:3", "Other:Other 41:0"], "");
    let u = units(&c);
    assert_eq!(u.len(), 2, "{u:?}");
    assert_eq!(u[0].inputs.len(), 4);
    assert_eq!(u[0].outputs.len(), 4);
    assert_eq!(u[0].in_indexes, vec![0, 1, 2, 3]);
    assert_eq!(u[1].inputs, vec!["Other:Other 41:0".to_string()]);
}

#[test]
fn units_keep_two_devices_of_the_same_client_name_and_different_numbers_apart() {
    let c = capture_of(&["Pad:Pad 20:0", "Pad:Pad 21:0"], "");
    assert_eq!(units(&c).len(), 2);
}

#[test]
fn units_join_two_ports_of_one_client_number_even_when_their_client_names_differ() {
    let c = capture_of(&["Alpha:Alpha MIDI 30:0", "Beta:Beta MIDI 30:1"], "");
    let u = units(&c);
    assert_eq!(u.len(), 1, "{u:?}");
    assert_eq!(u[0].inputs.len(), 2);
}

#[test]
fn units_group_by_client_name_when_the_names_carry_no_number() {
    let c = capture_of(&["Synth:Synth In", "Synth:Synth Thru", "Drum:Drum In"], "");
    let u = units(&c);
    assert_eq!(u.len(), 2, "{u:?}");
    assert_eq!(u[0].inputs.len(), 2);
}

#[test]
fn units_keep_an_input_only_port_and_an_output_only_port_in_their_device() {
    let v = serde_json::json!({ "format_version": crate::capture::FORMAT_VERSION,
        "outputs": ["Box:Box Out 50:0"], "inputs": ["Box:Box In 50:1", "Pad:Pad 51:0"],
        "cards": "", "usbids": [], "clients": "", "requests": [], "received": [] });
    let u = units(&Capture::parse(&v.to_string()).unwrap());
    assert_eq!(u.len(), 2);
    assert_eq!((u[0].inputs.len(), u[0].outputs.len()), (1, 1));
    assert_eq!((u[1].inputs.len(), u[1].outputs.len()), (1, 0));
}

#[test]
fn units_read_the_usb_ids_of_the_card_of_the_device() {
    let v = serde_json::json!({ "format_version": crate::capture::FORMAT_VERSION,
        "outputs": ["Minilab3:Minilab3 MIDI 40:0", "Minilab3:Minilab3 ALV 40:3"],
        "inputs": ["Minilab3:Minilab3 MIDI 40:0", "Minilab3:Minilab3 ALV 40:3"],
        "cards": " 6 [Minilab3       ]: USB-Audio - Minilab3\n",
        "usbids": [{"card": 6, "text": "1c75:020b\n"}], "clients": "", "requests": [], "received": [] });
    let u = units(&Capture::parse(&v.to_string()).unwrap());
    assert_eq!(u[0].usb, Some((0x1c75, 0x020b)));
}

// ---- one fingerprint per device -----------------------------------------------

const MINILAB: [&str; 4] = ["Minilab3:Minilab3 MIDI 40:0", "Minilab3:Minilab3 DIN THRU 40:1",
    "Minilab3:Minilab3 MCU/HUI 40:2", "Minilab3:Minilab3 ALV 40:3"];

#[test]
fn learn_writes_the_words_of_every_port_of_the_device_in_the_order_of_the_listing_without_the_machine_numbers() {
    let u = unit_of(&MINILAB, &MINILAB, None);
    let l = learn(&u, &mut Script::new(vec![]), Duration::from_millis(1000), "2026-11-21");
    valid_fingerprint(&l.fingerprint);
    let words = serde_json::json!(["minilab3 midi", "minilab3 din thru", "minilab3 mcu hui", "minilab3 alv"]);
    assert_eq!(l.fingerprint["ports"]["inputs"], words);
    assert_eq!(l.fingerprint["ports"]["outputs"], words);
}

#[test]
fn learn_writes_one_entry_per_port_when_ports_share_their_words() {
    let u = unit_of(&["Pad:Pad 9:0", "Pad:Pad 9:1"], &["Pad:Pad 9:0"], None);
    let l = learn(&u, &mut Script::new(vec![]), Duration::from_millis(1000), "2026-11-21");
    assert_eq!(l.fingerprint["ports"]["inputs"], serde_json::json!(["pad", "pad"]));
    assert_eq!(l.fingerprint["ports"]["outputs"], serde_json::json!(["pad"]));
}

#[test]
fn learn_writes_two_entries_for_the_two_ports_of_the_osmose() {
    let p = ["Osmose:Osmose  48:0", "Osmose:Osmose  48:1"];
    let l = learn(&unit_of(&p, &p, Some((0x2e14, 0x0010))), &mut Script::new(vec![]), Duration::from_millis(1000), "2026-11-21");
    valid_fingerprint(&l.fingerprint);
    assert_eq!(l.fingerprint["ports"]["inputs"], serde_json::json!(["osmose", "osmose"]));
    assert_eq!(l.fingerprint["ports"]["outputs"], serde_json::json!(["osmose", "osmose"]));
}

#[test]
fn learn_writes_one_entry_for_the_single_port_of_the_deluge() {
    let p = ["Deluge:Deluge OUT 32:0"];
    let l = learn(&unit_of(&p, &p, Some((0x16d0, 0x0ce2))), &mut Script::new(vec![]), Duration::from_millis(1000), "2026-11-21");
    valid_fingerprint(&l.fingerprint);
    assert_eq!(l.fingerprint["ports"]["inputs"].as_array().unwrap().len(), 1);
    assert_eq!(l.fingerprint["ports"]["outputs"].as_array().unwrap().len(), 1);
}

#[test]
fn learn_writes_three_entries_for_the_three_ports_of_the_electra_one() {
    let p = ["Electra Controller:Electra Controller Electra Port 28:0", "Electra Controller:Electra Controller Electra Port 28:1",
        "Electra Controller:Electra Controller Electra CTRL 28:2"];
    let l = learn(&unit_of(&p, &p, None), &mut Script::new(vec![]), Duration::from_millis(1000), "2026-11-21");
    let words = serde_json::json!(["electra controller electra port", "electra controller electra port", "electra controller electra ctrl"]);
    assert_eq!(l.fingerprint["ports"]["inputs"], words);
    assert_eq!(l.fingerprint["ports"]["outputs"], words);
}

#[test]
fn learn_leaves_out_the_outputs_key_of_a_device_without_output_and_the_inputs_key_of_one_without_input() {
    let l = learn(&unit_of(&["Pad:Pad 9:0"], &[], None), &mut Script::new(vec![]), Duration::from_millis(1000), "2026-11-21");
    valid_fingerprint(&l.fingerprint);
    assert!(l.fingerprint["ports"].get("outputs").is_none(), "{}", l.fingerprint);
    assert_eq!(l.fingerprint["ports"]["inputs"], serde_json::json!(["pad"]));
    let l = learn(&unit_of(&[], &["Pad:Pad 9:0"], None), &mut Script::new(vec![]), Duration::from_millis(1000), "2026-11-21");
    valid_fingerprint(&l.fingerprint);
    assert!(l.fingerprint["ports"].get("inputs").is_none(), "{}", l.fingerprint);
    assert_eq!(l.fingerprint["ports"]["outputs"], serde_json::json!(["pad"]));
}

#[test]
fn learn_sends_the_identity_request_on_every_output_of_the_device() {
    let u = unit_of(&MINILAB, &MINILAB, None);
    let mut wire = Script::new(vec![]);
    learn(&u, &mut wire, Duration::from_millis(1000), "2026-11-21");
    let ports: Vec<&str> = wire.sent.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(ports, MINILAB.to_vec());
    assert!(wire.sent.iter().all(|(_, m)| m == &IDENTITY_REQUEST.to_vec()));
    assert_eq!(wire.listened.len(), 1, "one listening for the whole device");
}

#[test]
fn learn_puts_what_any_input_of_the_device_sent_in_its_one_fingerprint() {
    let u = unit_of(&MINILAB, &MINILAB, None);
    let heard = vec![
        at_on(MINILAB[2], 12, &REPLY),
        at_on(MINILAB[0], 100, &[0xF8]), at_on(MINILAB[0], 120, &[0xF8]),
        at_on(MINILAB[1], 150, &[0xFE]),
        at_on(MINILAB[3], 200, &[0xB0, 74, 10]),
        at_on(MINILAB[0], 210, &[0x90, 60, 0]),
    ];
    let l = learn(&u, &mut Script::new(heard), Duration::from_millis(1000), "2026-11-21");
    valid_fingerprint(&l.fingerprint);
    assert_eq!(l.fingerprint["identity_reply"], "F0 7E 7F 06 02 00 20 6B 02 00 04 00 01 00 04 00 F7");
    assert_eq!(l.fingerprint["sends"], serde_json::json!({ "clock": true, "active_sensing": true, "note_off": "note_on_velocity_0" }));
    assert_eq!(l.fingerprint["controls"], serde_json::json!([{ "cc": 74, "channel": 0 }]));
}

#[test]
fn learn_measures_name_the_port_where_the_reply_and_each_period_were_measured() {
    let u = unit_of(&MINILAB, &MINILAB, None);
    let heard = vec![
        at_on(MINILAB[2], 12, &REPLY),
        at_on(MINILAB[0], 100, &[0xF8]), at_on(MINILAB[0], 120, &[0xF8]),
        at_on(MINILAB[1], 200, &[0xFE]), at_on(MINILAB[1], 500, &[0xFE]),
    ];
    let l = learn(&u, &mut Script::new(heard), Duration::from_millis(1000), "2026-11-21");
    assert_eq!(l.measured["identity_reply_ms"].as_f64(), Some(12.0));
    assert_eq!(l.measured["identity_reply_port"], MINILAB[2]);
    assert_eq!(l.measured["periods_ms"]["clock"].as_f64(), Some(20.0));
    assert_eq!(l.measured["periods_ms"]["active_sensing"].as_f64(), Some(300.0));
    assert_eq!(l.measured["periods_port"], serde_json::json!({ "clock": MINILAB[0], "active_sensing": MINILAB[1] }));
}

#[test]
fn learn_measures_a_period_on_one_port_never_across_two_ports() {
    let u = unit_of(&MINILAB, &MINILAB, None);
    let heard = vec![at_on(MINILAB[0], 100, &[0xF8]), at_on(MINILAB[1], 120, &[0xF8])];
    let l = learn(&u, &mut Script::new(heard), Duration::from_millis(1000), "2026-11-21");
    assert!(l.measured.get("periods_ms").is_none(), "{:?}", l.measured);
}

// ---- the kind of the client of a port, read from the list of clients ----

const FLUID_PORT: &str = "FLUID Synth (2267):Synth input port (2267:0) 128:0";

fn clients_with(entry_name: &str, tail: &str) -> String {
    format!("Client info\n  cur  clients : 2\n\nClient   0 : \"System\" [Kernel]\n  Port   0 : \"Timer\" (Rwe-)\n\
             Client 128 : \"{entry_name}\" {tail}\n  Port   0 : \"Synth input port (2267:0)\" (RWe-)\n")
}

#[test]
fn a_client_whose_first_word_in_brackets_is_user_is_a_program_whatever_follows_or_the_case() {
    for tail in ["[User Legacy]", "[User]", "[user]", "[USER Legacy]", "[User  Legacy extra]"] {
        assert_eq!(client_kind(&clients_with("FLUID Synth (2267)", tail), FLUID_PORT), ClientKind::User, "{tail}");
    }
}

#[test]
fn a_client_whose_first_word_in_brackets_is_kernel_is_a_device_of_the_system() {
    for tail in ["[Kernel]", "[Kernel Legacy]", "[kernel]", "[KERNEL Legacy]"] {
        assert_eq!(client_kind(&clients_with("FLUID Synth (2267)", tail), FLUID_PORT), ClientKind::Kernel, "{tail}");
    }
}

#[test]
fn a_client_whose_first_word_in_brackets_is_another_word_has_the_kind_unknown() {
    for tail in ["[Other Legacy]", "[Usermode]", "[Kernelish]", "[]", "[ ]", "no brackets"] {
        assert_eq!(client_kind(&clients_with("FLUID Synth (2267)", tail), FLUID_PORT), ClientKind::Unknown, "{tail}");
    }
}

#[test]
fn the_port_name_loses_one_final_number_in_parentheses_to_meet_the_name_of_its_client() {
    let list = clients_with("FLUID Synth", "[User Legacy]");
    assert_eq!(client_kind(&list, FLUID_PORT), ClientKind::User);
    assert_eq!(client_kind(&list, "FLUID Synth:Synth input port 128:0"), ClientKind::User, "an exact name meets as before");
    let kernel = clients_with("Minilab3", "[Kernel]");
    assert_eq!(client_kind(&kernel, "Minilab3 (7):Minilab3 MIDI 128:0"), ClientKind::Kernel);
}

#[test]
fn any_other_gap_between_the_port_name_and_the_name_of_the_client_has_the_kind_unknown() {
    let list = clients_with("FLUID Synth", "[User Legacy]");
    for port in ["FLUID Synth (2267) (1):Synth 128:0", "FLUID Synth (abc):Synth 128:0", "FLUID Synth ():Synth 128:0",
                 "FLUID Synth (22a7):Synth 128:0", "FLUID Synth 2267:Synth 128:0", "FLUID Other (2267):Synth 128:0"] {
        assert_eq!(client_kind(&list, port), ClientKind::Unknown, "{port}");
    }
    // The name of the entry is never stripped: only the port's.
    assert_eq!(client_kind(&clients_with("FLUID Synth (2267)", "[User Legacy]"), "FLUID Synth:Synth 128:0"), ClientKind::Unknown);
}

#[test]
fn a_port_with_no_entry_for_its_client_number_or_two_entries_has_the_kind_unknown() {
    let list = clients_with("FLUID Synth", "[User Legacy]");
    assert_eq!(client_kind(&list, "FLUID Synth (2267):Synth input port 129:0"), ClientKind::Unknown);
    assert_eq!(client_kind("", FLUID_PORT), ClientKind::Unknown);
    let twice = format!("{list}Client 128 : \"FLUID Synth (2267)\" [Kernel]\n");
    assert_eq!(client_kind(&twice, FLUID_PORT), ClientKind::Unknown);
}
