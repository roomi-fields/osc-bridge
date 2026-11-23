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
fn no_maker_request_leaves_without_consent_whatever_designates_the_port() {
    let probes = [
        probe_with(9, r#","usb":[{"vendor_id":7285,"product_id":523}],"ports":["Mini"]"#),
        by_usb(8),
        named(6, "Mini Lab"),
        bare(7),
    ];
    let all = ports(&[("Mini Lab", Some(ARTURIA_USB)), ("Mini Lab", None), ("Mini", Some((1, 2))), ("Digitakt", None)]);
    for sends in send_plan(&all, &probes, &[]) {
        assert_eq!(sends.sent, vec![IDENTITY_REQUEST.to_vec()], "{}: the universal request alone", sends.port);
        assert_eq!(sends.held, vec![req(9), req(8), req(6), req(7)], "{}: every maker request is held", sends.port);
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
fn a_port_with_a_usb_id_holds_every_maker_request_too() {
    let probes = [named(8, "FM-1"), by_usb(9), bare(7)];
    let all = ports(&[("Minilab3 MIDI", Some(ARTURIA_USB)), ("FM-1 Midi", None), ("Digitakt", None)]);
    let plan = send_plan(&all, &probes, &[]);
    assert_eq!(plan[0].sent, vec![IDENTITY_REQUEST.to_vec()], "its usb id sends nothing");
    assert_eq!(plan[0].held, vec![req(8), req(9), req(7)]);
    assert_eq!(plan[1].sent, vec![IDENTITY_REQUEST.to_vec()], "named as its maker, no usb id read: held");
    assert_eq!(plan[1].held, vec![req(8), req(9), req(7)]);
    assert_eq!(plan[2].sent, vec![IDENTITY_REQUEST.to_vec()]);
    assert_eq!(plan[2].held.len(), 3);
}

#[test]
fn a_port_named_by_two_makers_without_a_usb_id_holds_every_maker_request_until_consent() {
    let probes = [named(8, "Combo"), probe_with(9, r#","ports":["combo box"]"#), named(6, "Other")];
    let plan = send_plan(&ports(&[("Combo Box MIDI", None)]), &probes, &[]);
    assert_eq!(plan[0].sent, vec![IDENTITY_REQUEST.to_vec()], "a name alone sends no maker request");
    assert_eq!(plan[0].held, vec![req(8), req(9), req(6)]);
    let consented = send_plan(&ports(&[("Combo Box MIDI", None)]), &probes, &[0]);
    assert_eq!(consented[0].sent.len(), 4, "consent sends the three maker requests");
    assert!(consented[0].held.is_empty());
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
