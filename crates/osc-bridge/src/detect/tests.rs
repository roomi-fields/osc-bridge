use super::*;

mod consent;

fn cand(slug: &str, name: &str, vendor: &str, mfr: &[u8]) -> Candidate {
    Candidate { id: format!("acme/{slug}"), slug: slug.into(), name: name.into(), vendor: vendor.into(),
                file: format!("devices/{slug}.json"), tier: "pencilresearch".into(),
                manufacturer_id: mfr.to_vec(), identity: None, firmware: None, midi_of: None }
}

/// The `identity` block of a 0.5 driver, as `osd` reads it.
fn ident(block: &str) -> osd::Identity {
    let capabilities = if block.contains("\"script\"") { r#","capabilities":["lua"]"# } else { "" };
    let text = format!(r#"{{"$schema":"https://w3id.org/osd/0.5/device.schema.json","device":{{"name":"Box","osc_prefix":"/box"}}{capabilities},"identity":{block}}}"#);
    match osd::read(&text, osd::Kind::Device).unwrap_or_else(|e| panic!("{e}\n{text}")) {
        osd::Descriptor::Device(d) => d.identity.unwrap(),
        _ => unreachable!(),
    }
}

fn with_id(slug: &str, firmware: Option<&str>, id: &str) -> Candidate {
    Candidate { identity: Some(ident(id)), firmware: firmware.map(str::to_string), ..cand(slug, "Box", "Acme", &[]) }
}

#[test]
fn parses_three_byte_manufacturer_reply() {
    // Arturia, family 0x0002, model 0x0004, firmware 1.0.4.0
    let msg = [0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x00, 0x20, 0x6B, 0x02, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0xF7];
    let id = parse_identity_reply(&msg).unwrap();
    assert_eq!(id.manufacturer, vec![0x00, 0x20, 0x6B]);
    assert_eq!((id.family, id.model), (2, 4));
    assert_eq!(id.version, [1, 0, 4, 0]);
    assert_eq!(parse_identity_reply(&id.reply()), Some(id.clone()), "written back as it was read");
    assert_eq!(manufacturer_name(&id.manufacturer), Some("Arturia"));
}

#[test]
fn parses_one_byte_manufacturer_reply() {
    let msg = [0xF0, 0x7E, 0x10, 0x06, 0x02, 0x41, 0x31, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0xF7];
    let id = parse_identity_reply(&msg).unwrap();
    assert_eq!(id.manufacturer, vec![0x41]);
    assert_eq!(id.family, 0x31 | (0x02 << 7));
}

#[test]
fn rejects_non_identity_and_truncated_messages() {
    assert!(parse_identity_reply(&IDENTITY_REQUEST).is_none());
    assert!(parse_identity_reply(&[0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x41, 0x01, 0xF7]).is_none());
    assert!(parse_identity_reply(&[0x90, 60, 100]).is_none());
}

#[test]
fn port_name_alone_finds_the_device() {
    let cat = vec![
        cand("minilab3", "MiniLab 3", "Arturia", &[]),
        cand("keystep", "KeyStep", "Arturia", &[]),
        cand("tr_8s", "TR-8S", "Roland", &[]),
    ];
    let best = rank("Minilab3 MIDI", None, &[], &cat, 3);
    assert_eq!(cat[best[0].index].slug, "minilab3");
    let best = rank("TR-8S", None, &[], &cat, 3);
    assert_eq!(cat[best[0].index].slug, "tr_8s");
}

#[test]
fn identity_breaks_ties_between_same_named_models() {
    let cat = vec![
        cand("ms_20_korg", "MS-20", "Korg", &[]),
        cand("ms_20_arturia", "MS-20", "Arturia", &[]),
    ];
    let korg = Identity { manufacturer: vec![0x42], family: 0, model: 0, version: [0; 4] };
    let best = rank("MS-20 mini", None, &[korg.reply()], &cat, 2);
    assert_eq!(cat[best[0].index].slug, "ms_20_korg");
}

#[test]
fn an_input_is_paired_with_its_device_output() {
    let outs = vec!["Midi Through Port-0".to_string(), "minilogue xd MIDI 2".to_string()];
    assert_eq!(output_for("minilogue xd MIDI 1", &outs).as_deref(), Some("minilogue xd MIDI 2"));
    assert_eq!(output_for("Midi Through Port-0", &outs).as_deref(), Some("Midi Through Port-0"));
    assert_eq!(output_for("TR-8S", &outs), None);
    assert_eq!(output_for("TR-8S", &outs[1..]).as_deref(), Some("minilogue xd MIDI 2"), "the only output");
}

#[test]
fn unrelated_port_matches_nothing() {
    let cat = vec![
        cand("minilab3", "MiniLab 3", "Arturia", &[]),
        cand("pcm_80_v1_14", "PCM 80 v1.14", "Lexicon", &[]),
    ];
    assert!(rank("Midi Through:Midi Through Port-0 14:0", None, &[], &cat, 3).is_empty());
}

// A Roland reply: device id 10h, family 59 01, member 00 00, revision 00 03 00 00.
const ROLAND: [u8; 15] = [0xF0, 0x7E, 0x10, 0x06, 0x02, 0x41, 0x59, 0x01, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0xF7];

#[test]
fn an_identity_reply_finds_the_device_behind_a_nameless_port() {
    let cat = vec![
        with_id("mc909", None, r#"{"reply":[240,126,{"device_id":true},6,2,65,89,1,0,0],"firmware":{"decode":"raw_bytes","at":[10,11,12,13]}}"#),
        with_id("mc808", None, r#"{"reply":[240,126,{"device_id":true},6,2,65,20,2,0,0]}"#),
        cand("tr_8s", "TR-8S", "Roland", &[0x41]),
    ];
    let best = rank("USB MIDI Interface", None, &[ROLAND.to_vec()], &cat, 3);
    assert_eq!(best.len(), 1, "only the recognised driver: {best:?}");
    assert_eq!(cat[best[0].index].slug, "mc909");
    assert_eq!(best[0].recognised.as_ref().unwrap().firmware.as_deref(), Some("0.3.0.0"));
    assert!(rank("USB MIDI Interface", None, &[], &cat, 3).is_empty(), "no reply, nothing to go by");
}

#[test]
fn the_firmware_in_the_reply_picks_the_variant() {
    let id = r#"{"reply":[240,126,{"device_id":true},6,2,65,89,1,0,0],"firmware":{"decode":"raw_bytes","at":[10,11,12,13]}}"#;
    let cat = vec![with_id("old", Some("0.2"), id), with_id("new", Some("0.3"), id), with_id("any", None, id)];
    let order: Vec<&str> = rank("x", None, &[ROLAND.to_vec()], &cat, 3).iter().map(|r| cat[r.index].slug.as_str()).collect();
    assert_eq!(order, ["new", "any", "old"], "the same version first, another version last");
}

#[test]
fn of_equal_matches_the_driver_the_others_build_on_comes_first() {
    let id = r#"{"reply":[240,126,{"device_id":true},6,2,65,89,1,0,0]}"#;
    let fork = Candidate { midi_of: Some("acme/zbase".into()), ..with_id("afork", None, id) };
    let cat = vec![fork, with_id("zbase", None, id), cand("other", "Other", "Acme", &[])];
    let best = rank("USB MIDI Interface", None, &[ROLAND.to_vec()], &cat, 3);
    let order: Vec<&str> = best.iter().map(|r| cat[r.index].slug.as_str()).collect();
    assert_eq!(order, ["zbase", "afork"], "the shared MIDI first, though its id sorts last");
    assert_eq!((best[0].equally.as_slice(), best[1].equally.as_slice()), (&[0][..], &[1][..]), "each says the other matches too");
    let alone = rank("USB MIDI Interface", None, &[ROLAND.to_vec()], &cat[1..], 3);
    assert!(alone[0].equally.is_empty(), "no other match, nothing to say");
}

#[test]
fn a_maker_request_and_its_text_tell_a_firmware_apart() {
    let stock = with_id("stock", None, r#"{"reply":[240,126,{"device_id":true},6,2,0,32,60,2,0]}"#);
    let fork = with_id("mdx", None, r#"{"request":[240,0,32,60,2,0,112,48,247],"reply":[240,0,32,60,2,0,114,48]}"#);
    let named = with_id("felucca", None, r#"{"request":[240,125,70,76,1,247],"to":"named_ports","ports":["FM-1"],
        "reply":[240,125,70,76],"reply_text":"Dinghy","firmware":{"decode":"ascii","at":[5]}}"#);
    let cat = vec![stock, fork, named];
    let uir = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x00, 0x20, 0x3C, 0x02, 0x00, 0x00, 0x00, 1, 2, 3, 4, 0xF7].to_vec();
    let mdx = [0xF0, 0x00, 0x20, 0x3C, 0x02, 0x00, 0x72, 0x30, 1, 0, 0, 0, 0xF7].to_vec();
    let best = rank("x", None, &[uir.clone(), mdx], &cat, 3);
    assert_eq!(cat[best[0].index].slug, "mdx", "{best:?}");
    assert_eq!(cat[rank("x", None, &[uir], &cat, 3)[0].index].slug, "stock");

    let info = [&[0xF0, 0x7D, 0x46, 0x4C, 0x02][..], b"Dinghy 0.4", &[0xF7]].concat();
    let r = recognise(cat[2].identity.as_ref().unwrap(), "FM-1 Midi", None, std::slice::from_ref(&info)).unwrap();
    assert!(r.by_reply && r.by_port && r.own_request);
    assert_eq!(r.firmware.as_deref(), Some("Dinghy 0.4"));
    let other = [&[0xF0, 0x7D, 0x46, 0x4C, 0x02][..], b"Grimm", &[0xF7]].concat();
    assert!(recognise(cat[2].identity.as_ref().unwrap(), "x", None, &[other]).is_none(), "its text is not there");

    let all = probes(cat.iter().filter_map(|c| c.identity.as_ref().map(|i| (c.slug.as_str(), i)))).probes;
    assert_eq!(all.len(), 2, "the universal request is not a probe: {all:?}");
    let plan = send_plan(&[("FM-1 Midi".to_string(), None), ("Digitakt".to_string(), None)], &all, &[]);
    for sends in &plan {
        assert_eq!(sends.sent, vec![IDENTITY_REQUEST.to_vec()], "{}: no maker request without consent", sends.port);
        assert_eq!(sends.held.len(), 2, "{}", sends.port);
    }
}

#[test]
fn a_maker_request_that_is_one_short_sysex_message_has_no_fault() {
    assert_eq!(maker_request_fault(&[0xF0, 0x7D, 0x46, 0x4C, 0x01, 0xF7]), None);
    assert_eq!(maker_request_fault(&[0xF0, 0xF7]), None, "empty body");
    let mut at_max = vec![0xF0];
    at_max.extend(std::iter::repeat_n(0x01, 30));
    at_max.push(0xF7);
    assert_eq!(at_max.len(), 32);
    assert_eq!(maker_request_fault(&at_max), None, "32 bytes is the bound");
}

#[test]
fn a_maker_request_that_is_not_one_sysex_message_of_at_most_32_bytes_is_refused_with_its_bytes_told() {
    let mut too_long = vec![0xF0];
    too_long.extend(std::iter::repeat_n(0x01, 31));
    too_long.push(0xF7);
    assert_eq!(too_long.len(), 33);
    let cases: [(&[u8], &str); 7] = [
        (&[], "empty"),
        (&[0xF0], "a lone F0"),
        (&[0x7D, 0x01, 0xF7], "no F0"),
        (&[0xF0, 0x7D, 0x01], "no F7"),
        (&[0xF0, 0x7D, 0x90, 0x3C, 0x7F, 0xF7], "a note-on status byte inside"),
        (&[0xF0, 0x7D, 0xF7, 0xF0, 0x7D, 0xF7], "two messages in one"),
        (&too_long, "33 bytes"),
    ];
    for (bytes, why) in cases {
        let fault = maker_request_fault(bytes).unwrap_or_else(|| panic!("{why} must be refused"));
        assert!(!fault.is_empty(), "{why}");
    }
    let fault = maker_request_fault(&[0xF0, 0x7D, 0x90, 0xF7]).unwrap();
    assert!(fault.contains("F0 7D 90 F7"), "the refusal names the bytes: {fault}");
}

#[test]
fn a_packed_reply_is_read_once_unpacked() {
    // The M-VAVE identify query, as sent and as its 8-bit data.
    let query = [0xF0, 0x00, 0x32, 0x45, 0x00, 0x00, 0x00, 0x40, 0x7F, 0xF7];
    assert_eq!(unpack_lsb_8to7(&query), [0x00, 0x59, 0x11, 0x00, 0x00, 0x00, 0xFF]);
    let pack = |data: &[u8]| -> Vec<u8> {
        let (mut out, mut acc, mut bits) = (vec![0xF0], 0u32, 0u32);
        for b in data {
            acc |= (*b as u32) << bits;
            bits += 8;
            while bits >= 7 { out.push((acc & 0x7F) as u8); acc >>= 7; bits -= 7; }
        }
        if bits > 0 { out.push((acc & 0x7F) as u8); }
        out.push(0xF7);
        out
    };
    assert_eq!(pack(&[0x00, 0x59, 0x11, 0x00, 0x00, 0x00, 0xFF]), query, "the test packs as the device does");
    let reply = pack(&[&[0x00, 0x59, 0x11, 0x1B, 0x00, 0x00][..], b"FM-1_016", &[0; 19], &[0x42]].concat());
    let v16 = ident(r#"{"request":[240,0,50,69,0,0,0,64,127,247],"unpack":"lsb_8to7","reply":[0,89,17],"reply_text":"FM-1_016",
        "firmware":{"decode":"ascii","at":[6]}}"#);
    let r = recognise(&v16, "FM-1 Midi", None, std::slice::from_ref(&reply)).unwrap();
    assert_eq!(r.firmware.as_deref(), Some("FM-1_016"));
    let mut v15 = v16.clone();
    v15.reply_text = Some(Some("FM-1_015".into()));
    assert!(recognise(&v15, "FM-1 Midi", None, &[reply]).is_none());
}

#[test]
fn a_maker_pattern_takes_the_models_bytes_and_lua_reads_what_no_rule_covers() {
    let id = ident(r#"{"family":[89,1],"member":[0,0],"reply":[240,126,{"device_id":true},6,2,65,{"identity":"family"},{"identity":"member"}]}"#);
    assert!(recognise(&id, "x", None, &[ROLAND.to_vec()]).is_some());
    let mut other = id.clone();
    other.family = Some(Some(vec![20, 2]));
    assert!(recognise(&other, "x", None, &[ROLAND.to_vec()]).is_none(), "another model's bytes");
    let req = ident(r#"{"request":[240,0,32,50,0,1,{"identity":"family"},8,0,247],"family":[36,0],"reply":[240]}"#);
    assert_eq!(probe_of(&req).unwrap().unwrap().request, [0xF0, 0, 0x20, 0x32, 0, 1, 0x24, 0, 8, 0, 0xF7]);
    // A version written as BCD in one byte: no built-in rule, a few lines of Lua.
    let lua = ident(r#"{"reply":[240,126,"any",6,2,65],
        "script":"return reply[7] == 0x59",
        "firmware":{"decode":"lua","at":[],"script":"local b = reply[12]\nreturn string.format('%d.%02d', b >> 4, b & 15)"}}"#);
    let r = recognise(&lua, "x", None, &[ROLAND.to_vec()]).unwrap();
    assert_eq!(r.firmware.as_deref(), Some("0.03"));
    let not_it = [0xF0, 0x7E, 0x10, 0x06, 0x02, 0x41, 0x14, 0x02, 0, 0, 0, 0x12, 0, 0, 0xF7];
    assert!(recognise(&lua, "x", None, &[not_it.to_vec()]).is_none(), "its script says no");
}

#[test]
fn a_usb_port_name_recognises_a_device_that_answers_nothing() {
    let cat = vec![with_id("plinky", None, r#"{"ports":["PlinkySynth"]}"#), cand("other", "Other", "Acme", &[])];
    let best = rank("plinkysynth MIDI 1", None, &[], &cat, 3);
    assert_eq!(cat[best[0].index].slug, "plinky");
    assert!(best[0].recognised.as_ref().unwrap().by_port);
}

#[test]
fn the_signs_a_device_gives_of_itself_do_not_change_what_recognises_it() {
    let plain = ident(r#"{"reply":[240,126,{"device_id":true},6,2,65,89,1,0,0]}"#);
    let signed = ident(r#"{"reply":[240,126,{"device_id":true},6,2,65,89,1,0,0],
        "usb":[{"vendor_id":1,"product_id":2}],"sends":{"active_sensing":true,"clock":true}}"#);
    assert_eq!(signed.usb.as_ref().and_then(Option::as_ref).map(Vec::len), Some(1), "read and kept");
    assert_eq!(recognise(&plain, "x", None, &[ROLAND.to_vec()]).is_some(), recognise(&signed, "x", None, &[ROLAND.to_vec()]).is_some());
    assert!(recognise(&signed, "x", None, &[]).is_none(), "what it sends unasked is not a reply");
    assert_eq!(probe_of(&plain).unwrap(), probe_of(&signed).unwrap());
}

#[test]
fn firmware_decodes_as_each_maker_writes_it() {
    let rule = |decode: &str, at: &[usize], field: Option<&str>| -> osd::FirmwareRule {
        let at: Vec<String> = at.iter().map(|a| a.to_string()).collect();
        let field = field.map(|f| format!(r#","field":"{f}""#)).unwrap_or_default();
        ident(&format!(r#"{{"reply":[240],"firmware":{{"decode":"{decode}","at":[{}]{field}}}}}"#, at.join(",")))
            .firmware.unwrap().unwrap()
    };
    // Korg: minor 10 (LSB, MSB), major 2.
    let korg = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x42, 0x51, 0x01, 0x00, 0x00, 10, 0, 2, 0, 0xF7];
    assert_eq!(decode_firmware(&rule("korg_minor_major", &[10, 11, 12, 13], None), &korg).as_deref(), Some("2.10"));
    assert_eq!(decode_firmware(&rule("raw_bytes", &[10, 12], None), &korg).as_deref(), Some("10.2"));
    let seqtrak = [0xF0, 0x7E, 0x7F, 0x06, 0x02, 0x43, 0x00, 0x41, 0x64, 0x06, 5, 0, 0, 0x7F, 0xF7];
    assert_eq!(decode_firmware(&rule("yamaha_tenths", &[10], None), &seqtrak).as_deref(), Some("1.5"));
    assert_eq!(decode_firmware(&rule("raw_bytes", &[40], None), &seqtrak), None, "past the end");
    // Electra One: its info reply is JSON after six header bytes.
    let electra = [&[0xF0, 0x00, 0x21, 0x45, 0x01, 0x7F][..], br#"{"versionText":"v4.1.4","model":"mk2"}"#, &[0xF7]].concat();
    assert_eq!(decode_firmware(&rule("json", &[6], Some("versionText")), &electra).as_deref(), Some("4.1.4"));
    assert_eq!(decode_firmware(&rule("json", &[6], Some("serial")), &electra), None, "no such key");
    assert!(same_version("2.0", "2.0.0") && same_version("v1.5", "1.5") && !same_version("2.1", "2.10"));
    assert!(same_version("hacktribe-94c8e60", "hacktribe-94c8e60") && !same_version("1.2", "x"));
    assert!(same_version("1.2.1", "community-1.2.1") && same_version("FM-1_015", "15") && !same_version("1.1", "1.2.1"));
    assert!(same_version("1.1", "clipper-1.1-experimental") && !same_version("1.2", "1.2-on-2.0"), "the last number counts");
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

fn usb_driver(slug: &str, usb: &str) -> Candidate {
    with_id(slug, None, &format!(r#"{{"usb":{usb}}}"#))
}

#[test]
fn a_driver_naming_the_usb_ids_of_the_port_ranks_above_a_port_name_match() {
    let cat = vec![
        with_id("by_name", None, r#"{"ports":["Pad"]}"#),
        usb_driver("by_usb", r#"[{"vendor_id":4660,"product_id":22136}]"#),
    ];
    let best = rank("Pad MIDI 1", Some((4660, 22136)), &[], &cat, 3);
    assert_eq!(cat[best[0].index].slug, "by_usb");
    assert_eq!(best[0].score, 80);
    let named = best.iter().find(|r| cat[r.index].slug == "by_name").unwrap();
    assert_eq!(named.score, 60, "the port name keeps its 60");
}

#[test]
fn a_driver_not_naming_the_usb_ids_gains_nothing() {
    let cat = vec![usb_driver("other", r#"[{"vendor_id":1,"product_id":2}]"#)];
    assert!(rank("x", Some((4660, 22136)), &[], &cat, 3).is_empty(), "no other sign: dropped");
    let with_name = vec![Candidate { name: "Pad".into(), ..usb_driver("pad", r#"[{"vendor_id":1,"product_id":2}]"#) }];
    let with = rank("Pad", Some((4660, 22136)), &[], &with_name, 3);
    let without = rank("Pad", None, &[], &with_name, 3);
    assert_eq!(with[0].score, without[0].score);
    let half = vec![usb_driver("half", r#"[{"vendor_id":4660,"product_id":2}]"#)];
    assert!(rank("x", Some((4660, 22136)), &[], &half, 3).is_empty(), "both ids must match");
}

#[test]
fn a_driver_among_several_usb_ids_matches_any_of_them() {
    let cat = vec![usb_driver("multi", r#"[{"vendor_id":1,"product_id":2},{"vendor_id":4660,"product_id":22136}]"#)];
    assert_eq!(rank("x", Some((4660, 22136)), &[], &cat, 3).len(), 1);
}

#[test]
fn without_usb_ids_the_ranking_is_what_it_was() {
    let cat = vec![usb_driver("by_usb", r#"[{"vendor_id":4660,"product_id":22136}]"#)];
    assert!(rank("x", None, &[], &cat, 3).is_empty());
}

#[test]
fn the_usb_ids_recognise_a_device_that_answers_nothing_on_a_port_named_after_nothing() {
    let id = ident(r#"{"usb":[{"vendor_id":4660,"product_id":22136}]}"#);
    let r = recognise(&id, "USB MIDI Interface", Some((4660, 22136)), &[]).unwrap();
    assert!(r.by_usb && !r.by_reply && !r.by_port);
    assert!(recognise(&id, "USB MIDI Interface", Some((1, 2)), &[]).is_none());
    assert!(recognise(&id, "USB MIDI Interface", None, &[]).is_none());
    let id = ident(r#"{"ports":["Pad"],"usb":[{"vendor_id":4660,"product_id":22136}]}"#);
    let r = recognise(&id, "Pad", Some((4660, 22136)), &[]).unwrap();
    assert!(r.by_usb && r.by_port);
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
