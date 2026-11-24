//! The USB ids a system reads for a port (CoreMIDI, WinMM), their conversion, and their place in the capture.

use super::*;
use crate::capture::{Asound, Capture};

#[test]
fn a_coremidi_usb_property_gives_vendor_on_the_high_16_bits_and_product_on_the_low() {
    assert_eq!(usb_from_coremidi(0x1c75_020b), Some((0x1c75, 0x020b)));
    assert_eq!(usb_from_coremidi(0x0001_0002), Some((1, 2)));
    // A vendor above 0x7fff makes the signed value negative; the bits are the same.
    assert_eq!(usb_from_coremidi(0x9999_0001_u32 as i32), Some((0x9999, 1)));
}

#[test]
fn a_zero_coremidi_usb_property_gives_no_ids() {
    assert_eq!(usb_from_coremidi(0), None);
}

#[test]
fn a_windows_interface_path_gives_vid_and_pid_in_either_case() {
    assert_eq!(usb_from_interface_path(r"\\?\usb#vid_1c75&pid_020b#5&2a#{6994ad04-93ef-11d0-a3cc-00a0c9223196}"), Some((0x1c75, 0x020b)));
    assert_eq!(usb_from_interface_path(r"\\?\USB#VID_1C75&PID_020B#ABC"), Some((0x1c75, 0x020b)));
}

#[test]
fn a_windows_interface_path_with_an_interface_number_gives_vid_and_pid() {
    assert_eq!(usb_from_interface_path(r"\\?\USB#VID_0763&PID_2010&MI_02#7&1a2b&0&0002#{xyz}"), Some((0x0763, 0x2010)));
}

#[test]
fn a_windows_interface_path_without_vid_and_pid_gives_no_ids() {
    assert_eq!(usb_from_interface_path(r"\\?\SWD#MMDEVAPI#MIDII_00000001.P_0000#{abc}"), None);
    assert_eq!(usb_from_interface_path(""), None);
    assert_eq!(usb_from_interface_path(r"\\?\USB#VID_1C75#x"), None, "no PID");
    assert_eq!(usb_from_interface_path(r"\\?\USB#VID_ZZZZ&PID_020B#x"), None, "not hex");
}

#[test]
fn a_winmm_device_name_matches_the_midir_port_name_only_when_it_names_the_same_device() {
    assert!(winmm_name_matches("Minilab3 MIDI", "Minilab3 MIDI"));
    assert!(!winmm_name_matches("Minilab3 MIDI", "Digitakt"));
    assert!(!winmm_name_matches("Pad", "Pad 2"));
    assert!(!winmm_name_matches("Pad", ""));
}

#[test]
fn a_winmm_device_name_cut_at_31_characters_matches_the_longer_port_name() {
    let long = "Arturia KeyLab mkII 61 Extended Name";
    let cut: String = long.chars().take(31).collect();
    assert!(winmm_name_matches(long, &cut));
    assert!(!winmm_name_matches("Arturia KeyLab mkII 88 Extended Name", &cut));
    // A short name is never matched as a mere prefix.
    assert!(!winmm_name_matches("Pad Extended", "Pad"));
}

fn capture(outs: &[&str], ins: &[&str]) -> Capture {
    Capture::new(outs.iter().map(|s| s.to_string()).collect(), ins.iter().map(|s| s.to_string()).collect(), Asound::default())
}

const CARDS: &str = " 1 [Minilab3       ]: USB-Audio - Minilab3\n                      Arturia at usb-0000:00:14.0-2, full speed\n";

#[test]
fn a_port_takes_its_usb_ids_from_the_system_ids_of_the_capture() {
    let mut c = Capture::parse(&capture(&["Minilab3 MIDI"], &["Minilab3 MIDI"]).to_json()).unwrap();
    c.system_usb.insert("Minilab3 MIDI".into(), (0x1c75, 0x020b));
    assert_eq!(replay(&c)[0].usb, Some((0x1c75, 0x020b)));
    assert_eq!(output_usbs(&c), vec![("Minilab3 MIDI".to_string(), Some((0x1c75, 0x020b)))]);
}

#[test]
fn a_port_without_system_ids_takes_its_usb_ids_from_the_sound_cards() {
    let mut c = capture(&["Minilab3:Minilab3 MIDI 40:0"], &[]);
    c.cards = CARDS.into();
    c.usbids = vec![crate::capture::UsbId { card: 1, text: "1c75:020b\n".into() }];
    let other = replay(&c)[0].usb;
    assert_eq!(other, Some((0x1c75, 0x020b)));
}

#[test]
fn the_system_ids_win_over_the_sound_cards_when_a_capture_carries_both() {
    let mut c = capture(&["Minilab3:Minilab3 MIDI 40:0"], &[]);
    c.cards = CARDS.into();
    c.usbids = vec![crate::capture::UsbId { card: 1, text: "1c75:020b\n".into() }];
    c.system_usb.insert("Minilab3:Minilab3 MIDI 40:0".into(), (0x0001, 0x0002));
    assert_eq!(replay(&c)[0].usb, Some((1, 2)));
}

#[test]
fn the_system_ids_of_an_input_only_port_are_read_by_its_name() {
    let mut c = capture(&[], &["Keys In"]);
    c.system_usb.insert("Keys In".into(), (0x0763, 0x2010));
    assert_eq!(replay(&c)[0].usb, Some((0x0763, 0x2010)));
}

#[test]
fn a_port_the_system_ids_do_not_name_keeps_no_ids() {
    let mut c = capture(&["Pad", "Other"], &[]);
    c.system_usb.insert("Pad".into(), (1, 2));
    let found = replay(&c);
    assert_eq!(found[0].usb, Some((1, 2)));
    assert_eq!(found[1].usb, None);
}

