//! WebSocket transport — lets browser clients (which cannot speak UDP) join
//! the bridge as OSC clients.
//!
//! Wire format: **binary WS frames carry raw OSC packets**, byte-identical to
//! the UDP transport (rosc encoding). No JSON envelope — one encoder on both
//! sides. Text frames are ignored with a warning.
//!
//! Semantics: a connected WS client is equivalent to a dynamic `--osc-client`
//! — it receives every outbound message the bridge broadcasts (decoded MIDI-in,
//! SysEx replies, `/bridge/status` responses), and every binary frame it sends
//! goes through the exact same dispatch as OSC received over UDP.
//!
//! Implementation: synchronous `tungstenite`, matching the thread-based
//! architecture of the rest of the runtime (no async runtime). One accept
//! thread plus one thread per connection. Each connection thread multiplexes
//! both directions over a single socket: a short read timeout (1 ms) bounds
//! the added latency for outbound frames queued by `broadcast`.

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use rosc::OscPacket;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tungstenite::{HandshakeError, Message, WebSocket};

/// Callback invoked for every OSC packet decoded from an incoming WS frame.
/// The `SocketAddr` is the TCP peer address of the WS connection — it plays
/// the same role as the UDP `from` address in log lines.
pub type WsDispatch = Arc<dyn Fn(OscPacket, SocketAddr) + Send + Sync>;

/// Per-connection outbound queue depth. A slow client drops frames past this
/// backlog (the connection stays up) — same "lossy fan-out" semantics as UDP.
const OUTBOUND_QUEUE: usize = 256;

/// How long the connection thread blocks waiting for an inbound frame before
/// draining the outbound queue. Bounds the latency of broadcast frames.
const READ_TIMEOUT: Duration = Duration::from_millis(1);

/// A stalled client (TCP buffer full for this long) is disconnected.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// A client that doesn't complete the WS handshake within this window is
/// dropped — keeps half-open connections from pinning threads forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Registry of connected WS clients. Cheap to clone; `broadcast` fans an
/// already-encoded OSC packet out to every live connection.
#[derive(Clone, Default)]
pub struct WsClients {
    senders: Arc<Mutex<Vec<Sender<Vec<u8>>>>>,
}

impl WsClients {
    pub fn new() -> Self {
        Self::default()
    }

    /// True if at least one client is currently connected. Lets `send_osc`
    /// skip encoding when nobody is listening.
    pub fn has_clients(&self) -> bool {
        self.client_count() > 0
    }

    /// Number of currently registered clients. Disconnected clients linger
    /// until the next `broadcast` prunes them.
    pub fn client_count(&self) -> usize {
        self.senders.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Queue `bytes` (an encoded OSC packet) to every connected client.
    /// Disconnected clients are pruned; slow clients lose this frame but
    /// keep their connection.
    pub fn broadcast(&self, bytes: &[u8]) {
        let Ok(mut g) = self.senders.lock() else { return };
        g.retain(|tx| match tx.try_send(bytes.to_vec()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Disconnected(_)) => false,
        });
    }

    fn register(&self) -> Receiver<Vec<u8>> {
        let (tx, rx) = bounded(OUTBOUND_QUEUE);
        if let Ok(mut g) = self.senders.lock() {
            g.push(tx);
        }
        rx
    }
}

/// Bind `bind` and serve WebSocket clients. Returns the bound address once
/// the listener is up (fail-fast on port conflicts); accepting runs on a
/// background thread for the lifetime of the process.
pub fn serve(bind: &str, clients: WsClients, dispatch: WsDispatch) -> Result<SocketAddr> {
    let listener = TcpListener::bind(bind).with_context(|| format!("bind WS {bind}"))?;
    let local = listener.local_addr()?;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let clients = clients.clone();
            let dispatch = dispatch.clone();
            std::thread::spawn(move || {
                if let Err(e) = handle_conn(stream, clients, dispatch) {
                    eprintln!("WS conn: {e}");
                }
            });
        }
    });
    Ok(local)
}

fn handle_conn(stream: TcpStream, clients: WsClients, dispatch: WsDispatch) -> Result<()> {
    let peer = stream.peer_addr().context("WS peer_addr")?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut ws: WebSocket<TcpStream> = match tungstenite::accept(stream) {
        Ok(ws) => ws,
        Err(HandshakeError::Interrupted(_)) => bail!("WS handshake timeout from {peer}"),
        Err(HandshakeError::Failure(e)) => bail!("WS handshake with {peer}: {e}"),
    };
    ws.get_ref().set_read_timeout(Some(READ_TIMEOUT))?;

    let rx = clients.register();
    eprintln!("WS +client {peer}");
    let outcome = conn_loop(&mut ws, &rx, peer, &dispatch);
    eprintln!("WS -client {peer}");
    // Dropping `rx` disconnects the sender half; the registry prunes it on
    // the next broadcast.
    outcome
}

fn conn_loop(
    ws: &mut WebSocket<TcpStream>,
    rx: &Receiver<Vec<u8>>,
    peer: SocketAddr,
    dispatch: &WsDispatch,
) -> Result<()> {
    loop {
        // Outbound: flush everything queued by broadcast() since last pass.
        while let Ok(bytes) = rx.try_recv() {
            ws.send(Message::Binary(bytes.into()))
                .map_err(|e| anyhow!("WS send to {peer}: {e}"))?;
        }
        // Inbound: block up to READ_TIMEOUT for the next frame.
        match ws.read() {
            Ok(Message::Binary(b)) => match rosc::decoder::decode_udp(&b) {
                Ok((_, pkt)) => dispatch(pkt, peer),
                Err(e) => eprintln!("WS OSC decode err from {peer}: {e}"),
            },
            Ok(Message::Text(_)) => {
                eprintln!("WS text frame from {peer} ignored (binary OSC only)");
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {} // ping/pong — tungstenite answers pings itself
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(tungstenite::Error::ConnectionClosed)
            | Err(tungstenite::Error::AlreadyClosed) => return Ok(()),
            Err(e) => bail!("WS read from {peer}: {e}"),
        }
    }
}
