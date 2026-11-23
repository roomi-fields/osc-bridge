//! An `identity.request` the engine cannot build is a typed fault the loaded driver carries
//! (the driver, the field, the cause); detection says which of the two requests it sent.

use crate::common::{self, D05};

use osc_bridge::recognition::{self, probe_of, IDENTITY_REQUEST};
use osc_bridge::device::{faults, load};
use osc_bridge::programs;

/// A driver named "Acme Box" whose identity block is `identity`, loaded from a file.
fn loaded(identity: &str) -> (tempfile::TempDir, osd::Device) {
    let dir = common::temp("identity-request-fault");
    let path = dir.path().join("box.json");
    let json = format!(r#"{{"device":{{"name":"Acme Box","osc_prefix":"/b"}},"identity":{identity}}}"#);
    std::fs::write(&path, common::to05(&json)).unwrap();
    let dev = load(&path).unwrap_or_else(|e| panic!("driver refused at load: {e:#}"));
    (dir, dev)
}

/// An offset on its own place: `osd` reads it, no frame could compute it.
const BROKEN: &str = r#"{"request":[240,65,1,"{checksum:twos7@9}",247],"reply":[240]}"#;
const CLEAN: &str = r#"{"request":[240,65,"{dev}",6,"{family}",247],"device_id":{"default":16},"family":[89,1],"reply":[240]}"#;

#[test]
fn a_request_the_engine_cannot_build_is_a_fault_naming_the_driver_the_field_and_the_cause() {
    let (_d, dev) = loaded(BROKEN);
    let cause = probe_of(dev.identity.as_ref().unwrap()).unwrap_err().to_string();
    let all = faults(&dev);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].device, "Acme Box");
    assert_eq!(all[0].field, "identity.request");
    assert_eq!(all[0].cause, cause, "the text of probe_of's error, no second check");
}

#[test]
fn a_request_the_engine_can_build_gives_no_fault_and_the_same_bytes_as_before() {
    let (_d, dev) = loaded(CLEAN);
    assert!(faults(&dev).is_empty());
    let sent = programs::identity_request(&dev);
    assert_eq!(sent.bytes, [0xF0, 0x41, 0x10, 0x06, 0x59, 0x01, 0xF7]);
    assert!(sent.own);
    assert!(sent.fault.is_none());
}

#[test]
fn a_driver_without_its_own_request_gives_no_fault_and_the_universal_request() {
    for identity in [r#"{"reply":[240]}"#, r#"{"request":[240,126,127,6,1,247],"reply":[240]}"#] {
        let (_d, dev) = loaded(identity);
        assert!(faults(&dev).is_empty(), "{identity}");
        let sent = programs::identity_request(&dev);
        assert_eq!(sent.bytes, IDENTITY_REQUEST);
        assert!(!sent.own, "the universal request, not a maker's own");
        assert!(sent.fault.is_none(), "nothing was lost: {identity}");
    }
    let dir = common::temp("identity-request-none");
    let path = dir.path().join("box.json");
    std::fs::write(&path, format!(r#"{{"$schema":"{D05}","device":{{"name":"Acme Box","osc_prefix":"/b"}}}}"#)).unwrap();
    let dev = load(&path).unwrap();
    assert!(faults(&dev).is_empty());
    assert!(programs::identity_request(&dev).fault.is_none());
}

#[test]
fn a_broken_request_falls_back_to_the_universal_one_carrying_the_drivers_fault() {
    let (_d, dev) = loaded(BROKEN);
    let sent = programs::identity_request(&dev);
    assert_eq!(sent.bytes, IDENTITY_REQUEST);
    assert!(!sent.own);
    assert_eq!(sent.fault.as_ref(), faults(&dev).first(), "the same fault, signalled once");
}

#[test]
fn the_identity_address_sends_the_universal_request_for_a_broken_one() {
    let (_d, dev) = loaded(BROKEN);
    let out = programs::uniform_osc(&dev, "/b/identity", &[]).unwrap().unwrap();
    assert_eq!(out, vec![IDENTITY_REQUEST.to_vec()]);
}

#[test]
fn detection_returns_the_faults_of_the_identities_it_set_aside() {
    let (_a, broken) = loaded(BROKEN);
    let (_b, clean) = loaded(CLEAN);
    let (_c, plain) = loaded(r#"{"reply":[240]}"#);
    let found = recognition::probes([
        ("Acme Box", broken.identity.as_ref().unwrap()),
        ("Clean", clean.identity.as_ref().unwrap()),
        ("Plain", plain.identity.as_ref().unwrap()),
    ]);
    assert_eq!(found.probes.len(), 1, "only the buildable own request");
    assert_eq!(found.faults.len(), 1, "{:?}", found.faults);
    assert_eq!(found.faults[0].device, "Acme Box");
    assert_eq!(found.faults[0].field, "identity.request");
    assert_eq!(found.faults[0], faults(&broken)[0]);
}

#[test]
fn recognising_a_broken_identity_says_no_own_request_was_sent() {
    let (_d, dev) = loaded(r#"{"request":[240,65,1,"{checksum:twos7@9}",247],"reply":[240,126,127,6,2]}"#);
    let r = recognition::recognise(dev.identity.as_ref().unwrap(), "x", None, &[vec![0xF0, 0x7E, 0x7F, 6, 2, 0xF7]]).unwrap();
    assert!(r.by_reply);
    assert!(!r.own_request);
}

/// The faults detection returns for a folder of one driver (no MIDI, no probe sent).
fn detect_faults_of(identity: &str) -> Vec<osc_bridge::device::DeviceFault> {
    let (_d, dev) = loaded(identity);
    recognition::probes([("Acme Box", dev.identity.as_ref().unwrap())]).faults
}

#[test]
fn detect_hardware_tells_the_fault_of_a_broken_request_in_a_text_block_of_its_own() {
    let (_d, dev) = loaded(BROKEN);
    let cause = faults(&dev)[0].cause.clone();
    let blocks = osc_bridge::mcp::fault_blocks(&detect_faults_of(BROKEN));
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    for part in ["Fault", "Acme Box", "identity.request", cause.as_str(), "Universal"] {
        assert!(blocks[0].contains(part), "{part} missing from {}", blocks[0]);
    }
}

#[test]
fn detect_hardware_tells_no_fault_for_a_buildable_or_absent_request() {
    for identity in [CLEAN, r#"{"reply":[240]}"#] {
        let blocks = osc_bridge::mcp::fault_blocks(&detect_faults_of(identity));
        assert!(blocks.is_empty(), "{blocks:?}");
    }
    assert!(osc_bridge::mcp::fault_blocks(&[]).is_empty());
}

#[test]
fn the_run_start_lines_name_the_device_the_field_and_the_cause_of_a_fault() {
    let (_d, broken) = loaded(BROKEN);
    let cause = faults(&broken)[0].cause.clone();
    let lines = osc_bridge::runtime::fault_lines(&broken);
    assert_eq!(lines.len(), 1, "{lines:?}");
    for part in ["Acme Box", "identity.request", cause.as_str()] {
        assert!(lines[0].contains(part), "{part} missing from {}", lines[0]);
    }
    let (_e, clean) = loaded(CLEAN);
    assert!(osc_bridge::runtime::fault_lines(&clean).is_empty());
}
