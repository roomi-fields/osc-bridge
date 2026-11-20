//! The documents `export` writes, against reference facts taken from the exporters that
//! osc-bridge held before osd-formats replaced them: each fact of the reference must still be
//! in the document. A difference is either corrected upstream or accepted and written in
//! docs/EXPORT.md.

use osc_bridge::export::{export, Format};

const OSCQUERY: Format = Format(osd_formats::Format::OscQuery);
const MIDNAM: Format = Format(osd_formats::Format::Midnam);
use serde_json::Value;
use std::collections::BTreeSet;

fn load(id: &str) -> osd::Device {
    osc_bridge::device::load(format!("tests/fixtures/devices/{id}.json")).unwrap_or_else(|e| panic!("{id}: {e:#}"))
}

fn canon(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            format!("{{{}}}", keys.iter().map(|k| format!("{k:?}:{}", canon(&m[*k]))).collect::<Vec<_>>().join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canon).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// One line per node that has a type: its address, access, type tags, ranges and extended types,
/// and the same for each overload.
fn oscquery_facts(text: &str) -> BTreeSet<String> {
    fn walk(n: &Value, out: &mut BTreeSet<String>) {
        if let Some(path) = n["FULL_PATH"].as_str() {
            if n.get("TYPE").is_some() {
                let get = |v: &Value, k: &str| v.get(k).map(canon).unwrap_or_default();
                out.insert(format!("{path} access={} type={} range={} ext={}", get(n, "ACCESS"), get(n, "TYPE"), get(n, "RANGE"), get(n, "EXTENDED_TYPE")));
                for o in n["OVERLOADS"].as_array().into_iter().flatten() {
                    out.insert(format!("{path} overload type={} range={} ext={}", get(o, "TYPE"), get(o, "RANGE"), get(o, "EXTENDED_TYPE")));
                }
            }
        }
        for c in n["CONTENTS"].as_object().into_iter().flat_map(|m| m.values()) {
            walk(c, out);
        }
    }
    let mut out = BTreeSet::new();
    walk(&serde_json::from_str::<Value>(text).expect("OSCQuery document is JSON"), &mut out);
    out
}

/// One line per manufacturer, model, control (with its named values), bank and note.
fn midnam_facts(text: &str) -> BTreeSet<String> {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, opts).unwrap_or_else(|e| panic!("MIDNAM not well-formed: {e}"));
    let mut out = BTreeSet::new();
    for n in doc.descendants().filter(|n| n.is_element()) {
        let a = |k: &str| n.attribute(k).unwrap_or_default();
        match n.tag_name().name() {
            "Manufacturer" => {
                out.insert(format!("Manufacturer {}", n.text().unwrap_or_default()));
            }
            "Control" => {
                // The named values are inline, or in a ValueNameList the control uses by name.
                let used = n.descendants().find(|u| u.has_tag_name("UsesValueNameList")).and_then(|u| u.attribute("Name"));
                let list = used.and_then(|name| doc.descendants().find(|l| l.has_tag_name("ValueNameList") && l.attribute("Name") == Some(name)));
                let mut values: Vec<String> = n.descendants().chain(list.into_iter().flat_map(|l| l.descendants())).filter(|v| v.has_tag_name("Value")).map(|v| format!("{}={}", v.attribute("Number").unwrap_or_default(), v.attribute("Name").unwrap_or_default())).collect();
                values.sort();
                assert!(!a("Name").is_empty(), "a control is named: {n:?}");
                out.insert(format!("Control type={} number={} values=[{}]", a("Type"), a("Number"), values.join(";")));
            }
            "PatchBank" => {
                let patches = n.descendants().filter(|v| v.has_tag_name("Patch")).count();
                out.insert(format!("PatchBank name={} patches={patches}", a("Name")));
            }
            "Note" => {
                out.insert(format!("Note number={} name={}", a("Number"), a("Name")));
            }
            _ => {}
        }
    }
    out
}

fn reference(slug: &str, ext: &str) -> BTreeSet<String> {
    let path = format!("tests/fixtures/exports/{slug}.{ext}.facts");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")).lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn assert_keeps(id: &str, format: Format, ext: &str) {
    let slug = id.replace('/', "_");
    let doc = export(&load(id), format).unwrap_or_else(|e| panic!("{id}: {e}")).value;
    let now = if ext == "oscquery" { oscquery_facts(&doc) } else { midnam_facts(&doc) };
    let missing: Vec<String> = reference(&slug, ext).difference(&now).cloned().collect();
    assert!(missing.is_empty(), "{id} ({ext}): {} facts of the reference are no longer in the document:\n{}", missing.len(), missing.join("\n"));
}

