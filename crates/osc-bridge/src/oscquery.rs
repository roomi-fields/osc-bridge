//! OSCQuery — the OSC address space the bridge answers for its devices, as
//! the JSON namespace of the OSCQuery proposal
//! (<https://github.com/Vidvox/OSCQueryProposal>), and a small HTTP listener
//! serving it next to `osc-bridge run` / `orchestrate`.
//!
//! A method a client sends to is write-only (ACCESS 2: the bridge keeps no
//! value to read back); one the bridge sends to its OSC clients (MIDI in,
//! device replies) is read-only (ACCESS 1); an address that is both is 3.
//! No VALUE and no LISTEN: the bridge holds no state, and its WebSocket
//! sends every outbound message to every client. Trailing optional
//! arguments (a channel override, a trigger's note) are given as OVERLOADS.
//! Zeroconf advertising (`_oscjson._tcp`) is not done.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use crate::device::{address_text, full_osc_path};
use osd::{Arg as ArgSpec, ArgType, CcEntry as CcParamEntry, Device, MidiType, Trigger as TriggerEntry};
use crate::export::{credits, device_title, label, sources_json};
use crate::runtime::{parse_osc_tpl, Part};

/// A path template stands for at most this many addresses; past it, it is
/// named in its device's description instead.
const MAX_EXPANDED: usize = 128;

/// One OSC argument of a method.
#[derive(Clone)]
struct Arg {
    tag: &'static str,
    name: String,
    /// `{"MIN","MAX"}` or `{"VALS"}`, Null when unbounded.
    range: Value,
}

impl Arg {
    fn new(tag: &'static str, name: &str, range: Value) -> Self { Self { tag, name: name.into(), range } }
    fn int(name: &str, lo: i64, hi: i64) -> Self { Self::new("i", name, json!({ "MIN": lo, "MAX": hi })) }
    fn channel() -> Self { Self::int("channel", 0, 15) }
}

/// What is known of one address. `args[..required]` must be sent, the rest
/// may follow; `args[..primary]` is the TYPE, other lengths are OVERLOADS.
/// `args` None: a scripted route whose arguments only its script knows.
#[derive(Default)]
struct Method {
    access: u8,
    args: Option<Vec<Arg>>,
    required: usize,
    primary: usize,
    description: Vec<String>,
    tags: Vec<String>,
}

impl Method {
    fn write(args: Vec<Arg>, required: usize, primary: usize, description: String) -> Self {
        Self { access: 2, args: Some(args), required, primary, description: vec![description], tags: vec![] }
    }
    /// All arguments sent, nothing optional.
    fn exact(access: u8, args: Vec<Arg>, description: String) -> Self {
        let n = args.len();
        Self { access, args: Some(args), required: n, primary: n, description: vec![description], tags: vec![] }
    }
    fn tagged(mut self, tags: impl IntoIterator<Item = String>) -> Self {
        self.tags.extend(tags.into_iter().filter(|t| !t.is_empty()));
        self
    }
}

#[derive(Default)]
struct Node {
    method: Option<Method>,
    children: BTreeMap<String, Node>,
    /// A device's own node (its prefix): who it is and where its data is from.
    description: Option<String>,
    sources: Option<Value>,
}

impl Node {
    fn at(&mut self, path: &str) -> &mut Node {
        path.split('/').filter(|s| !s.is_empty())
            .fold(self, |n, seg| n.children.entry(seg.to_string()).or_default())
    }

    /// Add a method; at an address already known the first arguments stay
    /// (the bridge's dispatch order) and the access and descriptions add up.
    fn add(&mut self, path: &str, m: Method) {
        let node = self.at(path);
        match &mut node.method {
            None => node.method = Some(m),
            Some(cur) => {
                cur.access |= m.access;
                if cur.args.is_none() && m.args.is_some() {
                    (cur.args, cur.required, cur.primary) = (m.args, m.required, m.primary);
                }
                cur.description.extend(m.description.into_iter().filter(|d| !d.is_empty()));
                for t in m.tags {
                    if !cur.tags.contains(&t) { cur.tags.push(t); }
                }
            }
        }
    }

