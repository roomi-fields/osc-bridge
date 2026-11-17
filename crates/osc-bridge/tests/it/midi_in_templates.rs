//! The address of each incoming MIDI message: `osd` gives the default
//! template of a listed type; a device's own templates are kept as written.

use osc_bridge::midi_codec::{midi_to_osc, template};
use osd::{MidiIn, MidiTemplates, MidiType};

const ALL: [MidiType; 7] = [
    MidiType::NoteOn,
    MidiType::NoteOff,
    MidiType::Cc,
    MidiType::Pitchbend,
    MidiType::Aftertouch,
    MidiType::PolyAftertouch,
    MidiType::ProgramChange,
];

fn own(note_on: Option<&str>, cc: Option<&str>) -> MidiTemplates {
    let s = |o: Option<&str>| o.map(str::to_string);
    MidiTemplates {
        note_on: s(note_on),
        note_off: None,
        cc: s(cc),
        pitchbend: None,
        aftertouch: None,
        poly_aftertouch: None,
        program_change: None,
        remarks: Default::default(),
    }
}

#[test]
fn listed_type_is_reported_at_the_template_osd_publishes() {
    let map = MidiIn::Types(ALL.to_vec());
    for t in ALL {
        assert_eq!(template(Some(&map), t), Some(t.template().to_string()), "{t:?}");
    }
}

#[test]
fn type_absent_from_the_list_gets_no_address() {
    let map = MidiIn::Types(vec![MidiType::Cc]);
    assert_eq!(template(Some(&map), MidiType::Cc), Some(MidiType::Cc.template().to_string()));
    for t in ALL.into_iter().filter(|t| *t != MidiType::Cc) {
        assert_eq!(template(Some(&map), t), None, "{t:?}");
    }
    assert_eq!(template(Some(&MidiIn::Types(vec![])), MidiType::NoteOn), None);
    assert_eq!(template(None, MidiType::NoteOn), None);
}

#[test]
fn incoming_messages_get_the_default_address_under_the_prefix() {
    let map = MidiIn::Types(ALL.to_vec());
    let m = midi_to_osc(&[0x92, 60, 100], Some(&map), "/dev").unwrap();
    assert_eq!(m.addr, "/dev/note/on");
    let m = midi_to_osc(&[0xB1, 7, 99], Some(&map), "/dev").unwrap();
    assert_eq!(m.addr, "/dev/cc/7");
    let m = midi_to_osc(&[0xE0, 0, 64], Some(&map), "/dev").unwrap();
    assert_eq!(m.addr, "/dev/pitchbend");
    let m = midi_to_osc(&[0xC0, 5], Some(&map), "/dev").unwrap();
    assert_eq!(m.addr, "/dev/program_change");
    let only_cc = MidiIn::Types(vec![MidiType::Cc]);
    assert!(midi_to_osc(&[0x90, 60, 100], Some(&only_cc), "/dev").is_none());
}

#[test]
fn device_templates_are_kept_and_undescribed_types_get_no_address() {
    let map = MidiIn::Templates(own(Some("/my/on {note}"), Some("/my/cc/{num} {value}")));
    assert_eq!(template(Some(&map), MidiType::NoteOn), Some("/my/on {note}".to_string()));
    assert_eq!(template(Some(&map), MidiType::Cc), Some("/my/cc/{num} {value}".to_string()));
    for t in ALL.into_iter().filter(|t| !matches!(t, MidiType::NoteOn | MidiType::Cc)) {
        assert_eq!(template(Some(&map), t), None, "{t:?}");
    }
    let m = midi_to_osc(&[0xB0, 3, 9], Some(&map), "/my").unwrap();
    assert_eq!(m.addr, "/my/cc/3");
    assert!(midi_to_osc(&[0xC0, 5], Some(&map), "/my").is_none());
}
