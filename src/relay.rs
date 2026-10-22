//! Relay: lets an agent reach the MIDI ports of a machine it does not run on.
//!
//! A "door" — the `/connect` page in a browser (Web MIDI), `osc-bridge
//! connect`, a plugin, any program speaking this protocol — opens a
//! WebSocket to the hosted server. The server gives the session a pairing
//! code; the user hands that code to their agent, whose MCP calls
//! (`relay_*` tools) then travel through the server to the door and on to
//! the hardware. The server does all the work — it turns named OSC
//! addresses into MIDI bytes with the catalogue's drivers — and the door only
//! moves bytes between the WebSocket and its MIDI ports.
//!
//! Protocol: JSON text frames.
//! - server → door: `{"type":"hello","code","user"}` once, then
//!   `{"type":"midi_out","port","data":[bytes]}` for each message to send.
//! - door → server: `{"type":"ports","outputs":[names],"inputs":[names]}`
//!   whenever its ports change, `{"type":"midi_in","port","data":[bytes]}`
//!   for each message a device sends, and optionally
//!   `{"type":"share","enabled":bool}` (default true): whether the device
//!   identities seen in the session may be recorded for the catalogue.
//!
//! The door connects with `?user=<id>` to keep its account; an id the server
//! did not issue gets a new account. A session lives as long as its socket.
//!
//! Who may drive a door: whoever holds its pairing code (about 40 random
//! bits, new for each session) — and once an agent account has used it, that
//! account only (`Session::claim`); the door is told, with
//! `{"type":"paired"}`, and a new session gives a new code. The server
//! itself is protected by caps on doors: `MAX_DOORS` in all,
//! `MAX_DOORS_PER_CLIENT` per network address.

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{HandshakeError, Message, WebSocket};

/// Letters and digits that cannot be confused when read aloud or copied.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// MIDI messages from the door kept per session for `relay_listen`.
const INBOUND_KEPT: usize = 4096;

/// Outbound frames queued per session before the door is considered stuck.
const OUTBOUND_QUEUE: usize = 1024;

const READ_TIMEOUT: Duration = Duration::from_millis(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Doors served at once, handshakes included: each holds a thread, and the
/// container's thread budget is shared with the HTTP server.
pub const MAX_DOORS: usize = 64;

/// Doors one network address may hold at once.
pub const MAX_DOORS_PER_CLIENT: usize = 4;

/// A ping every so often keeps proxies from closing a quiet session.
const KEEPALIVE: Duration = Duration::from_secs(30);

/// One MIDI message received from the door.
#[derive(Clone, Debug)]
pub struct Inbound {
    pub at: Instant,
    pub port: String,
    pub data: Vec<u8>,
}

/// A connected door.
pub struct Session {
    pub code: String,
    pub user: String,
    outputs: Mutex<Vec<String>>,
    inputs: Mutex<Vec<String>>,
    inbound: Mutex<VecDeque<Inbound>>,
    share: Mutex<bool>,
    /// The agent account driving this door, from its first relay call.
    owner: Mutex<Option<String>>,
    tx: Sender<String>,
}

impl Session {
    pub fn outputs(&self) -> Vec<String> {
        self.outputs.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn inputs(&self) -> Vec<String> {
        self.inputs.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Let `user` drive this door: the first account to use the code owns
    /// it, others are refused.
    pub fn claim(&self, user: &str) -> std::result::Result<(), String> {
        let mut owner = self.owner.lock().map_err(|_| "relay unavailable")?;
        match owner.as_deref() {
            Some(o) if o == user => Ok(()),
            Some(_) => Err("this relay is paired with another agent: press New code on the page, and give the new code".into()),
            None => {
                *owner = Some(user.to_string());
                let _ = self.tx.try_send(json!({ "type": "paired" }).to_string());
                Ok(())
            }
        }
    }

    pub fn shares(&self) -> bool {
        self.share.lock().map(|g| *g).unwrap_or(false)
    }

    /// The output port to use: the one named (exactly, or by a fragment of
    /// its name), else the only one.
    pub fn output(&self, wanted: Option<&str>) -> std::result::Result<String, String> {
        let outs = self.outputs();
        match wanted {
            Some(w) => outs.iter().find(|o| *o == w)
                .or_else(|| outs.iter().find(|o| o.to_lowercase().contains(&w.to_lowercase())))
                .cloned()
                .ok_or_else(|| format!("no output port '{w}' on the relay (ports: {})", outs.join(", "))),
            None => match outs.as_slice() {
                [only] => Ok(only.clone()),
                [] => Err("the relay has no MIDI output: plug the device in, or allow MIDI access on the page".into()),
                _ => Err(format!("several outputs on the relay, pass port: {}", outs.join(", "))),
            },
        }
    }

    /// Queue MIDI messages for the door to send on `port`.
    pub fn send(&self, port: &str, messages: &[Vec<u8>]) -> std::result::Result<(), String> {
        for m in messages {
            let frame = json!({ "type": "midi_out", "port": port, "data": m }).to_string();
            self.tx.try_send(frame).map_err(|_| "the relay is not taking messages (disconnected or stuck)".to_string())?;
        }
        Ok(())
    }

    /// The MIDI messages received since `since`.
    pub fn received_since(&self, since: Instant) -> Vec<Inbound> {
        self.inbound.lock()
            .map(|g| g.iter().filter(|m| m.at >= since).cloned().collect())
            .unwrap_or_default()
    }

    fn handle(&self, frame: &str) {
        let Ok(v) = serde_json::from_str::<Value>(frame) else { return };
        let names = |k: &str| -> Vec<String> {
            v.get(k).and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.chars().take(120).collect()).take(64).collect())
                .unwrap_or_default()
        };
        match v.get("type").and_then(Value::as_str) {
            Some("ports") => {
                if let Ok(mut g) = self.outputs.lock() { *g = names("outputs"); }
                if let Ok(mut g) = self.inputs.lock() { *g = names("inputs"); }
            }
            Some("midi_in") => {
                let data: Vec<u8> = v.get("data").and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_u64).map(|b| b as u8).take(65536).collect())
                    .unwrap_or_default();
                if data.is_empty() { return; }
                let port = v.get("port").and_then(Value::as_str).unwrap_or("").chars().take(120).collect();
                if let Ok(mut g) = self.inbound.lock() {
                    if g.len() >= INBOUND_KEPT { g.pop_front(); }
                    g.push_back(Inbound { at: Instant::now(), port, data });
                }
            }
            Some("share") => {
                if let Ok(mut g) = self.share.lock() {
                    *g = v.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                }
            }
            _ => {}
        }
    }
}