    fn to_json(&self, full_path: &str) -> Value {
        let mut o = Map::new();
        o.insert("FULL_PATH".into(), json!(full_path));
        let mut text: Vec<String> = self.description.iter().cloned().collect();
        match &self.method {
            Some(m) => {
                o.insert("ACCESS".into(), json!(m.access));
                if let Some(args) = &m.args {
                    let sig = |n: usize| -> Map<String, Value> {
                        let a = &args[..n];
                        let mut s = Map::new();
                        s.insert("TYPE".into(), json!(a.iter().map(|x| x.tag).collect::<String>()));
                        if a.iter().any(|x| !x.range.is_null()) {
                            s.insert("RANGE".into(), json!(a.iter().map(|x| x.range.clone()).collect::<Vec<_>>()));
                        }
                        if !a.is_empty() {
                            s.insert("EXTENDED_TYPE".into(), json!(a.iter().map(|x| x.name.clone()).collect::<Vec<_>>()));
                        }
                        s
                    };
                    o.extend(sig(m.primary));
                    let overloads: Vec<Value> = (m.required..=args.len()).filter(|n| *n != m.primary)
                        .map(|n| Value::Object(sig(n))).collect();
                    if !overloads.is_empty() {
                        o.insert("OVERLOADS".into(), json!(overloads));
                    }
                }
                text.extend(m.description.iter().cloned());
                if !m.tags.is_empty() {
                    o.insert("TAGS".into(), json!(m.tags));
                }
            }
            None => { o.insert("ACCESS".into(), json!(0)); }
        }
        if !text.is_empty() {
            o.insert("DESCRIPTION".into(), json!(text.join(" | ")));
        }
        if let Some(s) = &self.sources {
            o.insert("SOURCES".into(), s.clone());
        }
        if !self.children.is_empty() {
            let base = full_path.trim_end_matches('/');
            let contents: Map<String, Value> = self.children.iter()
                .map(|(k, c)| (k.clone(), c.to_json(&format!("{base}/{k}")))).collect();
            o.insert("CONTENTS".into(), Value::Object(contents));
        }
        Value::Object(o)
    }
}

/// The namespace of the devices one bridge serves: every route under its
/// device's prefix, and the bridge's own `/bridge/*`. With one device the
/// root carries its description and credit too.
pub fn namespace(devices: &[Device]) -> Value {
    let mut root = Node::default();
    bridge_routes(&mut root);
    for d in devices {
        device_routes(&mut root, d);
    }
    if let [d] = devices {
        root.description = Some(device_description(d));
        root.sources = Some(sources_json(d));
    } else {
        let names: Vec<String> = devices.iter().map(device_title).collect();
        root.description = Some(format!("osc-bridge serving {} device(s): {}", devices.len(), names.join(", ")));
    }
    root.to_json("/")
}

fn device_description(d: &Device) -> String {
    format!("{} through osc-bridge. {}", device_title(d), credits(d).join(". "))
}

fn bridge_routes(root: &mut Node) {
    root.add("/bridge/status", Method::exact(2, vec![], "Ask which devices this bridge serves.".into()));
    root.add("/bridge/docs", Method::exact(2, vec![], "Ask for each device's integration guide.".into()));
    root.add("/bridge/status/device", Method::exact(1, vec![Arg::new("s", "device", Value::Null), Arg::new("s", "status", Value::Null)],
        "Sent by the bridge: one served device and its status.".into()));
    root.add("/bridge/docs/device", Method::exact(1, vec![Arg::new("s", "device", Value::Null), Arg::new("s", "markdown", Value::Null)],
        "Sent by the bridge: one device's integration guide.".into()));
}

fn device_routes(root: &mut Node, d: &Device) {
    let mut unlisted: Vec<String> = Vec::new();
    let osc = d.device.transport.as_ref().is_some_and(|t| t.kind == osd::TransportKind::Osc);
    if osc {
        osc_device_routes(root, d, &mut unlisted);
    } else {
        midi_device_routes(root, d, &mut unlisted);
    }
    let node = root.at(&d.device.osc_prefix);
    let mut text = device_description(d);
    if !unlisted.is_empty() {
        text.push_str(&format!(" Also answers: {}.", unlisted.join(", ")));
    }
    node.description = Some(text);
    node.sources = Some(sources_json(d));
}

/// The routes of a hardware (MIDI/SysEx) driver, in the order the bridge
/// tries them (`runtime::handle_message`), then what it sends back.
fn midi_device_routes(root: &mut Node, d: &Device, unlisted: &mut Vec<String>) {
    let p = |rel: &str| full_osc_path(d, rel);
    let custom = d.custom_commands.as_deref().unwrap_or_default();
    for c in custom {
        root.add(&p(&c.osc), Method { access: 2, description: vec!["Runs the driver's script, which reads the arguments.".into()], ..Default::default() });
    }
    if !custom.is_empty() {
        unlisted.push("the routes its scripts add at run time".into());
    }
    if let Some(mo) = &d.midi_out {
        let ch = mo.default_channel.unwrap_or(0) + 1;
        let note = || Arg::int("note", 0, 127);
        let u7 = |n: &str| Arg::int(n, 0, 127);
        let perf = [
            ("/note/on", vec![note(), u7("velocity"), Arg::channel()], 2, 2, "Note On"),
            ("/note/off", vec![note(), u7("velocity"), Arg::channel()], 1, 2, "Note Off (velocity 0 by default)"),
            ("/pitchbend", vec![Arg::int("value", 0, 16383), Arg::channel()], 1, 1, "Pitch bend, 8192 at rest"),
            ("/aftertouch", vec![u7("value"), Arg::channel()], 1, 1, "Channel aftertouch"),
            ("/poly_aftertouch", vec![note(), u7("value"), Arg::channel()], 2, 2, "Polyphonic aftertouch"),
            ("/program_change", vec![u7("program"), Arg::channel()], 1, 1, "Program change"),
        ];
        for (rel, args, required, primary, what) in perf {
            root.add(&p(rel), Method::write(args, required, primary, format!("{what}, on MIDI channel {ch} unless given.")));
        }
        for n in 0..=127 {
            root.add(&p(&format!("/cc/{n}")), Method::write(vec![u7("value"), Arg::channel()], 1, 1,
                format!("Control Change {n}, on MIDI channel {ch} unless given.")));
        }
    }
    if let Some(t) = &d.triggers {
        let default_ch = d.midi_out.as_ref().and_then(|m| m.default_channel).unwrap_or(0);
        for e in &t.entries {
            root.add(&p(&e.osc), trigger_method(e, default_ch));
        }
    }
    if let Some(t) = &d.cc_params {
        for e in &t.entries {
            root.add(&p(&e.osc), cc_method(e, e.channel.or(t.channel).unwrap_or(0)));
        }
    }
    if let Some(t) = &d.params {
        for e in &t.entries {
            let [lo, hi] = e.range.unwrap_or([0, 127]);
            root.add(&p(&e.osc), Method::exact(2, vec![Arg::int("value", lo, hi)],
                format!("{}: SysEx parameter {} {} {} {}.", label(&e.osc), e.pr.unwrap_or(0), e.p.unwrap_or(0), e.c.unwrap_or(0), e.r.unwrap_or(0)))
                .tagged([expects_of(e.expects.as_deref())]));
        }
    }
    let banks_of = |pr: &osd::Programs| pr.banks.clone().unwrap_or_default();
    if let Some(pr) = d.programs.as_ref().filter(|pr| !banks_of(pr).is_empty()) {
        let all = banks_of(pr);
        let first = |b: &osd::Bank| b.first_number.unwrap_or(1);
        let names: Vec<&str> = all.iter().map(|b| b.name.as_str()).collect();
        let lo = all.iter().map(first).min().unwrap_or(0);
        let hi = all.iter().map(|b| first(b) + b.size - 1).max().unwrap_or(0);
        let banks: Vec<String> = all.iter().map(|b| format!("{} {}-{}", b.name, first(b), first(b) + b.size - 1)).collect();
        let selects = pr.selects.as_deref().unwrap_or("program");
        root.add(&p(crate::programs::RECALL), Method::write(
            vec![Arg::new("s", "bank", json!({ "VALS": names })), Arg::int("number", lo, hi), Arg::channel()], 2, 2,
            format!("Recalls a stored {selects} by bank (its name, or its place from 0) and number: {}.", banks.join(", "))));
    }
    if let Some(dump) = d.programs.as_ref().and_then(|pr| pr.dump.as_ref()).filter(|x| x.request.is_some() || x.script.is_some()) {
        let names: Vec<&str> = d.programs.iter().flat_map(|pr| pr.banks.iter().flatten()).map(|b| b.name.as_str()).collect();
        let (args, what) = if dump.kind == osd::DumpKind::EditBuffer || names.is_empty() {
            (vec![Arg::channel()], "Asks the device for the sound it plays (recall the stored one first).")
        } else {
            (vec![Arg::new("s", "bank", json!({ "VALS": names })), Arg::int("number", 0, 16383), Arg::channel()],
             "Asks the device for a stored sound, by bank and number.")
        };
        let required = if args.len() == 3 { 2 } else { 0 };
        root.add(&p(crate::programs::DUMP), Method::write(args, required, required, what.into()));
    }
    // Every MIDI device: who it is, the same address whatever its maker.
    root.add(&p(crate::programs::IDENTITY), Method::exact(2, vec![],
        "Sends the request the device answers with its identity (the Universal Identity Request unless its driver names another).".into()));
    if d.identity.as_ref().is_some_and(|i| i.reply.as_ref().is_some_and(|r| !r.is_empty()) || i.script.is_some()) {
        root.add(&p(crate::programs::IDENTITY_REPLY), Method::exact(1, vec![Arg::new("s", "firmware", Value::Null), Arg::new("s", "hex", Value::Null)],
            "Sent by the bridge: the device's identity reply, with the firmware version read from it.".into()));
    }
    command_routes(root, d, false, unlisted);
    root.add(&p("/raw/syx"), Method::exact(2, vec![Arg::new("s", "hex", Value::Null)],
        "Sends one SysEx message as it is, written in hex from F0 to F7.".into()));
    if d.params.is_some() {
        root.add(&p("/param/get"), Method::exact(2, ["pr", "p", "c", "r"].iter().map(|n| Arg::int(n, 0, 127)).collect(),
            "Asks the device for a SysEx parameter's value; the reply comes as the driver's replies say.".into()));
    }

    // What the bridge sends to its OSC clients.
    let m = d.midi_in.as_ref();
    let tpl_of = |t: MidiType| crate::midi_codec::template(m, t);
    let midi_in = [
        (tpl_of(MidiType::NoteOn), "Note On"), (tpl_of(MidiType::NoteOff), "Note Off"), (tpl_of(MidiType::Cc), "Control Change"),
        (tpl_of(MidiType::Pitchbend), "Pitch bend"), (tpl_of(MidiType::Aftertouch), "Channel aftertouch"),
        (tpl_of(MidiType::PolyAftertouch), "Polyphonic aftertouch"), (tpl_of(MidiType::ProgramChange), "Program change"),
    ];
    let u7_values = |name: &str| -> Option<Vec<String>> {
        let hi = if name == "channel" { 15 } else { 127 };
        Some((0..=hi).map(|v: i32| v.to_string()).collect())
    };
    for (tpl, what) in midi_in {
        let Some(tpl) = tpl else { continue };
        emitted(root, d, &tpl, &format!("Sent by the bridge: {what} received from the device."), &u7_values, &|_| "i", unlisted);
    }
    let by_name = |name: &str| command_arg_values(d, name);
    for r in d.replies.iter().flatten() {
        emitted(root, d, &crate::device::emit_text(r), "Sent by the bridge: a reply from the device.", &by_name, &|_| "i", unlisted);
    }
    root.add(&p("/sysex/raw"), Method::exact(1, vec![Arg::new("s", "hex", Value::Null)],
        "Sent by the bridge: SysEx from the device that no reply matched, in hex.".into()));
}

/// The routes of a software (OSC) driver: its commands (or, in
/// passthrough, everything under its prefix) and the replies it relays.
fn osc_device_routes(root: &mut Node, d: &Device, unlisted: &mut Vec<String>) {
    let t = d.device.transport.as_ref();
    let target = t.map(|t| format!("{}:{}", t.host.as_deref().unwrap_or("127.0.0.1"),
        t.port.map(|p| p.to_string()).unwrap_or_else(|| "?".into()))).unwrap_or_default();
    if let Some(pt) = t.and_then(|t| t.passthrough_prefix.as_deref()) {
        unlisted.push(format!("any address under {}, sent on to {target} as {}/…",
            d.device.osc_prefix, if pt.is_empty() { "" } else { pt }));
        root.at(&d.device.osc_prefix);
    } else {
        command_routes(root, d, true, unlisted);
    }
    for r in d.replies.iter().flatten() {
        let tag_of = |name: &str| -> &'static str {
            r.match_args.iter().flatten().find(|a| a.name == name).map(|a| osc_tag(&a.ty)).unwrap_or("i")
        };
        let by_name = |name: &str| command_arg_values(d, name);
        emitted(root, d, &crate::device::emit_text(r), &format!("Sent by the bridge: relayed from {target}."), &by_name, &tag_of, unlisted);
    }
}

