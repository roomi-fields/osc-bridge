//! A stand-in for the SynthHub server, on a free local port, keeping the
//! service's contract for kept drivers: each file under `/api/v1/devices/`
//! comes with its `ETag` (the SHA-256 of the file served, in hexadecimal,
//! between quotes), a request whose `If-None-Match` names it gets
//! `304 Not Modified` without a body, and a client the service marks as out
//! of date gets `Deprecation` with `Link: <url>; rel="deprecation"`
//! (RFC 9745) on every reply. It also names a device's best variant by its
//! slug (`/v1/devices/<slug>`) and gives an empty trust list. Every request
//! it receives is recorded.

#![allow(dead_code)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One request the fake server received.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub url: String,
    pub if_none_match: Option<String>,
    /// The status it was answered with.
    pub status: u16,
}

pub struct FakeSynthHub {
    pub base: String,
    pub requests: Arc<Mutex<Vec<Seen>>>,
}

impl FakeSynthHub {
    /// Serve the drivers under `root`; `deprecation`, the address of a newer
    /// osc-bridge, marks every reply as for a deprecated client.
    pub fn start(root: &Path, deprecation: Option<&str>) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (root, seen, deprecation) = (root.to_path_buf(), requests.clone(), deprecation.map(str::to_string));
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let url = req.url().to_string();
                let if_none_match = req.headers().iter().find(|h| h.field.equiv("If-None-Match")).map(|h| h.value.to_string());
                let (status, body, etag) = match url.strip_prefix("/api/v1/devices/") {
                    Some(rel) => match std::fs::read(root.join(rel)) {
                        Ok(bytes) => {
                            let etag = format!("\"{}\"", sha256_hex(&bytes));
                            if if_none_match.as_deref() == Some(etag.as_str()) {
                                (304, Vec::new(), Some(etag))
                            } else {
                                (200, bytes, Some(etag))
                            }
                        }
                        Err(_) => (404, b"not found\n".to_vec(), None),
                    },
                    None if url == "/v1/trust" => (200, br#"{"devices":{}}"#.to_vec(), None),
                    // A device's variants by its slug: the Acme one only.
                    None => match url.strip_prefix("/v1/devices/").filter(|s| root.join(format!("acme/{s}.json")).is_file()) {
                        Some(slug) => (200, serde_json::json!({ "best": format!("acme/{slug}"),
                            "variants": [{ "id": format!("acme/{slug}"), "slug": slug }] }).to_string().into_bytes(), None),
                        None => (404, b"{}".to_vec(), None),
                    },
                };
                seen.lock().unwrap().push(Seen { method: req.method().to_string(), url, if_none_match, status });
                let mut resp = tiny_http::Response::from_data(body).with_status_code(status);
                if let Some(etag) = etag {
                    resp.add_header(header("ETag", &etag));
                }
                if let Some(link) = &deprecation {
                    resp.add_header(header("Deprecation", "@1767225600"));
                    resp.add_header(header("Link", &format!("<{link}>; rel=\"deprecation\"")));
                }
                let _ = req.respond(resp);
            }
        });
        Self { base, requests }
    }

    /// The requests received so far for driver files (`/api/v1/devices/…`).
    pub fn driver_requests(&self) -> Vec<Seen> {
        self.requests.lock().unwrap().iter().filter(|s| s.url.starts_with("/api/v1/devices/")).cloned().collect()
    }

    pub fn forget_requests(&self) {
        self.requests.lock().unwrap().clear();
    }
}

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh folder for one test, under the system's temporary folder.
pub fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("osc-bridge-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A catalogue of two Acme devices: `acme/box` (with its notes, naming the
/// Acme profile in its identity block) and `acme/synth` (with its notes).
pub fn catalogue(root: &Path) {
    std::fs::create_dir_all(root.join("acme")).unwrap();
    std::fs::write(root.join("acme/box.json"), r#"{"device":{"name":"Box","vendor":"Acme","osc_prefix":"/box"},
        "identity":{"profile":"acme","family":[1,0],"member":[1,0]}}"#).unwrap();
    std::fs::write(root.join("acme/box.md"), "# Box\n\nThe box's notes.\n").unwrap();
    std::fs::write(root.join("acme/_profile.json"), r#"{"identity":{"device_id":{"default":127},
        "reply":[240,126,"{dev}",6,2,0,33,9,"{family}","{member}"]}}"#).unwrap();
    std::fs::write(root.join("acme/synth.json"), r#"{"device":{"name":"Synth","vendor":"Acme","osc_prefix":"/synth"}}"#).unwrap();
    std::fs::write(root.join("acme/synth.md"), "# Synth\n\nThe synth's notes.\n").unwrap();
}

/// Run one MCP session over stdio-like buffers, linked to the server at
/// `base`, its drivers kept in `cache`: one reply per request with an id.
pub fn mcp_session(base: &str, cache: &Path, requests: &[Value]) -> Vec<Value> {
    let opts = osc_bridge::mcp::McpOptions {
        devices_dir: None,
        default_target: "127.0.0.1:9000".parse().unwrap(),
        online: true,
        private: vec![],
    };
    let input: String = requests.iter().map(|r| format!("{r}\n")).collect();
    let mut out = Vec::new();
    osc_bridge::mcp::serve(opts, base, Some(cache.to_path_buf()), input.as_bytes(), &mut out).unwrap();
    String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

pub fn tool_call(id: u64, name: &str, args: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } })
}

pub fn initialize() -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } } })
}

/// The text of every content item of a tool call's result.
pub fn texts(reply: &Value) -> Vec<String> {
    reply["result"]["content"].as_array().into_iter().flatten()
        .filter_map(|c| c["text"].as_str().map(str::to_string)).collect()
}
