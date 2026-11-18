use super::*;
use crate::export::{export, Format};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;

fn load(id: &str) -> osd::Device {
    crate::device::load(format!("tests/fixtures/devices/{id}.json")).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// The namespace the export writes, as the server answers from it.
fn namespace_of(dev: &osd::Device) -> Value {
    serde_json::from_str(&export(dev, Format::OscQuery).unwrap().value).unwrap()
}

/// The address of the first node that takes a value, depth first.
fn first_leaf(n: &Value) -> Option<String> {
    if n.get("TYPE").is_some_and(|t| t != "") {
        return n["FULL_PATH"].as_str().map(str::to_string);
    }
    n["CONTENTS"].as_object()?.values().find_map(first_leaf)
}

#[test]
fn requests_get_a_node_an_attribute_or_the_host() {
    let dev = load("novation/bass_station_ii");
    let ns = namespace_of(&dev);
    let host = host_info(std::slice::from_ref(&dev), "127.0.0.1:7777".parse().unwrap(), None);
    let leaf = first_leaf(&ns).expect("a node that takes a value");
    let type_tags = ns_node(&ns, &leaf)["TYPE"].clone();
    assert_eq!(answer(&ns, &host, "/").1.unwrap()["FULL_PATH"], "/");
    assert_eq!(answer(&ns, &host, &leaf).1.unwrap()["FULL_PATH"], leaf.as_str());
    assert_eq!(answer(&ns, &host, &format!("{leaf}?TYPE#x")).1.unwrap(), json!({ "TYPE": type_tags }));
    assert_eq!(answer(&ns, &host, &format!("{leaf}?VALUE")).0, 400);
    assert_eq!(answer(&ns, &host, "/nothing/here").0, 404);
    let h = answer(&ns, &host, "/anything?HOST_INFO").1.unwrap();
    assert_eq!((h["OSC_PORT"].as_u64(), h["OSC_IP"].as_str(), h["OSC_TRANSPORT"].as_str()),
               (Some(7777), Some("127.0.0.1"), Some("UDP")));
}

fn ns_node<'a>(ns: &'a Value, path: &str) -> &'a Value {
    path.split('/').filter(|s| !s.is_empty()).fold(ns, |n, seg| &n["CONTENTS"][seg])
}

#[test]
fn the_listener_answers_over_http() {
    let dev = load("novation/bass_station_ii");
    let local = serve("127.0.0.1:0", std::slice::from_ref(&dev), "127.0.0.1:7777".parse().unwrap(), None).unwrap();
    let get = |target: &str| -> String {
        let mut s = TcpStream::connect(local).unwrap();
        write!(s, "GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    };
    let root = get("/");
    assert!(root.starts_with("HTTP/1.1 200") && root.contains("application/json"), "{root}");
    let body: Value = serde_json::from_str(root.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["FULL_PATH"], "/");
    assert!(get("/?HOST_INFO").contains("\"OSC_PORT\":7777"));
    assert!(get("/no/such/node").starts_with("HTTP/1.1 404"));
}

fn unsourced(id: &str, prefix: &str) -> osd::Device {
    let mut dev = load(id);
    dev.sources = None;
    dev.device.osc_prefix = prefix.to_string();
    dev
}

#[test]
fn a_device_without_sources_is_named_in_faults_and_serves_nothing_under_its_prefix() {
    let bare = unsourced("novation/bass_station_ii", "/bare");
    assert!(export(&bare, Format::OscQuery).is_err(), "the export of a device without sources is refused");
    let served = namespace(std::slice::from_ref(&bare));
    assert_eq!(served.unserved_prefixes, vec!["/bare".to_string()]);
    assert_eq!(served.faults.len(), 1, "{:?}", served.faults);
    assert!(served.faults[0].contains("not served"), "{:?}", served.faults);
    assert!(served.namespace["CONTENTS"].as_object().is_none_or(|c| c.is_empty()), "{}", served.namespace);
}

#[test]
fn the_devices_with_sources_are_still_served_beside_one_without() {
    let good = load("novation/bass_station_ii");
    let bare = unsourced("arturia/microfreak", "/bare");
    let served = namespace(&[bare, good.clone()]);
    assert_eq!(served.unserved_prefixes, vec!["/bare".to_string()]);
    assert_eq!(served.namespace, namespace_of(&good), "the one device left is served as its export document");
}

#[test]
fn two_served_devices_are_merged_under_one_root() {
    let a = load("novation/bass_station_ii");
    let b = load("arturia/microfreak");
    let served = namespace(&[a.clone(), b.clone()]);
    assert!(served.faults.is_empty() && served.unserved_prefixes.is_empty());
    let desc = served.namespace["DESCRIPTION"].as_str().unwrap();
    assert!(desc.starts_with("osc-bridge serving 2 device(s): "), "{desc}");
    let root = &served.namespace["CONTENTS"];
    for dev in [&a, &b] {
        let name = dev.device.osc_prefix.trim_start_matches('/');
        assert!(root.get(name).is_some(), "{name} under the root: {:?}", root.as_object().map(|o| o.keys().collect::<Vec<_>>()));
    }
}