/// What a control or a parameter expects; continuous when it says nothing.
fn expects_of(e: Option<&str>) -> String {
    e.unwrap_or("continuous").to_string()
}

fn osc_tag(ty: &str) -> &'static str {
    match ty {
        "float" => "f",
        "string" => "s",
        "bool" => "T",
        _ => "i",
    }
}

/// The declared commands, a path template expanded over its captures.
fn command_routes(root: &mut Node, d: &Device, osc: bool, unlisted: &mut Vec<String>) {
    for c in d.commands.iter().flatten() {
        let full = full_osc_path(d, &address_text(&c.osc));
        let captures = captures(&full);
        let cargs = c.args.as_deref().unwrap_or_default();
        let args: Vec<Arg> = cargs.iter().filter(|a| !captures.contains(&a.name)).map(arg_of).collect();
        let declared: Vec<&ArgSpec> = cargs.iter().filter(|a| !captures.contains(&a.name)).collect();
        let required = declared.iter().rposition(|a| a.default.is_none()).map_or(0, |i| i + 1);
        let what = |addr: &str| {
            let name = label(addr.strip_prefix(d.device.osc_prefix.as_str()).unwrap_or(addr));
            match (&c.forward, osc) {
                (Some(f), true) => format!("{name}: sent to the target as {}.", address_text(&f.path)),
                (_, true) => format!("{name}: declared without a forward, ignored."),
                _ => format!("{name}: SysEx/MIDI command."),
            }
        };
        let values = |name: &str| cargs.iter().find(|a| a.name == name).and_then(spec_values);
        match expand(&full, &values) {
            Some(addrs) => for a in addrs {
                root.add(&a, Method::write(args.clone(), required, args.len(), what(&a)));
            },
            None => unlisted.push(full),
        }
    }
}