#[test]
fn an_oscquery_document_keeps_every_node_the_reference_has() {
    assert_keeps("acme/box.vendor.fw-2", OSCQUERY, "oscquery");
}

#[test]
fn a_midnam_document_keeps_its_controls_values_and_banks() {
    assert_keeps("acme/box.vendor.fw-2", MIDNAM, "midnam");
    assert_keeps("arturia/microfreak.vendor.fw-microfreak-tools-0.1.0", MIDNAM, "midnam");
}

#[test]
fn a_midnam_document_keeps_the_notes_a_drum_machine_names() {
    assert_keeps("elektron/model_samples", MIDNAM, "midnam");
}

#[test]
fn both_documents_carry_the_credit_and_licence_of_the_sources() {
    let acme = load("acme/box.vendor.fw-2");
    let ns: Value = serde_json::from_str(&export(&acme, OSCQUERY).unwrap().value).unwrap();
    let node = ns["CONTENTS"]["box"]["DESCRIPTION"].as_str().expect("the device's node carries the credit");
    assert!(node.contains("CC0-1.0") && node.contains("osc-bridge test fixture"), "{node}");
    let xml = export(&acme, MIDNAM).unwrap().value;
    assert!(xml.contains("CC0-1.0") && xml.contains("osc-bridge test fixture"), "{xml}");

    let shared = load("arturia/microfreak.vendor.fw-microfreak-tools-0.1.0");
    for format in [OSCQUERY, MIDNAM] {
        let text = export(&shared, format).unwrap().value;
        assert!(text.contains("Apache-2.0") && text.contains("CC-BY-SA-4.0"), "{format:?}: the sources of the file whose MIDI it shares are credited too");
    }
}

#[test]
fn the_running_bridge_answers_the_namespace_the_export_writes() {
    use std::io::{Read, Write};
    let dev = load("acme/box.vendor.fw-2");
    let local = osc_bridge::oscquery::serve("127.0.0.1:0", std::slice::from_ref(&dev), "127.0.0.1:7777".parse().unwrap(), None).unwrap();
    let mut s = std::net::TcpStream::connect(local).unwrap();
    write!(s, "GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut reply = String::new();
    s.read_to_string(&mut reply).unwrap();
    let served = oscquery_facts(reply.split("\r\n\r\n").nth(1).unwrap());
    let exported = oscquery_facts(&export(&dev, OSCQUERY).unwrap().value);
    assert!(!exported.is_empty());
    for line in &exported {
        assert!(served.contains(line), "the running bridge does not serve: {line}");
    }
    for line in reference("acme_box.vendor.fw-2", "oscquery").iter().filter(|l| l.starts_with("/box/")) {
        assert!(served.contains(line), "the running bridge lost: {line}");
    }
}

#[test]
fn a_device_that_speaks_only_osc_has_no_midnam_and_says_so() {
    let dev = load("bitwig/bitwig-studio.third-party-osc.fw-5");
    let err = export(&dev, MIDNAM).unwrap_err().to_string();
    assert_eq!(err, "Bitwig Bitwig Studio (Bitwig Studio 5 + DrivenByMoss (latest)) speaks OSC, not MIDI: it has no MIDI names");
}

#[test]
fn an_unknown_format_name_keeps_its_message() {
    let err = "ctrlr".parse::<Format>().unwrap_err().to_string();
    assert!(err.starts_with("unknown format 'ctrlr' (one of: "), "{err}");
    for name in osc_bridge::export::format_names() {
        assert!(err.contains(name), "{name} missing from {err}");
    }
    assert!(err.contains("oscquery") && err.contains("midnam") && err.contains("deluge"), "{err}");
}

#[test]
fn what_the_formats_could_not_carry_comes_with_the_document() {
    let dev = load("arturia/microfreak.vendor.fw-microfreak-tools-0.1.0");
    // The firmware of a source has no place in a MIDNAM or an OSCQuery document: osd-formats lists it.
    for format in [MIDNAM, OSCQUERY] {
        let out = export(&dev, format).unwrap();
        assert!(!out.value.is_empty());
        assert!(out.lost.iter().any(|l| l.place == "/_sources/0/firmware"), "{format:?}: {:?}", out.lost);
    }
}