/// Every connected door, by pairing code, and the doors held per address.
#[derive(Default)]
pub struct Relays {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    doors: std::sync::atomic::AtomicUsize,
    per_client: Mutex<HashMap<String, usize>>,
}

/// One door counted against its address; released on drop.
struct ClientSlot<'a> {
    relays: &'a Relays,
    client: String,
}

impl Drop for ClientSlot<'_> {
    fn drop(&mut self) {
        if let Ok(mut g) = self.relays.per_client.lock() {
            if let Some(n) = g.get_mut(&self.client) {
                *n -= 1;
                if *n == 0 { g.remove(&self.client); }
            }
        }
    }
}

impl Relays {
    fn take_slot(&self, client: &str) -> Option<ClientSlot<'_>> {
        let mut g = self.per_client.lock().ok()?;
        let n = g.entry(client.to_string()).or_insert(0);
        if *n >= MAX_DOORS_PER_CLIENT { return None; }
        *n += 1;
        Some(ClientSlot { relays: self, client: client.to_string() })
    }
}

impl Relays {
    /// The session of a pairing code (case and dashes do not matter).
    pub fn get(&self, code: &str) -> Option<Arc<Session>> {
        let code = normalize(code);
        self.sessions.lock().ok()?.get(&code).cloned()
    }

    fn open(&self, user: String) -> (Arc<Session>, Receiver<String>) {
        let (tx, rx) = bounded(OUTBOUND_QUEUE);
        let mut sessions = self.sessions.lock().expect("relay registry");
        let code = loop {
            let c = new_code();
            if !sessions.contains_key(&c) { break c; }
        };
        let session = Arc::new(Session {
            code: code.clone(),
            user,
            outputs: Mutex::new(Vec::new()),
            inputs: Mutex::new(Vec::new()),
            inbound: Mutex::new(VecDeque::new()),
            share: Mutex::new(true),
            owner: Mutex::new(None),
            tx,
        });
        sessions.insert(code, session.clone());
        (session, rx)
    }

    fn close(&self, code: &str) {
        if let Ok(mut g) = self.sessions.lock() { g.remove(code); }
    }

    pub fn count(&self) -> usize {
        self.sessions.lock().map(|g| g.len()).unwrap_or(0)
    }
}

/// `ABCD-EFGH` → `ABCDEFGH`, upper case.
fn normalize(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

/// Eight characters of `CODE_ALPHABET`: about 40 bits.
fn new_code() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("system random source");
    bytes.iter().map(|b| CODE_ALPHABET[*b as usize % CODE_ALPHABET.len()] as char).collect()
}

/// `ABCDEFGH` → `ABCD-EFGH`, as shown to people.
pub fn display_code(code: &str) -> String {
    format!("{}-{}", &code[..4], &code[4..])
}