/// An address the bridge sends, from an `emit_osc` / `midi_in` template
/// (`/cc/{num} {value} {channel}`): the address part expanded, one argument
/// per placeholder that follows.
fn emitted(root: &mut Node, d: &Device, tpl: &str, what: &str, values: &dyn Fn(&str) -> Option<Vec<String>>,
           tag_of: &dyn Fn(&str) -> &'static str, unlisted: &mut Vec<String>) {
    let mut parts = tpl.splitn(2, ' ');
    let addr = full_osc_path(d, parts.next().unwrap_or(""));
    let args: Vec<Arg> = parts.next().unwrap_or("").split_whitespace().map(|tok| {
        let name = tok.trim_matches(|c| c == '{' || c == '}');
        Arg::new(tag_of(name), name, Value::Null)
    }).collect();
    match expand(&addr, values) {
        Some(addrs) => for a in addrs {
            root.add(&a, Method::exact(1, args.clone(), what.into()));
        },
        None => unlisted.push(format!("{addr} (sent by the bridge)")),
    }
}

fn captures(tpl: &str) -> Vec<String> {
    parse_osc_tpl(tpl).into_iter().filter_map(|p| match p { Part::Capture(c) => Some(c), _ => None }).collect()
}

/// The addresses a template stands for, each capture taking every value
/// `values` gives it; None when one has none or past [`MAX_EXPANDED`].
fn expand(tpl: &str, values: &dyn Fn(&str) -> Option<Vec<String>>) -> Option<Vec<String>> {
    let mut out = vec![String::new()];
    for part in parse_osc_tpl(tpl) {
        match part {
            Part::Literal(s) => out.iter_mut().for_each(|o| o.push_str(&s)),
            Part::Capture(name) => {
                let vals = values(&name)?;
                if vals.is_empty() || out.len() * vals.len() > MAX_EXPANDED { return None; }
                out = out.iter().flat_map(|o| vals.iter().map(move |v| format!("{o}{v}"))).collect();
            }
        }
    }
    Some(out)
}

