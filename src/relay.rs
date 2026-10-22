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
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

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
    tx: Sender<String>,
}

impl Session {
    pub fn outputs(&self) -> Vec<String> {
        self.outputs.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn inputs(&self) -> Vec<String> {
        self.inputs.lock().map(|g| g.clone()).unwrap_or_default()
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

/// Every connected door, by pairing code.
#[derive(Default)]
pub struct Relays {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
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
/// (and localhost); programs that send no origin are always accepted.
/// Returns once the listener is up; doors are served on background threads.
pub fn serve(bind: &str, relays: Arc<Relays>, accounts: Arc<dyn Accounts>, allowed_origins: Vec<String>) -> Result<SocketAddr> {
    let listener = TcpListener::bind(bind).with_context(|| format!("bind relay {bind}"))?;
    let local = listener.local_addr()?;
    let allowed = Arc::new(allowed_origins);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let (relays, accounts, allowed) = (relays.clone(), accounts.clone(), allowed.clone());
            std::thread::spawn(move || {
                if let Err(e) = door(stream, &relays, accounts.as_ref(), &allowed) {
                    eprintln!("relay: {e}");
                }
            });
        }
    });
    Ok(local)
}

fn door(stream: TcpStream, relays: &Relays, accounts: &dyn Accounts, allowed: &[String]) -> Result<()> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut presented: Option<String> = None;
    let check = |req: &Request, resp: Response| -> std::result::Result<Response, ErrorResponse> {
        presented = req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("user=")))
            .map(str::to_string);
        let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
        if crate::ws::origin_allowed(origin, allowed) {
            return Ok(resp);
        }
        let mut deny = ErrorResponse::new(Some("origin not allowed".into()));
        *deny.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
        Err(deny)
    };
    let mut ws: WebSocket<TcpStream> = match tungstenite::accept_hdr(stream, check) {
        Ok(ws) => ws,
        Err(HandshakeError::Interrupted(_)) => anyhow::bail!("handshake timeout"),
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
        loop {
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
        let addr = serve("127.0.0.1:0", relays.clone(), Arc::new(AnyAccount), vec![]).unwrap();
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
    fn pages_from_other_sites_are_refused() {
        let relays = Arc::new(Relays::default());
        let addr = serve("127.0.0.1:0", relays, Arc::new(AnyAccount), vec!["https://osc-bridge.roomi-fields.com".into()]).unwrap();
        let req = |origin: &str| {
            let mut r = tungstenite::client::IntoClientRequest::into_client_request(format!("ws://{addr}/relay")).unwrap();
            r.headers_mut().insert("origin", origin.parse().unwrap());
            tungstenite::connect(r)
        };
        assert!(req("https://osc-bridge.roomi-fields.com").is_ok());
        assert!(req("https://evil.example").is_err());
    }
}
