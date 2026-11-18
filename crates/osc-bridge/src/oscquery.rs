//! OSCQuery — a small HTTP listener serving the OSC address space the bridge
//! answers for its devices, next to `osc-bridge run` / `orchestrate`. The
//! namespace is the JSON document of the OSCQuery proposal
//! (<https://github.com/Vidvox/OSCQueryProposal>), written by `osd-formats`
//! through `crate::export::export`: what the listener answers is the
//! document the export gives. The listener adds HOST_INFO.
//! Zeroconf advertising (`_oscjson._tcp`) is not done.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use crate::export::{device_title, export, Format};
use osd::Device;

/// The namespace a bridge serves, and the devices it cannot serve one for.
pub struct Served {
    pub namespace: Value,
    /// One line per device left out: the device and the fault `osd-formats` gives.
    pub faults: Vec<String>,
    /// The prefixes of those devices.
    pub unserved_prefixes: Vec<String>,
}

/// The namespace of the devices one bridge serves: the export document of
/// each, their nodes merged under one root. With one device it is that
/// device's export document. A device whose document cannot be written (no
/// `_sources`) is left out and named in `faults`; the others are served.
pub fn namespace(devices: &[Device]) -> Served {
    let mut docs = Vec::new();
    let (mut faults, mut unserved_prefixes) = (Vec::new(), Vec::new());
    let mut served = Vec::new();
    for d in devices {
        match export(d, Format::OscQuery).and_then(|out| Ok(serde_json::from_str::<Value>(&out.value)?)) {
            Ok(doc) => {
                docs.push(doc);
                served.push(d);
            }
            Err(e) => {
                faults.push(format!("Fault:   {}: OSCQuery namespace not served: {e}", device_title(d)));
                unserved_prefixes.push(d.device.osc_prefix.clone());
            }
        }
    }
    let devices = served;
    let mut docs = docs.into_iter();
    let mut root = docs.next().unwrap_or_else(|| json!({ "FULL_PATH": "/", "ACCESS": 0 }));
    let rest: Vec<Value> = docs.collect();
    if !rest.is_empty() {
        let names: Vec<String> = devices.iter().map(|d| device_title(d)).collect();
        root["DESCRIPTION"] = json!(format!("osc-bridge serving {} device(s): {}", devices.len(), names.join(", ")));
        if let Some(o) = root.as_object_mut() {
            o.remove("SOURCES");
        }
        for doc in rest {
            merge_contents(&mut root, &doc);
        }
    }
    Served { namespace: root, faults, unserved_prefixes }
}

/// Adds the children of `from` under `into`, level by level.
fn merge_contents(into: &mut Value, from: &Value) {
    let Some(children) = from.get("CONTENTS").and_then(Value::as_object) else { return };
    if into.get("CONTENTS").is_none() {
        into["CONTENTS"] = json!({});
    }
    let target = into["CONTENTS"].as_object_mut().expect("CONTENTS is an object");
    for (name, child) in children {
        match target.get_mut(name) {
            Some(existing) => merge_contents(existing, child),
            None => {
                target.insert(name.clone(), child.clone());
            }
        }
    }
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
    let served = namespace(devices);
    for line in &served.faults {
        eprintln!("{line}");
    }
    let shared = std::sync::Arc::new((served.namespace, host_info(devices, osc, ws)));
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