/// The values a command argument takes in a path: its range, or the
/// numbers of its named values; none for text.
fn spec_values(a: &ArgSpec) -> Option<Vec<String>> {
    if let Some(v) = &a.values {
        return Some(v.values().map(|n| n.to_string()).collect());
    }
    let [lo, hi] = a.range.or(match a.ty { ArgType::U7 => Some([0, 127]), ArgType::Bool => Some([0, 1]), _ => None })?;
    if hi < lo || (hi - lo) as usize >= MAX_EXPANDED { return None; }
    Some((lo..=hi).map(|n| n.to_string()).collect())
}

/// A capture of an emitted address, valued as the command arguments of the
/// same name (`/track/{n}/volume` replies, `n` in 1..8).
fn command_arg_values(d: &Device, name: &str) -> Option<Vec<String>> {
    d.commands.iter().flatten().flat_map(|c| c.args.iter().flatten()).find(|a| a.name == name).and_then(spec_values)
}

fn arg_of(a: &ArgSpec) -> Arg {
    let bounded = |lo: i64, hi: i64| json!({ "MIN": lo, "MAX": hi });
    let (tag, range) = match a.ty {
        ArgType::Float => ("f", a.range.map(|[l, h]| bounded(l, h)).unwrap_or(Value::Null)),
        ArgType::String => ("s", Value::Null),
        ArgType::Bool => ("T", Value::Null),
        ArgType::Enum => ("s", a.values.as_ref().map(|v| json!({ "VALS": v.keys().collect::<Vec<_>>() })).unwrap_or(Value::Null)),
        ArgType::U7 => ("i", { let [l, h] = a.range.unwrap_or([0, 127]); bounded(l, h) }),
        ArgType::U14 => ("i", { let [l, h] = a.range.unwrap_or([0, 16383]); bounded(l, h) }),
        _ => ("i", a.range.map(|[l, h]| bounded(l, h)).unwrap_or(Value::Null)),
    };
    Arg::new(tag, &a.name, range)
}

