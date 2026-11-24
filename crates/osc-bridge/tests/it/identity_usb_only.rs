//! An identity that carries only USB ids is a means of identity the recognition reads: it loads.

use crate::common;

use osc_bridge::device::load;

fn load_identity(identity: &str) -> anyhow::Result<osd::Device> {
    let dir = common::temp("identity-usb-only");
    let path = dir.path().join("box.json");
    let json = format!(r#"{{"device":{{"name":"Acme Box","osc_prefix":"/b"}},"identity":{identity}}}"#);
    std::fs::write(&path, common::to06(&json)).unwrap();
    load(&path)
}

#[test]
fn an_identity_with_only_usb_ids_loads() {
    let dev = load_identity(r#"{"usb":[{"vendor_id":7285,"product_id":516}]}"#)
        .unwrap_or_else(|e| panic!("refused: {e:#}"));
    assert!(dev.identity.is_some());
}

#[test]
fn an_identity_with_no_means_is_refused_naming_the_four() {
    let err = load_identity(r#"{"device_id":{"default":16}}"#).unwrap_err();
    let text = format!("{err:#}");
    for word in ["reply", "script", "ports", "USB"] {
        assert!(text.contains(word), "{word} missing: {text}");
    }
}
