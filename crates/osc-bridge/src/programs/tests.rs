use super::*;

/// A 0.5 driver whose `programs` block is `programs`.
fn dev(programs: &str) -> osd::Device {
    let capabilities = if programs.contains("recall_script") { r#","capabilities":["lua"]"# } else { "" };
    let text = format!(r#"{{"$schema":"https://w3id.org/osd/0.5/device.schema.json",
        "device":{{"name":"Box","vendor":"Acme","osc_prefix":"/box"}}{capabilities},
        "midi_out":{{"default_channel":2}},"identity":{{"device_id":{{"default":16}},"family":[81,1],"reply":[240,126]}},
        "programs":{programs}}}"#);
    match osd::read(&text, osd::Kind::Device).unwrap_or_else(|e| panic!("{e}\n{text}")) {
        osd::Descriptor::Device(d) => *d,
        _ => unreachable!(),
    }
}

#[test]
fn a_bank_and_a_number_become_the_bank_select_and_the_program_change() {
    let d = dev(r#"{"banks":[{"name":"User","msb":81,"lsb":0,"size":128},
        {"name":"Preset A","msb":81,"lsb":64,"size":128,"display":"001-128"},
        {"name":"B","lsb":1,"first_pc":1,"size":72,"first_number":129}]}"#);
    assert_eq!(recall(&d, "preset a", 1, None).unwrap(), vec![vec![0xB2, 0, 81], vec![0xB2, 32, 64], vec![0xC2, 0]]);
    assert_eq!(recall(&d, "1", 128, Some(0)).unwrap(), vec![vec![0xB0, 0, 81], vec![0xB0, 32, 64], vec![0xC0, 127]]);
    assert_eq!(recall(&d, "B", 130, None).unwrap(), vec![vec![0xB2, 32, 1], vec![0xC2, 2]], "CC 32 only, PC from 1");
    assert!(recall(&d, "B", 128, None).unwrap_err().contains("129..=200"));
    assert!(recall(&d, "Card", 1, None).unwrap_err().contains("User, Preset A, B"));
}

#[test]
fn a_recall_sequence_wraps_the_program_change_or_replaces_it() {
    let d = dev(r#"{"before":[[240,66,{"arg":"channel","add":48},117,78,2,247]],
        "after":[[240,65,{"device_id":true},{"arg":"index","encoding":"u14_lsb"},{"arg":"index","encoding":"u14_msb"},247]],
        "banks":[{"name":"A","size":100},{"name":"B","size":100}]}"#);
    let m = recall(&d, "B", 100, None).unwrap();
    assert_eq!(m, vec![vec![0xF0, 0x42, 0x32, 0x75, 0x4E, 2, 0xF7], vec![0xC2, 99], vec![0xF0, 0x41, 16, 199 & 0x7F, 1, 0xF7]]);
    let song = dev(r#"{"program_change":false,"before":[[243,{"arg":"program"}]],"banks":[{"name":"Songs","size":100,"first_number":0}]}"#);
    assert_eq!(recall(&song, "Songs", 42, None).unwrap(), vec![vec![0xF3, 42]], "Song Select, no program change");
}

#[test]
fn a_dump_request_takes_the_models_bytes_and_a_checksum() {
    let d = dev(r#"{"banks":[{"name":"A","size":64}],
        "dump":{"kind":"request","request":[240,66,{"arg":"channel","add":48},{"identity":"family","index":0},28,{"arg":"index"},{"checksum":"twos7","from":3},247]}}"#);
    // 51 + 1C + 05 = 0x72; 128 - 0x72 = 0x0E.
    assert_eq!(dump(&d, Some(("A", 6)), None).unwrap(), vec![vec![0xF0, 0x42, 0x32, 81, 0x1C, 5, 0x0E, 0xF7]]);
    let edit = dev(r#"{"dump":{"kind":"edit_buffer","request":[240,66,{"arg":"channel","add":48},{"identity":"family","index":0},16,247]}}"#);
    assert_eq!(dump(&edit, None, Some(0)).unwrap(), vec![vec![0xF0, 0x42, 0x30, 81, 0x10, 0xF7]]);
    assert!(dump(&dev(r#"{"dump":{"kind":"request"}}"#), None, None).unwrap_err().contains("not in this driver"));
}

#[test]
fn lua_writes_a_recall_no_frame_can() {
    // A file named by its bank letter and number, as text (Behringer UB-Xa style).
    let d = dev(r#"{"banks":[{"name":"A","msb":0,"lsb":0,"size":128}],
        "recall_script":"local name = string.format('%s%03d', bank_name, number)\nlocal m = {0xF0, 0x7D}\nfor i = 1, #name do m[#m + 1] = name:byte(i) end\nm[#m + 1] = 0xF7\nreturn {m}"}"#);
    assert_eq!(recall(&d, "A", 1, None).unwrap(), vec![[&[0xF0, 0x7D][..], b"A001", &[0xF7]].concat()]);
}

#[test]
fn the_same_addresses_on_every_device() {
    let d = dev(r#"{"banks":[{"name":"A","size":16},{"name":"B","first_pc":16,"size":16}],"dump":{"kind":"edit_buffer","request":[240,1,{"arg":"channel"},247]}}"#);
    let call = |addr: &str, args: Vec<OscType>| uniform_osc(&d, addr, &args);
    assert_eq!(call("/box/program/recall", vec![OscType::String("B".into()), OscType::Int(3)]).unwrap().unwrap(), vec![vec![0xC2, 18]]);
    assert_eq!(call("/box/program/recall", vec![OscType::Int(1), OscType::Int(3), OscType::Int(9)]).unwrap().unwrap(), vec![vec![0xC9, 18]]);
    assert!(call("/box/program/recall", vec![OscType::String("B".into())]).unwrap().is_err());
    assert_eq!(call("/box/program/dump", vec![]).unwrap().unwrap(), vec![vec![0xF0, 1, 2, 0xF7]]);
    assert_eq!(call("/box/identity", vec![]).unwrap().unwrap(), vec![crate::detect::IDENTITY_REQUEST.to_vec()]);
    assert!(call("/box/program_change", vec![]).is_none());
    let reply = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x41];
    let m = identity_reply(&d, &reply).unwrap();
    assert_eq!((m.addr.as_str(), &m.args[1]), ("/box/identity/reply", &OscType::String("F0 7E 00 06 02 41".into())));
    assert!(identity_reply(&d, &[0xF0, 0x41, 0xF7]).is_none());
}
