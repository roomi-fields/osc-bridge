//! Firmwares that share one MIDI interface name it with `midi_of`: no copies.

use osc_bridge::device::{Device, IDENTITY};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn drivers(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            drivers(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push(p);
        }
    }
}

/// How a file speaks MIDI: everything but its identity.
fn body(doc: &Value) -> String {
    let mut b = doc.as_object().unwrap().clone();
    b.retain(|k, _| !k.starts_with('_'));
    if let Some(Value::Object(d)) = b.get_mut("device") {
        d.retain(|k, _| !IDENTITY.contains(&k.as_str()));
    }
    serde_json::to_string(&b).unwrap()
}

#[test]
fn every_link_names_a_driver_of_the_same_device() {
    let mut files = Vec::new();
    drivers(Path::new("devices"), &mut files);
    for f in files {
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        if doc.get("midi_of").is_some() {
            Device::load(&f).unwrap_or_else(|e| panic!("{e}"));
        }
    }
}

#[test]
fn no_two_drivers_of_a_device_copy_the_same_midi() {
    let mut files = Vec::new();
    drivers(Path::new("devices"), &mut files);
    let mut seen: HashMap<(String, String, String), PathBuf> = HashMap::new();
    let mut copies = Vec::new();
    for f in files {
        let Ok(doc) = serde_json::from_str::<Value>(&std::fs::read_to_string(&f).unwrap()) else { continue };
        let Some(name) = doc.pointer("/device/name").and_then(Value::as_str) else { continue };
        if doc.get("midi_of").is_some() {
            continue;
        }
        let maker = f.parent().unwrap().file_name().unwrap().to_string_lossy().to_string();
        if let Some(first) = seen.insert((maker, name.to_string(), body(&doc)), f.clone()) {
            copies.push(format!("{} copies {}", f.display(), first.display()));
        }
    }
    assert!(copies.is_empty(), "same MIDI, so one file names the other with midi_of:\n{}", copies.join("\n"));
}
