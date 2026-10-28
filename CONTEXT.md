# osc-bridge — glossary

The words of the engine: how it runs a device and how a hosted service reaches a person's devices. The words of device descriptions (driver, maker profile, shared MIDI, capability, fingerprint, sound, program) are defined by the OSD format's glossary: https://github.com/roomi-fields/open-synth-descriptors/blob/main/CONTEXT.md.

## Running a device

**Session**:
The engine's live state for one connected device: its compiled driver, its script state, its dynamic routes, its pending replies.
_Avoid_: connection, instance

**Core message**:
What a caller says to a device: an address with typed arguments, independent of the wire.
_Avoid_: command, OSC message (when the wire is not OSC)

**Transport**:
How messages travel to a device or a client: MIDI 1, MIDI 2, OSC over UDP, WebSocket.
_Avoid_: driver, backend

**Virtual device**:
A device that exists only in software and answers like the device its driver describes.
_Avoid_: emulator (kept for third-party software such as the FM-1 emulator), mock

**Consent request**:
A message held until the person accepts it, because it may overwrite what the device keeps.
_Avoid_: confirmation, prompt

## Reaching a person's devices

**Installation**:
A copy of osc-bridge, or an agent, with its account issued by the service, linked to a person once they sign in.
_Avoid_: account, user id, client

**Door**:
What connects a person's devices to the relay: the connect page, the command line, later a plugin.
_Avoid_: client, bridge, agent

**Relay**:
The service that carries messages between an agent and the devices behind a door, with the person's consent for what may overwrite their memory.
_Avoid_: proxy, tunnel
