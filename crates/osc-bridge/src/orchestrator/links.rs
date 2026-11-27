//! The orchestrator's links: one table, changed while it runs. The
//! configuration file and the agent tools write to it through the same entry
//! points (`LinkTable::new`, `insert`, `remove`); a message reads it through a
//! shared snapshot.
//!
//! Control addresses, answered to whoever asks, as `/bridge/status` is:
//!  - `/bridge/link/add <from> <to> [in_lo in_hi out_lo out_hi]` →
//!    `/bridge/link/added <from> <to>` or `/bridge/link/error <text>`
//!  - `/bridge/link/remove <from> <to>` → `/bridge/link/removed <from> <to>`
//!    or `/bridge/link/error <text>`
//!  - `/bridge/link/list` → one `/bridge/link/list/item <from> <to> <origin>
//!    [in_lo in_hi out_lo out_hi]` per link (origin `file` or `live`), then
//!    `/bridge/link/list/end <count>`

use rosc::OscMessage;
use std::sync::{Arc, RwLock};

use super::{LoadedDevice, MapRule, RouteRule, ROUTE_MAX_DEPTH};
use crate::runtime::{match_osc_path, parse_osc_tpl, Part};
use osd::Device;

/// Where a link comes from: the configuration file, or an agent while the
/// orchestrator runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Origin {
    File,
    Live,
}

impl Origin {
    fn as_str(self) -> &'static str {
        match self {
            Origin::File => "file",
            Origin::Live => "live",
        }
    }
}

/// Compiled form of a RouteRule, pattern parsed once when the link is made.
pub(super) struct CompiledRoute {
    pub(super) from: String,
    pub(super) from_parts: Vec<Part>,
    pub(super) to_template: String,
    pub(super) map: Option<MapRule>,
    origin: Origin,
}

impl CompiledRoute {
    fn new(r: &RouteRule, origin: Origin) -> Self {
        Self {
            from: r.from.clone(),
            from_parts: parse_osc_tpl(&r.from),
            to_template: r.to.clone(),
            map: r.map.clone(),
            origin,
        }
    }
}

/// The table of links. A change never waits for a message in flight, and a
/// message sees a whole table, from before or after the change: readers take
/// an `Arc` snapshot, writers swap in a new table.
pub struct LinkTable {
    current: RwLock<Arc<Vec<Arc<CompiledRoute>>>>,
}

impl LinkTable {
    /// A table holding the links of the configuration file.
    pub fn new(file_links: &[RouteRule]) -> Self {
        let all = file_links.iter().map(|r| Arc::new(CompiledRoute::new(r, Origin::File))).collect();
        Self { current: RwLock::new(Arc::new(all)) }
    }

    /// The table as it is now; later changes do not alter it.
    pub(super) fn snapshot(&self) -> Arc<Vec<Arc<CompiledRoute>>> {
        self.current.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Add a link made live.
    pub fn insert(&self, rule: RouteRule) {
        let _ = self.insert_checked(rule, |_| Ok(()));
    }

    /// Add a link made live once `check` accepts the table it would join. The
    /// check and the change are one step: two additions cannot both pass
    /// against the same table.
    fn insert_checked(
        &self,
        rule: RouteRule,
        check: impl FnOnce(&[Arc<CompiledRoute>]) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut cur = self.current.write().unwrap_or_else(|e| e.into_inner());
        check(&cur)?;
        let mut next: Vec<Arc<CompiledRoute>> = cur.as_ref().clone();
        next.push(Arc::new(CompiledRoute::new(&rule, Origin::Live)));
        *cur = Arc::new(next);
        Ok(())
    }

    /// Remove the link from `from` to `to`; false when there is none.
    pub fn remove(&self, from: &str, to: &str) -> bool {
        let mut cur = self.current.write().unwrap_or_else(|e| e.into_inner());
        let next: Vec<Arc<CompiledRoute>> =
            cur.iter().filter(|r| !(r.from == from && r.to_template == to)).cloned().collect();
        if next.len() == cur.len() {
            return false;
        }
        *cur = Arc::new(next);
        true
    }
}

fn is_reachable_address(n: &serde_json::Value) -> bool {
    n.get("ACCESS").and_then(serde_json::Value::as_i64).is_some_and(|a| a != 0)
}

/// What the devices' drivers have, to judge a link made live: the OSCQuery
/// namespace of the devices, the prefixes whose every address is passed on,
/// and the prefixes that own an address.
struct KnownAddresses {
    namespace: serde_json::Value,
    /// Prefixes of the devices with no namespace: their addresses are not judged.
    unjudged: Vec<String>,
    passthrough: Vec<String>,
    prefixes: Vec<String>,
}

impl KnownAddresses {
    fn new(devices: &[LoadedDevice]) -> Self {
        let devs: Vec<Device> = devices.iter().map(|d| d.device().clone()).collect();
        // A device with no namespace (no `sources`, faulted when the namespace is served) has
        // no address to judge a link by: what lies under its prefix is passed as it is.
        let served = crate::oscquery::namespace(&devs);
        Self {
            namespace: served.namespace,
            unjudged: served.unserved_prefixes,
            passthrough: devs
                .iter()
                .filter(|d| d.device.transport.as_ref().is_some_and(|t| t.passthrough_prefix.is_some()))
                .map(|d| d.device.osc_prefix.clone())
                .collect(),
            prefixes: devs.iter().map(|d| d.device.osc_prefix.clone()).collect(),
        }
    }

