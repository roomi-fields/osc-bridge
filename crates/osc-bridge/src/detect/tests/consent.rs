//! The maker requests a port may receive without the person's consent, and the consent
//! that adds the others to the port it names.

use super::*;

const ARTURIA_USB: (u16, u16) = (0x1c75, 0x020b);

/// The probe of an identity block: its own request `[F0, <tag>, F7]`, then `extra`.
fn probe_with(tag: u8, extra: &str) -> Probe {
    let block = format!(r#"{{"request":[240,{tag},247],"reply":[240,{tag}]{extra}}}"#);
    probe_of(&ident(&block)).unwrap().expect("a maker request")
}

fn named(tag: u8, port: &str) -> Probe {
    probe_with(tag, &format!(r#","to":"named_ports","ports":["{port}"]"#))
}
fn by_usb(tag: u8) -> Probe {
    probe_with(tag, r#","usb":[{"vendor_id":7285,"product_id":523}]"#)
}
fn bare(tag: u8) -> Probe {
    probe_with(tag, "")
}
fn req(tag: u8) -> Vec<u8> {
    vec![0xF0, tag, 0xF7]
}
fn ports(list: &[(&str, Option<(u16, u16)>)]) -> Vec<(String, Option<(u16, u16)>)> {
    list.iter().map(|(n, u)| (n.to_string(), *u)).collect()
}
fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_probe_carries_the_usb_ids_and_the_port_names_of_its_identity() {
    let p = probe_with(9, r#","usb":[{"vendor_id":7285,"product_id":523}],"ports":["Mini"]"#);
    assert_eq!(p.usb, vec![ARTURIA_USB]);
    assert_eq!(p.match_ports, strings(&["Mini"]));
    assert!(p.ports.is_empty(), "`to` is all: the names are for matching, not for addressing");
    let n = named(8, "FM-1");
    assert_eq!((n.ports.clone(), n.match_ports.clone()), (strings(&["FM-1"]), strings(&["FM-1"])));
    let b = bare(7);
    assert!(b.usb.is_empty() && b.match_ports.is_empty() && b.ports.is_empty());
}

#[test]
fn a_port_with_a_usb_id_is_matched_by_that_id_alone() {
    let p = probe_with(9, r#","usb":[{"vendor_id":7285,"product_id":523}],"ports":["Mini"]"#);
    assert!(p.safe_for("whatever name", Some(ARTURIA_USB)), "its usb id");
    assert!(!p.safe_for("Mini Lab", Some((1, 2))), "another usb id: the name no longer counts");
    assert!(p.safe_for("Mini Lab", None), "no usb id read: the name counts");
    assert!(!p.safe_for("Digitakt", None));
}

#[test]
fn a_probe_with_usb_and_no_port_names_goes_only_to_that_usb_id() {
    let p = by_usb(9);
    assert!(p.safe_for("any", Some(ARTURIA_USB)));
    assert!(!p.safe_for("Mini Lab", None), "no usb id read and no name to compare");
    assert!(!p.safe_for("Mini Lab", Some((1, 2))));
}

#[test]
fn a_probe_without_usb_ids_and_names_is_held_back_on_every_port() {
    let p = bare(7);
    for usb in [None, Some(ARTURIA_USB)] {
        assert!(!p.safe_for("Minilab3", usb));
    }
}

#[test]
fn an_unknown_port_receives_the_universal_request_alone_and_has_every_maker_request_held() {
    let probes = [named(8, "FM-1"), by_usb(9), bare(7)];
    let plan = send_plan(&ports(&[("Mystery Box 20:0", None)]), &probes, &[]);
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].port, "Mystery Box 20:0");
    assert_eq!(plan[0].sent, vec![IDENTITY_REQUEST.to_vec()]);
    assert_eq!(plan[0].held, vec![req(8), req(9), req(7)], "in the order of the probes");
}

#[test]
fn a_port_receives_the_requests_of_its_maker_and_holds_the_others() {
    let probes = [named(8, "FM-1"), by_usb(9), bare(7)];
    let all = ports(&[("Minilab3 MIDI", Some(ARTURIA_USB)), ("FM-1 Midi", None), ("Digitakt", None)]);
    let plan = send_plan(&all, &probes, &[]);
    assert_eq!(plan[0].sent, vec![IDENTITY_REQUEST.to_vec(), req(9)]);
    assert_eq!(plan[0].held, vec![req(8), req(7)]);
    assert_eq!(plan[1].sent, vec![IDENTITY_REQUEST.to_vec(), req(8)]);
    assert_eq!(plan[1].held, vec![req(9), req(7)]);
    assert_eq!(plan[2].sent, vec![IDENTITY_REQUEST.to_vec()]);
    assert_eq!(plan[2].held.len(), 3);
}

#[test]
fn a_port_named_by_two_makers_receives_the_requests_of_those_two_and_no_other() {
    let probes = [named(8, "Combo"), probe_with(9, r#","ports":["combo box"]"#), named(6, "Other")];
    let plan = send_plan(&ports(&[("Combo Box MIDI", None)]), &probes, &[]);
    assert_eq!(plan[0].sent, vec![IDENTITY_REQUEST.to_vec(), req(8), req(9)]);
    assert_eq!(plan[0].held, vec![req(6)]);
}

#[test]
fn a_consented_port_receives_the_held_requests_and_holds_none_the_others_are_unchanged() {
    let probes = [named(8, "FM-1"), by_usb(9), bare(7)];
    let both = ports(&[("Mystery Box", None), ("Digitakt", None)]);
    let plan = send_plan(&both, &probes, &[0]);
    assert_eq!(plan[0].sent.len(), 4, "the universal request and the three maker requests");
    assert_eq!(plan[0].sent[0], IDENTITY_REQUEST.to_vec());
    for t in [8, 9, 7] {
        assert!(plan[0].sent.contains(&req(t)), "{t}");
    }
    assert!(plan[0].held.is_empty());
    assert_eq!(plan[1].sent, vec![IDENTITY_REQUEST.to_vec()], "consent is per port");
    assert_eq!(plan[1].held.len(), 3);
}

#[test]
fn a_request_goes_to_a_port_once_whether_safe_or_consented() {
    let probes = [by_usb(9), bare(7)];
    let plan = send_plan(&ports(&[("Minilab3", Some(ARTURIA_USB))]), &probes, &[0]);
    let mut sent = plan[0].sent.clone();
    sent.sort();
    sent.dedup();
    assert_eq!(sent.len(), plan[0].sent.len(), "{:?}", plan[0].sent);
    assert_eq!(plan[0].sent.len(), 3);
}

#[test]
fn a_consent_fragment_names_one_port_by_a_part_of_its_name_in_any_case() {
    let names = strings(&["Minilab3 MIDI 40:0", "Digitakt 41:0"]);
    assert_eq!(consent_indexes(&names, &strings(&["digi"])).unwrap(), vec![1]);
    assert_eq!(consent_indexes(&names, &strings(&["MINILAB", "digitakt"])).unwrap(), vec![0, 1]);
    assert!(consent_indexes(&names, &[]).unwrap().is_empty());
}

#[test]
fn an_exact_index_names_one_port() {
    let names = strings(&["Box A 20:0", "Box B 21:0", "Box C 22:0"]);
    assert_eq!(consent_indexes(&names, &strings(&["1"])).unwrap(), vec![1]);
    assert_eq!(consent_indexes(&names, &strings(&["0", "2"])).unwrap(), vec![0, 2]);
}

#[test]
fn a_consent_fragment_matching_several_ports_is_refused_naming_them() {
    let names = strings(&["Box A 20:0", "Box B 21:0", "Digitakt 22:0"]);
    let err = consent_indexes(&names, &strings(&["box"])).expect_err("ambiguous");
    assert!(err.contains("Box A 20:0") && err.contains("Box B 21:0") && !err.contains("Digitakt"), "{err}");
}

#[test]
fn a_consent_fragment_matching_no_port_is_refused_naming_it_and_the_ports_there_are() {
    let names = strings(&["Box A 20:0", "Digitakt 22:0"]);
    for bad in ["zzz", "7"] {
        let err = consent_indexes(&names, &strings(&[bad])).expect_err("no port");
        assert!(err.contains(bad) && err.contains("Box A 20:0") && err.contains("Digitakt 22:0"), "{err}");
    }
}

#[test]
fn one_bad_consent_fragment_refuses_the_whole_list() {
    let names = strings(&["Box A 20:0", "Digitakt 22:0"]);
    assert!(consent_indexes(&names, &strings(&["digi", "nothing"])).is_err());
}
