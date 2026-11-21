use super::*;

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
    let felucca = all.iter().find(|p| p.request[1] == 0x7D).unwrap();
    assert!(felucca.goes_to("FM-1 Midi") && !felucca.goes_to("Digitakt"), "sent only where it is safe");
    assert!(all.iter().find(|p| p.request[1] == 0x00).unwrap().goes_to("anything"));
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
        ("Minilab3_1".to_string(), 0x1c75, 0x0288),
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