fn cc_method(e: &CcParamEntry, ch: u8) -> Method {
    let lsb_from_call = e.nrpn_lsb_range.filter(|_| e.nrpn_msb.is_some() && e.nrpn_lsb.is_none());
    let range = e.range.unwrap_or([0, 127]);
    let address = match (e.nrpn_msb, e.nrpn_lsb, lsb_from_call, e.rpn_msb, e.rpn_lsb, e.cc) {
        (Some(m), Some(l), ..) => format!("NRPN {m}:{l}"),
        (Some(m), None, Some([lo, hi]), ..) => format!("NRPN {m}, LSB {lo}-{hi} given after the value"),
        (_, _, _, Some(m), Some(l), _) => format!("RPN {m}:{l}"),
        (.., Some(cc)) => match (e.cc_lsb, range[1] > 127) {
            (Some(lsb), true) => format!("CC {cc} + LSB CC {lsb} (14-bit)"),
            _ => format!("CC {cc}"),
        },
        _ => "no MIDI address".into(),
    };
    let mut args = vec![Arg::int("value", range[0], range[1])];
    if let Some([lo, hi]) = lsb_from_call {
        args.push(Arg::int("nrpn_lsb", lo, hi));
    }
    let required = args.len();
    args.push(Arg::channel());
    let mut what = format!("{}: {address}, on MIDI channel {} unless given.", label(&e.osc), ch + 1);
    if let Some(u) = e.remarks.get("_usage").and_then(Value::as_str) {
        what.push_str(&format!(" Values: {u}."));
    }
    Method::write(args, required, required, what).tagged([e.section.clone().unwrap_or_default(), expects_of(e.expects.as_deref())])
}

fn trigger_method(e: &TriggerEntry, default_ch: u8) -> Method {
    let [lo, hi] = e.note_range;
    let notes = match e.note {
        Some(n) if lo == hi => format!("note {n}"),
        Some(n) => format!("note {n} (plays {lo}-{hi})"),
        None => format!("notes {lo}-{hi}, the note given"),
    };
    let mode = crate::device::channel_mode_name(e.channel_mode);
    let channel = match (e.channel, mode) {
        (Some(c), _) => format!("MIDI channel {}", c + 1),
        (None, mode) => format!("the device's {mode} channel (MIDI channel {} unless given)", default_ch + 1),
    };
    let mut what = format!("{}: {notes} on {channel}.", e.sound);
    for extra in [&e.velocity_usage, &e.comments].into_iter().flatten() {
        what.push_str(&format!(" {extra}"));
    }
    let args = vec![Arg::int("velocity", 1, 127), Arg::int("note", lo, hi), Arg::channel()];
    let (required, primary) = if e.note.is_some() { (0, 1) } else { (2, 2) };
    Method::write(args, required, primary, what).tagged(e.section.clone())
}

