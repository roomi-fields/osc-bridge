use super::*;

fn load(id: &str) -> osd::Device {
    crate::device::load(format!("tests/fixtures/devices/{id}.json")).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn parse(xml: &str) -> roxmltree::Document<'_> {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    roxmltree::Document::parse_with_options(xml, opts).unwrap_or_else(|e| panic!("not well-formed: {e}\n{xml}"))
}

fn all<'a>(doc: &'a roxmltree::Document, tag: &str) -> Vec<roxmltree::Node<'a, 'a>> {
    doc.descendants().filter(|n| n.has_tag_name(tag)).collect()
}

/// The `_usage` note of a control, kept by `osd` among the remarks of its entry.
fn usage(e: &osd::CcEntry) -> Option<&str> {
    e.remarks.get("_usage").and_then(|v| v.as_str())
}

#[test]
fn a_cc_and_nrpn_synth_names_its_controls() {
    let dev = load("novation/bass_station_ii");
    let xml = midnam(&dev).unwrap();
    let doc = parse(&xml);
    let root = doc.root_element();
    assert_eq!(root.tag_name().name(), "MIDINameDocument");
    let order: Vec<&str> = root.children().filter(|n| n.is_element()).map(|n| n.tag_name().name()).collect();
    assert_eq!(order, ["Author", "MasterDeviceNames"]);
    assert!(all(&doc, "Author")[0].text().unwrap().contains("CC-BY-SA-4.0"));
    assert!(xml.contains("Source: MIDI Guide"), "credit in the comment");
    assert_eq!(all(&doc, "Manufacturer")[0].text(), Some("Novation"));
    assert_eq!(all(&doc, "ChannelNameSetAssign").len(), 16);
    let controls = all(&doc, "Control");
    let cc = dev.cc_params.as_ref().unwrap();
    let first_cc = cc.entries.iter().find(|e| e.cc.is_some() && e.nrpn_msb.is_none()).unwrap();
    assert!(controls.iter().any(|c| c.attribute("Type") == Some("7bit")
        && c.attribute("Number") == Some(&first_cc.cc.unwrap().to_string())));
    let nrpn = cc.entries.iter().find(|e| e.nrpn_msb.is_some() && e.nrpn_lsb.is_some()).unwrap();
    let number = ((nrpn.nrpn_msb.unwrap() as u16) << 7 | nrpn.nrpn_lsb.unwrap() as u16).to_string();
    assert!(controls.iter().any(|c| c.attribute("Type") == Some("NRPN") && c.attribute("Number") == Some(&number)));
    let named = cc.entries.iter().filter(|e| usage(e).and_then(value_names).is_some()).count();
    assert!(named > 0 && !all(&doc, "ValueNameList").is_empty(), "named values from the source's usage");
    assert!(all(&doc, "NoteNameList").is_empty(), "no drum map");
}

#[test]
fn a_drum_machine_names_its_notes_per_channel() {
    let dev = load("elektron/model_samples");
    let doc_text = midnam(&dev).unwrap();
    let doc = parse(&doc_text);
    let notes = all(&doc, "Note");
    let t = &dev.triggers.as_ref().unwrap().entries;
    let first = &t[0];
    assert!(notes.iter().any(|n| n.attribute("Number") == Some(&first.note.unwrap().to_string())
        && n.attribute("Name") == Some(first.sound.as_str())));
    let sets: Vec<_> = all(&doc, "ChannelNameSet");
    let lists: Vec<_> = all(&doc, "UsesNoteNameList");
    assert!(!lists.is_empty());
    for u in lists {
        let name = u.attribute("Name").unwrap();
        assert_eq!(all(&doc, "NoteNameList").iter().filter(|l| l.attribute("Name") == Some(name)).count(), 1, "{name}");
    }
    for s in sets {
        let elems: Vec<&str> = s.children().filter(|n| n.is_element()).map(|n| n.tag_name().name()).collect();
        assert_eq!(elems.first(), Some(&"AvailableForChannels"));
        assert_eq!(elems.last(), Some(&"PatchBank"), "the DTD's order");
    }
}

#[test]
fn a_firmware_sharing_midi_credits_both_and_osc_devices_have_no_midnam() {
    let fw = load("arturia/microfreak.vendor.fw-microfreak-tools-0.1.0");
    let xml = midnam(&fw).unwrap();
    let doc = parse(&xml);
    let author = all(&doc, "Author")[0].text().unwrap().to_string();
    assert!(author.contains("Apache-2.0") && author.contains("CC-BY-SA-4.0"), "{author}");
    assert!(xml.contains("MIDI shared from arturia/microfreak"));
    assert!(!all(&doc, "Control").is_empty(), "the shared controllers");
    assert!(midnam(&load("bitwig/bitwig-studio.third-party-osc.fw-5")).is_err());
}

#[test]
fn program_banks_carry_their_bank_select_and_program_changes() {
    let xml = midnam(&load("acme/box.vendor.fw-2")).unwrap();
    let doc = parse(&xml);
    let banks = all(&doc, "PatchBank");
    let names: Vec<&str> = banks.iter().filter_map(|b| b.attribute("Name")).collect();
    assert_eq!(names, ["User", "Factory"]);
    let cc: Vec<(&str, &str)> = banks[1].descendants().filter(|n| n.has_tag_name("ControlChange"))
        .map(|n| (n.attribute("Control").unwrap(), n.attribute("Value").unwrap())).collect();
    assert_eq!(cc, [("0", "0"), ("32", "1")]);
    let patches: Vec<_> = banks[1].descendants().filter(|n| n.has_tag_name("Patch")).collect();
    assert_eq!(patches.len(), 64);
    assert_eq!((patches[63].attribute("Number"), patches[63].attribute("ProgramChange")), (Some("64"), Some("63")));
    let plain = midnam(&load("korg/minilogue")).unwrap();
    assert!(plain.contains(r#"<PatchBank Name="Programs">"#), "no banks: the empty one the DTD wants");
}

#[test]
fn usage_text_gives_named_values_only_when_it_lists_them() {
    assert_eq!(value_names("0: Off; 1-127: On"), Some(vec![(0, "Off".into()), (1, "On".into())]));
    assert_eq!(value_names("0: Heel; 127: Toe"), Some(vec![(0, "Heel".into()), (127, "Toe".into())]));
    assert_eq!(value_names("0-11: Select machine"), None);
    assert_eq!(value_names("Cutoff frequency"), None);
    assert_eq!(value_names("-12: Down; 0: Centre; 12: Up").map(|v| v[0].0), Some(-12));
    assert_eq!(esc(r#"A & "B" <c>"#), "A &amp; &quot;B&quot; &lt;c&gt;");
    assert_eq!(comment("a -- b-"), "a - - b- ");
}