#[test]
fn the_system_ids_are_kept_in_the_capture_json_by_port_name() {
    let mut c = capture(&["Pad"], &[]);
    c.system_usb.insert("Pad".into(), (1, 2));
    let again = Capture::parse(&c.to_json()).unwrap();
    assert_eq!(again.system_usb.get("Pad"), Some(&(1, 2)));
    assert_eq!(again, c);
}

#[test]
fn a_capture_recorded_without_system_ids_is_read_unchanged_and_written_without_them() {
    let old = r#"{"format_version":2,"outputs":["Pad"],"inputs":[],"cards":"","usbids":[],"clients":"","requests":[],"received":[]}"#;
    let c = Capture::parse(old).unwrap();
    assert!(c.system_usb.is_empty());
    assert!(!c.to_json().contains("system_usb"));
}

fn winrt(name: &str, id: &str, instance_ids: &[&str]) -> WinrtDevice {
    WinrtDevice { name: name.into(), id: id.into(), instance_ids: instance_ids.iter().map(|s| s.to_string()).collect() }
}

fn ports(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

const SOFTWARE_ID: &str = r"\\?\SWD#MMDEVAPI#MIDII_00000001.P_0000#{abc}";

#[test]
fn a_winrt_device_gives_its_port_the_ids_of_its_own_id_path() {
    let devices = [winrt("Minilab3 MIDI", r"\\?\USB#VID_1C75&PID_020B#ABC", &[])];
    let found = usb_of_winrt_devices(&ports(&["Minilab3 MIDI"]), &devices);
    assert_eq!(found.get("Minilab3 MIDI"), Some(&(0x1c75, 0x020b)));
    assert_eq!(found.len(), 1);
}

#[test]
fn a_winrt_device_whose_id_is_a_software_path_takes_the_ids_of_its_container_instance() {
    let devices = [winrt("Minilab3 MIDI", SOFTWARE_ID, &["SWD\\X\\1", r"USB\VID_1C75&PID_020B&MI_03\7&1a2b&0&0002"])];
    let found = usb_of_winrt_devices(&ports(&["Minilab3 MIDI"]), &devices);
    assert_eq!(found.get("Minilab3 MIDI"), Some(&(0x1c75, 0x020b)));
}

#[test]
fn the_first_winrt_path_that_carries_ids_wins() {
    let devices = [winrt("Pad", r"\\?\USB#VID_0001&PID_0002#a", &[r"USB\VID_0003&PID_0004\b"])];
    assert_eq!(usb_of_winrt_devices(&ports(&["Pad"]), &devices).get("Pad"), Some(&(1, 2)));
    let devices = [winrt("Pad", SOFTWARE_ID, &[r"USB\VID_0003&PID_0004\b", r"USB\VID_0005&PID_0006\c"])];
    assert_eq!(usb_of_winrt_devices(&ports(&["Pad"]), &devices).get("Pad"), Some(&(3, 4)));
}

#[test]
fn a_winrt_device_with_no_ids_in_any_path_gives_its_port_nothing() {
    let devices = [winrt("Loop Port", SOFTWARE_ID, &["SWD\\MMDEVAPI\\MIDII_1", ""])];
    assert!(usb_of_winrt_devices(&ports(&["Loop Port"]), &devices).is_empty());
    let devices = [winrt("Loop Port", SOFTWARE_ID, &[])];
    assert!(usb_of_winrt_devices(&ports(&["Loop Port"]), &devices).is_empty());
}

#[test]
fn no_winrt_devices_or_no_ports_give_an_empty_table() {
    assert!(usb_of_winrt_devices(&ports(&["Pad"]), &[]).is_empty());
    let devices = [winrt("Pad", r"\\?\USB#VID_0001&PID_0002#a", &[])];
    assert!(usb_of_winrt_devices(&[], &devices).is_empty());
}

#[test]
fn a_winrt_device_is_matched_to_a_port_name_cut_at_31_characters() {
    let long = "Arturia KeyLab mkII 61 Extended Name";
    let cut: String = long.chars().take(31).collect();
    let devices = [winrt(long, r"\\?\USB#VID_1C75&PID_0001#a", &[])];
    let found = usb_of_winrt_devices(&[cut.clone(), "Digitakt".into()], &devices);
    assert_eq!(found.get(&cut), Some(&(0x1c75, 1)));
    assert_eq!(found.len(), 1);
}

#[test]
fn each_port_takes_the_ids_of_its_own_winrt_device() {
    let devices = [
        winrt("Pad", r"\\?\USB#VID_0001&PID_0002#a", &[]),
        winrt("Keys", r"\\?\USB#VID_0003&PID_0004#a", &[]),
    ];
    let found = usb_of_winrt_devices(&ports(&["Keys", "Pad", "Other"]), &devices);
    assert_eq!(found.get("Pad"), Some(&(1, 2)));
    assert_eq!(found.get("Keys"), Some(&(3, 4)));
    assert_eq!(found.get("Other"), None);
}

#[test]
fn two_winrt_devices_named_like_one_port_give_ids_only_when_they_agree() {
    let agree = [
        winrt("Pad", r"\\?\USB#VID_0001&PID_0002#a", &[]),
        winrt("Pad", SOFTWARE_ID, &[r"USB\VID_0001&PID_0002\b"]),
    ];
    assert_eq!(usb_of_winrt_devices(&ports(&["Pad"]), &agree).get("Pad"), Some(&(1, 2)));
    let differ = [
        winrt("Pad", r"\\?\USB#VID_0001&PID_0002#a", &[]),
        winrt("Pad", r"\\?\USB#VID_0003&PID_0004#a", &[]),
    ];
    assert!(usb_of_winrt_devices(&ports(&["Pad"]), &differ).is_empty());
}