/// HOST_INFO of a bridge whose OSC is received on `osc` (UDP), and on `ws`
/// (raw OSC in binary frames) when the WebSocket is on.
pub fn host_info(devices: &[Device], osc: SocketAddr, ws: Option<SocketAddr>) -> Value {
    let names: Vec<String> = devices.iter().map(device_title).collect();
    let mut h = json!({
        "NAME": format!("osc-bridge: {}", names.join(", ")),
        "OSC_PORT": osc.port(),
        "OSC_TRANSPORT": "UDP",
        "EXTENSIONS": {
            "ACCESS": true, "RANGE": true, "DESCRIPTION": true, "TAGS": true,
            "EXTENDED_TYPE": true, "OVERLOADS": true, "VALUE": false, "LISTEN": false,
        },
    });
    if !osc.ip().is_unspecified() {
        h["OSC_IP"] = json!(osc.ip().to_string());
    }
    if let Some(ws) = ws {
        h["WS_PORT"] = json!(ws.port());
        if !ws.ip().is_unspecified() { h["WS_IP"] = json!(ws.ip().to_string()); }
    }
    h
}

/// Attributes a node may be asked for one by one (`/path?RANGE`).
const ATTRIBUTES: [&str; 11] = ["FULL_PATH", "CONTENTS", "TYPE", "ACCESS", "RANGE", "DESCRIPTION", "TAGS",
    "EXTENDED_TYPE", "OVERLOADS", "SOURCES", "HOST_INFO"];

/// The answer to one request target (`/minilab3/cc/74?RANGE#x`): HTTP status
/// and body. `/` is the whole namespace, a path its node and what is under
/// it, `?HOST_INFO` the host, `?ATTRIBUTE` that attribute of the node.
pub fn answer(namespace: &Value, host_info: &Value, target: &str) -> (u16, Option<Value>) {
    let target = target.split('#').next().unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if query == "HOST_INFO" {
        return (200, Some(host_info.clone()));
    }
    let path = percent_decode(path);
    let mut node = namespace;
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        match node.get("CONTENTS").and_then(|c| c.get(seg)) {
            Some(n) => node = n,
            None => return (404, None),
        }
    }
    if query.is_empty() {
        return (200, Some(node.clone()));
    }
    if !ATTRIBUTES.contains(&query) {
        return (400, None);
    }
    match node.get(query) {
        Some(v) => (200, Some(json!({ query: v }))),
        None => (204, None),
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(v)) => { out.push(v); i += 3; }
            (c, _) => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Serve the namespace of `devices` over HTTP on `bind`. Returns the bound
/// address; requests are answered on background threads.
pub fn serve(bind: &str, devices: &[Device], osc: SocketAddr, ws: Option<SocketAddr>) -> Result<SocketAddr> {
    let listener = TcpListener::bind(bind).with_context(|| format!("bind OSCQuery {bind}"))?;
    let local = listener.local_addr()?;
    let shared = std::sync::Arc::new((namespace(devices), host_info(devices, osc, ws)));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let shared = shared.clone();
            std::thread::spawn(move || {
                if let Err(e) = handle(stream, &shared.0, &shared.1) {
                    eprintln!("OSCQuery: {e}");
                }
            });
        }
    });
    Ok(local)
}

fn handle(mut stream: TcpStream, namespace: &Value, host_info: &Value) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        let n = stream.read(&mut chunk)?;
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or("/"));
    let (status, body) = match method {
        "GET" | "HEAD" => answer(namespace, host_info, target),
        _ => (405, None),
    };
    let reason = match status { 200 => "OK", 204 => "No Content", 400 => "Bad Request", 404 => "Not Found", _ => "Method Not Allowed" };
    let body = body.map(|b| serde_json::to_string(&b).unwrap_or_default()).unwrap_or_default();
    let mut resp = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
    if status != 204 {
        resp.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    resp.push_str("\r\n");
    if method == "GET" { resp.push_str(&body); }
    stream.write_all(resp.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests;