/// Accounts the relay can resolve: keep a known one, or issue a new one.
pub trait Accounts: Send + Sync {
    fn resolve(&self, presented: Option<&str>) -> Option<String>;
}

/// Serve doors on `bind`. Browser pages are accepted from `allowed_origins`
/// (and localhost); programs that send no origin are always accepted. With
/// `behind_proxy`, a door's address is the one the proxy appended to
/// `X-Forwarded-For`. Returns once the listener is up; doors are served on
/// background threads.
pub fn serve(bind: &str, relays: Arc<Relays>, accounts: Arc<dyn Accounts>, allowed_origins: Vec<String>,
             behind_proxy: bool) -> Result<SocketAddr> {
    use std::sync::atomic::Ordering;
    let listener = TcpListener::bind(bind).with_context(|| format!("bind relay {bind}"))?;
    let local = listener.local_addr()?;
    let allowed = Arc::new(allowed_origins);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            if relays.doors.fetch_add(1, Ordering::SeqCst) >= MAX_DOORS {
                relays.doors.fetch_sub(1, Ordering::SeqCst);
                eprintln!("relay: {MAX_DOORS} doors open, connection refused");
                continue; // dropping the stream closes it
            }
            let (relays, accounts, allowed) = (relays.clone(), accounts.clone(), allowed.clone());
            std::thread::spawn(move || {
                if let Err(e) = door(stream, &relays, accounts.as_ref(), &allowed, behind_proxy) {
                    eprintln!("relay: {e}");
                }
                relays.doors.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    Ok(local)
}

fn door(stream: TcpStream, relays: &Relays, accounts: &dyn Accounts, allowed: &[String], behind_proxy: bool) -> Result<()> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let peer = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    let mut presented: Option<String> = None;
    let mut slot: Option<ClientSlot> = None;
    let check = |req: &Request, resp: Response| -> std::result::Result<Response, ErrorResponse> {
        let deny = |status: tungstenite::http::StatusCode, why: &str| {
            let mut deny = ErrorResponse::new(Some(why.into()));
            *deny.status_mut() = status;
            deny
        };
        presented = req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("user=")))
            .map(str::to_string);
        let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
        if !crate::ws::origin_allowed(origin, allowed) {
            return Err(deny(tungstenite::http::StatusCode::FORBIDDEN, "origin not allowed"));
        }
        let client = behind_proxy.then(|| req.headers().get("x-forwarded-for")
            .and_then(|v| v.to_str().ok()).and_then(crate::ws::proxy_seen_address)).flatten()
            .unwrap_or_else(|| peer.clone());
        slot = relays.take_slot(&client);
        if slot.is_none() {
            return Err(deny(tungstenite::http::StatusCode::TOO_MANY_REQUESTS, "too many relays from this address"));
        }
        Ok(resp)
    };
    // A plain HTTP request (a crawler, a browser opening the URL) gets a
    // proper answer instead of a closed connection.
    let mut plain = stream.try_clone()?;
    let mut ws: WebSocket<TcpStream> = match tungstenite::accept_hdr(stream, check) {
        Ok(ws) => ws,
        Err(HandshakeError::Interrupted(_)) => anyhow::bail!("handshake timeout"),
        Err(HandshakeError::Failure(tungstenite::Error::Protocol(e))) => {
            let body = "This is the osc-bridge relay: connect with a WebSocket. See https://osc-bridge.roomi-fields.com/connect/\n";
            let _ = std::io::Write::write_all(&mut plain, format!(
                "HTTP/1.1 426 Upgrade Required\r\nUpgrade: websocket\r\nConnection: close\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()).as_bytes());
            anyhow::bail!("not a WebSocket request: {e}");
        }
        Err(HandshakeError::Failure(e)) => anyhow::bail!("handshake: {e}"),
    };
    ws.get_ref().set_read_timeout(Some(READ_TIMEOUT))?;
    let Some(user) = accounts.resolve(presented.as_deref()) else {
        let _ = ws.send(Message::Text(json!({ "type": "error", "error": "no account available" }).to_string().into()));
        return Ok(());
    };
    let (session, rx) = relays.open(user.clone());
    let hello = json!({ "type": "hello", "code": display_code(&session.code), "user": user });
    let outcome = (|| -> Result<()> {
        ws.send(Message::Text(hello.to_string().into()))?;
        let mut last_ping = Instant::now();
        loop {
            if last_ping.elapsed() >= KEEPALIVE {
                ws.send(Message::Ping(Vec::new().into()))?;
                last_ping = Instant::now();
            }
            while let Ok(frame) = rx.try_recv() {
                ws.send(Message::Text(frame.into()))?;
            }
            match ws.read() {
                Ok(Message::Text(t)) => session.handle(&t),
                Ok(Message::Close(_)) => return Ok(()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return Ok(()),
                Err(e) => return Err(e.into()),
            }
        }
    })();
    relays.close(&session.code);
    drop(slot);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AnyAccount;
    impl Accounts for AnyAccount {
        fn resolve(&self, presented: Option<&str>) -> Option<String> {
            Some(presented.unwrap_or("fresh").to_string())
        }
    }

    fn text(ws: &mut WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>) -> Value {
        loop {
            if let Message::Text(t) = ws.read().unwrap() {
                return serde_json::from_str(&t).unwrap();
            }
        }
    }

    #[test]
    fn a_door_pairs_and_carries_midi_both_ways() {
        let relays = Arc::new(Relays::default());
        let addr = serve("127.0.0.1:0", relays.clone(), Arc::new(AnyAccount), vec![], false).unwrap();
        let (mut ws, _) = tungstenite::connect(format!("ws://{addr}/relay?user=abc")).unwrap();
        let hello = text(&mut ws);
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["user"], "abc");
        let code = hello["code"].as_str().unwrap().to_string();
        assert_eq!(code.len(), 9);
        ws.send(Message::Text(json!({"type": "ports", "outputs": ["minilogue SOUND"], "inputs": ["minilogue SOUND"]}).to_string().into())).unwrap();
        let session = (0..200).find_map(|_| {
            std::thread::sleep(Duration::from_millis(5));
            relays.get(&code.to_lowercase()).filter(|s| !s.outputs().is_empty())
        }).expect("session with ports");
        assert_eq!(session.output(None).unwrap(), "minilogue SOUND");
        assert_eq!(session.output(Some("sound")).unwrap(), "minilogue SOUND");
        session.send("minilogue SOUND", &[vec![0xB0, 43, 45]]).unwrap();
        let out = text(&mut ws);
        assert_eq!(out, json!({"type": "midi_out", "port": "minilogue SOUND", "data": [0xB0, 43, 45]}));
        let t0 = Instant::now();
        ws.send(Message::Text(json!({"type": "midi_in", "port": "minilogue SOUND", "data": [0xB0, 43, 64]}).to_string().into())).unwrap();
        let got = (0..200).find_map(|_| {
            std::thread::sleep(Duration::from_millis(5));
            let r = session.received_since(t0);
            (!r.is_empty()).then_some(r)
        }).expect("inbound message");
        assert_eq!(got[0].data, vec![0xB0, 43, 64]);
        ws.close(None).unwrap();
        let closed = (0..200).any(|_| { std::thread::sleep(Duration::from_millis(5)); relays.get(&code).is_none() });
        assert!(closed, "the session ends with its socket");
    }

    #[test]
    fn the_first_agent_owns_a_door_and_one_address_holds_few() {
        let relays = Arc::new(Relays::default());
        let addr = serve("127.0.0.1:0", relays.clone(), Arc::new(AnyAccount), vec![], false).unwrap();
        let mut doors: Vec<_> = (0..MAX_DOORS_PER_CLIENT).map(|_| tungstenite::connect(format!("ws://{addr}/relay")).unwrap().0).collect();
        assert!(tungstenite::connect(format!("ws://{addr}/relay")).is_err(), "one address, too many doors");
        let code = text(&mut doors[0])["code"].as_str().unwrap().to_string();
        let session = relays.get(&code).unwrap();
        session.claim("agent-a").unwrap();
        assert_eq!(text(&mut doors[0])["type"], "paired");
        session.claim("agent-a").unwrap();
        assert!(session.claim("agent-b").is_err());
        for mut d in doors.drain(..) { d.close(None).unwrap(); let _ = d.read(); }
        let freed = (0..200).any(|_| {
            std::thread::sleep(Duration::from_millis(5));
            tungstenite::connect(format!("ws://{addr}/relay")).is_ok()
        });
        assert!(freed, "closing doors frees the address");
    }

    #[test]
    fn a_plain_http_request_gets_426() {
        use std::io::{Read, Write};
        let addr = serve("127.0.0.1:0", Arc::new(Relays::default()), Arc::new(AnyAccount), vec![], false).unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(b"GET /relay HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 426"), "{reply}");
    }

    #[test]
    fn pages_from_other_sites_are_refused() {
        let relays = Arc::new(Relays::default());
        let addr = serve("127.0.0.1:0", relays, Arc::new(AnyAccount), vec!["https://osc-bridge.roomi-fields.com".into()], false).unwrap();
        let req = |origin: &str| {
            let mut r = tungstenite::client::IntoClientRequest::into_client_request(format!("ws://{addr}/relay")).unwrap();
            r.headers_mut().insert("origin", origin.parse().unwrap());
            tungstenite::connect(r)
        };
        assert!(req("https://osc-bridge.roomi-fields.com").is_ok());
        assert!(req("https://evil.example").is_err());
    }
}