    fn owner(&self, addr: &str) -> Option<&str> {
        self.prefixes.iter().filter(|p| addr.starts_with(p.as_str())).max_by_key(|p| p.len()).map(String::as_str)
    }

    fn has(&self, addr: &str) -> bool {
        if self.passthrough.iter().chain(&self.unjudged).any(|p| addr.starts_with(p.as_str())) {
            return true;
        }
        let mut node = &self.namespace;
        for seg in addr.split('/').skip(1) {
            match node.get("CONTENTS").and_then(|c| c.get(seg)) {
                Some(n) => node = n,
                None => return false,
            }
        }
        is_reachable_address(node)
    }

    /// True when a pattern with captures matches some address of the drivers.
    fn any_matches(&self, parts: &[Part]) -> bool {
        fn walk(n: &serde_json::Value, parts: &[Part]) -> bool {
            if is_reachable_address(n) {
                if let Some(p) = n.get("FULL_PATH").and_then(serde_json::Value::as_str) {
                    if match_osc_path(parts, p).is_some() {
                        return true;
                    }
                }
            }
            n.get("CONTENTS")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|c| c.values().any(|v| walk(v, parts)))
        }
        walk(&self.namespace, parts)
    }

    /// Refuse an address that no device owns, or that its driver does not have.
    fn check(&self, addr: &str, role: &str) -> Result<(), String> {
        if !addr.starts_with('/') {
            return Err(format!("{role} {addr} is not an OSC address: it starts with '/'"));
        }
        let Some(owner) = self.owner(addr) else {
            return Err(format!("no device owns {addr} ({role} of the link)"));
        };
        if addr.contains('{') {
            // A pattern: a source must match some address; a target is filled in while a message runs.
            if role == "source" && !self.any_matches(&parse_osc_tpl(addr)) {
                return Err(format!("{addr} matches no address of the drivers ({role} of the link)"));
            }
            return Ok(());
        }
        if !self.has(addr) {
            return Err(format!("{addr} is not an address the driver of {owner} has ({role} of the link)"));
        }
        Ok(())
    }
}

/// The longest chain of concrete links through `from -> to` once it is added,
/// or the loop it closes (second value true). Links with captures are left to
/// the cut at four hops that applies while a message runs.
fn chain_through(table: &[Arc<CompiledRoute>], from: &str, to: &str) -> (Vec<String>, bool) {
    let edges: Vec<(&str, &str)> = table
        .iter()
        .filter(|r| !r.from.contains('{') && !r.to_template.contains('{'))
        .map(|r| (r.from.as_str(), r.to_template.as_str()))
        .collect();
    if from == to {
        return (vec![from.to_string(), to.to_string()], true);
    }
    let (ahead, looped) = longest(&edges, to, true, &mut vec![to], Some(from));
    if looped {
        let mut chain = vec![from.to_string()];
        chain.extend(ahead.iter().map(|s| s.to_string()));
        return (chain, true);
    }
    let (back, _) = longest(&edges, from, false, &mut vec![from], None);
    let mut chain: Vec<String> = back.iter().rev().map(|s| s.to_string()).collect();
    chain.extend(ahead.iter().map(|s| s.to_string()));
    (chain, false)
}

/// The longest path from `at` along the links (`forward`) or against them,
/// never visiting an address twice; stops at once, flagged, on reaching `stop`.
fn longest<'a>(
    edges: &[(&'a str, &'a str)],
    at: &'a str,
    forward: bool,
    path: &mut Vec<&'a str>,
    stop: Option<&str>,
) -> (Vec<&'a str>, bool) {
    let mut best = path.clone();
    for (a, b) in edges {
        let (here, next) = if forward { (*a, *b) } else { (*b, *a) };
        if here != at {
            continue;
        }
        if stop == Some(next) {
            let mut closed = path.clone();
            closed.push(next);
            return (closed, true);
        }
        if path.contains(&next) {
            continue;
        }
        path.push(next);
        let (found, looped) = longest(edges, next, forward, path, stop);
        path.pop();
        if looped {
            return (found, true);
        }
        if found.len() > best.len() {
            best = found;
        }
    }
    (best, false)
}

