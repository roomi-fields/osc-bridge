# synth-io — frame

## Role

- **R1** `synth-io` carries messages between the core and the outside: MIDI 1 ports, MIDI 2 endpoints, OSC over UDP, WebSocket clients, and the OSCQuery description of a device's addresses.

## Receives

- **R2** MIDI messages and core messages to send, with the port or client they go to.
- **R3** Bytes and packets from ports, sockets and clients.

## Returns

- **R4** MIDI messages and core messages received, tagged with their port or client; the list of ports present; typed faults.

## Knows

- **R5** The core's message types, the MIDI 1 byte encoding, MIDI 2 packets, the OSC encoding, WebSocket and OSCQuery over HTTP.

## Does not know

- **R6** Drivers, sessions, what a message means to a device, the service, MCP.

## Refuses

- **R7** A message to a port or client that does not exist: a typed fault.
- **R8** Output faster than the pacing set for a port; it queues and paces instead.

## Invariants

- **R9** Messages to one port leave in the order they were given.
- **R10** A MIDI message encoded and decoded on the same transport is unchanged.

## Cost

- **R11** Under 1 ms of added delay per message at the default pacing, measured on a loopback port.
