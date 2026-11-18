//! The addresses osc-bridge answers beside the ones a descriptor declares: `/bridge/*`, and for
//! each MIDI device `/identity`, `/program/*`, `/raw/syx`, `/sysex/raw`. They are osc-bridge's
//! conventions, not the device's: `osd-formats` writes the namespace of the device, this module
//! adds them to it, for the export and for the running bridge alike.

use serde_json::{json, Map, Value};

use crate::device::full_osc_path;
use osd::Device;

/// One argument of an address: its type tag, its name, its range (`{"MIN","MAX"}`, `{"VALS"}`, or Null).
struct Arg {
    tag: &'static str,
    name: &'static str,
    range: Value,
}

fn text(name: &'static str) -> Arg {
    Arg { tag: "s", name, range: Value::Null }
}

fn int(name: &'static str, lo: i64, hi: i64) -> Arg {
    Arg { tag: "i", name, range: json!({ "MIN": lo, "MAX": hi }) }
}

fn channel() -> Arg {
    int("channel", 0, 15)
}

/// The signature of the first `n` arguments.
fn signature(args: &[Arg], n: usize) -> Map<String, Value> {
    let a = &args[..n];
    let mut s = Map::new();
    s.insert("TYPE".into(), json!(a.iter().map(|x| x.tag).collect::<String>()));
    if a.iter().any(|x| !x.range.is_null()) {
        s.insert("RANGE".into(), json!(a.iter().map(|x| x.range.clone()).collect::<Vec<_>>()));
    }
    if !a.is_empty() {
        s.insert("EXTENDED_TYPE".into(), json!(a.iter().map(|x| x.name).collect::<Vec<_>>()));
    }
    s
}

/// One address: `args[..required]` must be sent, the rest may follow and is given as OVERLOADS
/// when it differs from the primary signature (`args[..primary]`).
fn method(path: &str, access: u8, args: Vec<Arg>, required: usize, primary: usize, what: &str) -> Value {
    let mut o = Map::new();
    o.insert("FULL_PATH".into(), json!(path));
    o.insert("ACCESS".into(), json!(access));
    o.extend(signature(&args, primary));
    let overloads: Vec<Value> = (required..=args.len()).filter(|n| *n != primary).map(|n| Value::Object(signature(&args, n))).collect();
    if !overloads.is_empty() {
        o.insert("OVERLOADS".into(), json!(overloads));
    }
    o.insert("DESCRIPTION".into(), json!(what));
    Value::Object(o)
}

/// Puts `node` at `path` in the namespace, the nodes on the way made when missing; an address the
/// namespace already has is left as it is.
fn put(root: &mut Value, path: &str, node: Value) {
    let mut at = root;
    let mut built = String::new();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        built.push('/');
        built.push_str(seg);
        if at.get("CONTENTS").is_none() {
            at["CONTENTS"] = json!({});
        }
        let contents = at["CONTENTS"].as_object_mut().expect("CONTENTS is an object");
        at = contents.entry(seg.to_string()).or_insert_with(|| json!({ "FULL_PATH": built, "ACCESS": 0 }));
    }
    if at["ACCESS"] == 0 && at.get("TYPE").is_none() {
        let contents = at.get("CONTENTS").cloned();
        *at = node;
        if let Some(c) = contents {
            at["CONTENTS"] = c;
        }
    }
}

/// Adds osc-bridge's own addresses to the namespace `osd-formats` wrote for `devices`.
pub fn add_to(namespace: &mut Value, devices: &[&Device]) {
    let add = |ns: &mut Value, path: &str, access, args, required, primary, what: &str| {
        put(ns, path, method(path, access, args, required, primary, what));
    };
    add(namespace, "/bridge/status", 2, vec![], 0, 0, "Ask which devices this bridge serves.");
    add(namespace, "/bridge/docs", 2, vec![], 0, 0, "Ask for each device's integration guide.");
    add(namespace, "/bridge/status/device", 1, vec![text("device"), text("status")], 2, 2, "Sent by the bridge: one served device and its status.");
    add(namespace, "/bridge/docs/device", 1, vec![text("device"), text("markdown")], 2, 2, "Sent by the bridge: one device's integration guide.");
    for d in devices {
        if d.device.transport.as_ref().is_some_and(|t| t.kind == osd::TransportKind::Osc) {
            continue;
        }
        device_routes(namespace, d);
    }
}

fn device_routes(ns: &mut Value, d: &Device) {
    let p = |rel: &str| full_osc_path(d, rel);
    let add = |ns: &mut Value, rel: &str, access, args, required, primary, what: &str| {
        let path = p(rel);
        put(ns, &path, method(&path, access, args, required, primary, what));
    };
    let banks_of = |pr: &osd::Programs| pr.banks.clone().unwrap_or_default();
    if let Some(pr) = d.programs.as_ref().filter(|pr| !banks_of(pr).is_empty()) {
        let all = banks_of(pr);
        let first = |b: &osd::Bank| b.first_number.unwrap_or(1);
        let names: Vec<&str> = all.iter().map(|b| b.name.as_str()).collect();
        let lo = all.iter().map(first).min().unwrap_or(0);
        let hi = all.iter().map(|b| first(b) + b.size - 1).max().unwrap_or(0);
        let banks: Vec<String> = all.iter().map(|b| format!("{} {}-{}", b.name, first(b), first(b) + b.size - 1)).collect();
        let selects = pr.selects.as_deref().unwrap_or("program");
        add(ns, crate::programs::RECALL, 2,
            vec![Arg { tag: "s", name: "bank", range: json!({ "VALS": names }) }, int("number", lo, hi), channel()], 2, 2,
            &format!("Recalls a stored {selects} by bank (its name, or its place from 0) and number: {}.", banks.join(", ")));
    }
    if let Some(dump) = d.programs.as_ref().and_then(|pr| pr.dump.as_ref()).filter(|x| x.request.is_some() || x.script.is_some()) {
        let names: Vec<&str> = d.programs.iter().flat_map(|pr| pr.banks.iter().flatten()).map(|b| b.name.as_str()).collect();
        let (args, what) = if dump.kind == osd::DumpKind::EditBuffer || names.is_empty() {
            (vec![channel()], "Asks the device for the sound it plays (recall the stored one first).")
        } else {
            (vec![Arg { tag: "s", name: "bank", range: json!({ "VALS": names }) }, int("number", 0, 16383), channel()],
             "Asks the device for a stored sound, by bank and number.")
        };
        let required = if args.len() == 3 { 2 } else { 0 };
        add(ns, crate::programs::DUMP, 2, args, required, required, what);
    }
    add(ns, crate::programs::IDENTITY, 2, vec![], 0, 0,
        "Sends the request the device answers with its identity (the Universal Identity Request unless its driver names another).");
    if d.identity.as_ref().is_some_and(|i| i.reply.as_ref().is_some_and(|r| !r.is_empty()) || i.script.is_some()) {
        add(ns, crate::programs::IDENTITY_REPLY, 1, vec![text("firmware"), text("hex")], 2, 2,
            "Sent by the bridge: the device's identity reply, with the firmware version read from it.");
    }
    add(ns, "/raw/syx", 2, vec![text("hex")], 1, 1, "Sends one SysEx message as it is, written in hex from F0 to F7.");
    add(ns, "/sysex/raw", 1, vec![text("hex")], 1, 1, "Sent by the bridge: SysEx from the device that no reply matched, in hex.");
}