/// The table and what judges a change to it: the one entry point of the
/// control addresses.
pub(super) struct Links {
    pub(super) table: LinkTable,
    known: KnownAddresses,
}

fn number(a: &rosc::OscType) -> Option<f64> {
    match a {
        rosc::OscType::Int(i) => Some(f64::from(*i)),
        rosc::OscType::Long(i) => Some(*i as f64),
        rosc::OscType::Float(f) => Some(f64::from(*f)),
        rosc::OscType::Double(f) => Some(*f),
        _ => None,
    }
}

fn link_error(text: String) -> Vec<OscMessage> {
    vec![OscMessage { addr: "/bridge/link/error".into(), args: vec![rosc::OscType::String(text)] }]
}

fn two_texts(msg: &OscMessage) -> Option<(String, String)> {
    match (msg.args.first(), msg.args.get(1)) {
        (Some(rosc::OscType::String(a)), Some(rosc::OscType::String(b))) => Some((a.clone(), b.clone())),
        _ => None,
    }
}

const SCALING_HELP: &str = "the scaling is four numbers: input lo hi, output lo hi";

impl Links {
    pub(super) fn new(file_links: &[RouteRule], devices: &[LoadedDevice]) -> Self {
        Self { table: LinkTable::new(file_links), known: KnownAddresses::new(devices) }
    }

    /// Answer `/bridge/link/add|remove|list`; None for any other address.
    pub(super) fn control(&self, msg: &OscMessage) -> Option<Vec<OscMessage>> {
        use rosc::OscType::{Float, Int, String as Str};
        let reply = |addr: &str, args: Vec<rosc::OscType>| vec![OscMessage { addr: addr.into(), args }];
        match msg.addr.as_str() {
            "/bridge/link/list" => {
                let snap = self.table.snapshot();
                let mut out: Vec<OscMessage> = snap
                    .iter()
                    .map(|r| {
                        let mut args =
                            vec![Str(r.from.clone()), Str(r.to_template.clone()), Str(r.origin.as_str().into())];
                        if let Some(m) = &r.map {
                            args.extend(m.from.iter().chain(m.to.iter()).map(|x| Float(*x as f32)));
                        }
                        OscMessage { addr: "/bridge/link/list/item".into(), args }
                    })
                    .collect();
                out.push(OscMessage { addr: "/bridge/link/list/end".into(), args: vec![Int(snap.len() as i32)] });
                Some(out)
            }
            "/bridge/link/remove" => {
                let Some((from, to)) = two_texts(msg) else {
                    return Some(link_error("remove needs the source and the target addresses".into()));
                };
                Some(if self.table.remove(&from, &to) {
                    reply("/bridge/link/removed", vec![Str(from), Str(to)])
                } else {
                    link_error(format!("no link from {from} to {to}"))
                })
            }
            "/bridge/link/add" => {
                let Some((from, to)) = two_texts(msg) else {
                    return Some(link_error("add needs the source and the target addresses".into()));
                };
                let map = match &msg.args[2..] {
                    [] => None,
                    [a, b, c, d] => match (number(a), number(b), number(c), number(d)) {
                        (Some(a), Some(b), Some(c), Some(d)) => Some(MapRule { from: [a, b], to: [c, d] }),
                        _ => return Some(link_error(SCALING_HELP.into())),
                    },
                    _ => return Some(link_error(SCALING_HELP.into())),
                };
                let rule = RouteRule { from: from.clone(), to: to.clone(), map };
                let done = self
                    .known
                    .check(&from, "source")
                    .and_then(|()| self.known.check(&to, "target"))
                    .and_then(|()| self.table.insert_checked(rule, |table| refuse_chain(table, &from, &to)));
                Some(match done {
                    Ok(()) => reply("/bridge/link/added", vec![Str(from), Str(to)]),
                    Err(e) => link_error(e),
                })
            }
            _ => None,
        }
    }
}

/// Refuse a link already there, one that closes a loop, or one that makes a
/// chain of more than four hops with the links already present.
fn refuse_chain(table: &[Arc<CompiledRoute>], from: &str, to: &str) -> Result<(), String> {
    if table.iter().any(|r| r.from == from && r.to_template == to) {
        return Err(format!("{from} is already linked to {to}"));
    }
    let (chain, looped) = chain_through(table, from, to);
    let shown = chain.join(" -> ");
    if looped {
        return Err(format!("the link would close a loop: {shown}"));
    }
    let hops = chain.len() - 1;
    if hops > ROUTE_MAX_DEPTH as usize {
        return Err(format!("the link would make a chain of {hops} hops, over {ROUTE_MAX_DEPTH}: {shown}"));
    }
    Ok(())
}
