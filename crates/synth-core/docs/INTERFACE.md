# synth-core — interface

`synth-core` offers a device compiled from a resolved descriptor, and a session that takes inputs and returns outputs. Every call returns its outputs or a typed fault; none performs I/O. The list below is the target of the core epic; once the crate exists, the part between the generated markers is drawn from its rustdoc comments.

## Overview

| Export | Shape | Returns | Refuses |
|---|---|---|---|
| `Device::compile` | `fn compile(device: &osd::Device) -> Result<Device, Fault>` | the compiled device | unknown capture, encoding, checksum or capability |
| `Session::new` | `fn new(device: Arc<Device>) -> Session` | a session with empty state | — |
| `Session::send` | `fn send(&mut self, msg: CoreMessage, now: Instant) -> Result<Outputs, Fault>` | MIDI or OSC messages, consent requests | unrouted message, argument out of range |
| `Session::receive` | `fn receive(&mut self, midi: MidiMessage, now: Instant) -> Result<Outputs, Fault>` | core messages to report | script over budget |
| `Session::consent` | `fn consent(&mut self, id: ConsentId, given: bool) -> Outputs` | the held messages, or nothing | — |
| `recognise` | `fn recognise(replies: &[Reply], candidates: &[Arc<Device>]) -> Vec<Match>` | the devices that match, best first, with firmware | — |
| `VirtualDevice` | `fn answer(&mut self, midi: MidiMessage) -> Vec<MidiMessage>` | what the described device would answer | — |
| `CoreMessage`, `MidiMessage`, `Outputs`, `Match`, `ConsentId` | typed values | — | — |
| `Fault` | typed fault | — | — |

## The faults

- `UnknownCapture`, `UnknownEncoding`, `UnknownChecksum`, `MissingCapability` — at compile time.
- `Unrouted` — no route of the device accepts the message.
- `OutOfRange` — an argument outside its declared range.
- `ScriptBudget` — a script exceeded its time or memory budget.
- `ScriptFault` — a script failed.

## The guard

- The shared cases: a driver, inputs, expected outputs, through `Session` only.
- A test that refuses any dependency performing I/O, and a build for `wasm32-unknown-unknown`.
- A bench of the message-to-bytes delay.

## Consumers

- `synth-io`: `MidiMessage`, `CoreMessage`
- `osc-bridge`: `Device`, `Session`, `recognise`, `VirtualDevice`, `CoreMessage`, `MidiMessage`, `Outputs`, `Fault`
- the SyntHub service: `Device`, `Session`, `recognise`, `CoreMessage`, `MidiMessage`, `Outputs`, `Fault`
- the SyntHub site: `Device`, `Session`, `recognise`, `VirtualDevice`, `MidiMessage`, `Outputs`

<!-- généré: début -->
<!-- généré: fin -->
