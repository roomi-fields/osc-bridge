# synth-core — frame

## Role

- **R1** `synth-core` holds the session of one device: it turns core messages into the device's MIDI or OSC messages and the device's messages back into core messages, recognises a device from its replies, recalls and reads its programs, and runs the scripts its driver declares.
- **R2** It also holds a virtual device: a session that answers like the device its driver describes.

## Receives

- **R3** A resolved device from `osd`.
- **R4** Core messages (an address with typed arguments), MIDI messages from the device, the time when it matters (pacing, timeouts), and the caller's answer to a consent request.

## Returns

- **R5** MIDI or OSC messages to send, core messages to report, consent requests, and typed faults.

## Knows

- **R6** The `osd` types, the meaning of each encoding and checksum, the MIDI messages, the Lua sandbox.

## Does not know

- **R7** Ports, sockets, files, clocks, the network, the service, MCP; how a MIDI message travels on the wire (MIDI 1 bytes or MIDI 2 packets) belongs to `synth-io`.

## Refuses

- **R8** A driver whose templates name an unknown capture, whose encodings or checksums are unknown, or whose declared capability it does not have: at compile time.
- **R9** A message no route of the device accepts, or with arguments outside their range: a typed fault, nothing sent.
- **R10** Sending a SysEx that may overwrite the device's memory without the caller's consent; only the identity request needs none.
- **R11** A script that exceeds its time or memory budget.

## Invariants

- **R12** Given the same driver, session state and inputs, it returns the same outputs.
- **R13** Every shared case (driver, inputs, expected outputs) passes.
- **R14** It compiles to `wasm32-unknown-unknown`.

## Cost

- **R15** Under 1 ms from a core message to its MIDI messages on a desktop machine, measured by a bench; compiling a device under 5 ms.
