# synth-io — interface

`synth-io` offers one adapter per transport, each with the same shape: open, send, receive. The list below is the target of the core epic; once the crate exists, the part between the generated markers is drawn from its rustdoc comments.

## Overview

| Export | Shape | Returns | Refuses |
|---|---|---|---|
| `MidiPorts::list` | `fn list() -> Result<Vec<PortInfo>, Fault>` | the ports present, inputs and outputs | — |
| `MidiPorts::open` | `fn open(name: &str, pacing: Pacing) -> Result<MidiPort, Fault>` | an open MIDI 1 port | unknown port |
| `MidiPort::send` | `fn send(&self, msg: &MidiMessage) -> Result<(), Fault>` | — | closed port |
| `OscSocket::bind` | `fn bind(addr: SocketAddr) -> Result<OscSocket, Fault>` | an OSC endpoint over UDP | address in use |
| `WsServer::serve` | `fn serve(addr: SocketAddr, origins: &[String]) -> Result<WsServer, Fault>` | WebSocket clients carrying OSC | origin refused |
| `OscQuery::serve` | `fn serve(addr: SocketAddr, tree: Namespace) -> Result<(), Fault>` | the address description over HTTP | address in use |
| `Inbound` | a channel of `(Source, MidiMessage or CoreMessage)` | what arrives, tagged | — |
| `Pacing`, `PortInfo`, `Source`, `Fault` | typed values | — | — |

MIDI 2 adds `Ump` endpoints of the same shape when a device of the catalogue needs it.

## The faults

- `UnknownPort` — no port of that name.
- `Closed` — the port or client is gone.
- `AddressInUse` — the socket address is taken.
- `OriginRefused` — a WebSocket client from an origin not allowed.

## The guard

- Round trips on loopback ports and sockets, per transport.
- A bench of the added delay at the default pacing.

## Consumers

- `osc-bridge`: `MidiPorts`, `MidiPort`, `OscSocket`, `WsServer`, `OscQuery`, `Inbound`, `Pacing`, `Fault`

<!-- généré: début -->
<!-- généré: fin -->
