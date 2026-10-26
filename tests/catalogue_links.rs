//! Firmwares that share one MIDI interface name it with `midi_of`: no copies.

use osc_bridge::device::{resolve_midi_of, Device, IDENTITY, OWN_LISTS, OWN_TABLES};
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

/// The chain of files a driver's MIDI comes from, itself first.
fn chain(f: &Path) -> Vec<PathBuf> {
    let mut out = vec![f.to_path_buf()];
    while let Some(id) = serde_json::from_str::<Value>(&std::fs::read_to_string(out.last().unwrap()).unwrap()).ok()
        .and_then(|d| d.get("midi_of").and_then(Value::as_str).map(str::to_string)) {
        let next = Path::new("devices").join(format!("{id}.json"));
        if out.contains(&next) { break; }
        out.push(next);
    }
    out
}

/// The routes a `midi_of` file adds of its own, empty when none.
fn own_routes(doc: &Value) -> String {
    let mut own = serde_json::Map::new();
    for key in OWN_LISTS.iter().chain(OWN_TABLES.iter()) {
        let v = if OWN_LISTS.contains(key) { doc.get(*key) } else { doc.get(*key).and_then(|t| t.get("entries")) };
        if let Some(v) = v.filter(|v| v.as_array().is_some_and(|a| !a.is_empty())) {
            own.insert(key.to_string(), v.clone());
        }
    }
    if own.is_empty() { String::new() } else { serde_json::to_string(&own).unwrap() }
}

#[test]
fn no_two_drivers_of_a_device_copy_the_same_midi() {
    let mut files = Vec::new();
    drivers(Path::new("devices"), &mut files);
    files.sort();
    // Per device: each file's MIDI as it is loaded, and the routes it adds.
    let mut by_device: HashMap<(String, String), Vec<(PathBuf, String, String)>> = HashMap::new();
    for f in files {
        let Ok(doc) = serde_json::from_str::<Value>(&std::fs::read_to_string(&f).unwrap()) else { continue };
        let Some(name) = doc.pointer("/device/name").and_then(Value::as_str).map(str::to_string) else { continue };
        let maker = f.parent().unwrap().file_name().unwrap().to_string_lossy().to_string();
        let own = own_routes(&doc);
        let Ok(mut resolved) = resolve_midi_of(&f, doc) else { continue };
        resolved.as_object_mut().unwrap().remove("midi_of");
        by_device.entry((maker, name)).or_default().push((f, body(&resolved), own));
    }
    let mut copies = Vec::new();
    for list in by_device.values() {
        let body_of: HashMap<&PathBuf, &String> = list.iter().map(|(f, b, _)| (f, b)).collect();
        for (i, (a, body_a, own_a)) in list.iter().enumerate() {
            for (b, body_b, own_b) in &list[i + 1..] {
                // The same MIDI is fine when both take it from one file that has it.
                let (ca, cb) = (chain(a), chain(b));
                let shared = ca.iter().any(|c| cb.contains(c) && body_of.get(c) == Some(&body_a));
                if body_a == body_b && !shared {
                    copies.push(format!("{} has the same MIDI as {}", b.display(), a.display()));
                } else if !own_a.is_empty() && own_a == own_b {
                    copies.push(format!("{} adds the same routes as {}", b.display(), a.display()));
                }
            }
        }
    }
    copies.sort();
    assert!(copies.is_empty(), "same MIDI, so one file names the other with midi_of:\n{}", copies.join("\n"));
}
