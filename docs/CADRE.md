# osc-bridge (application) — frame

## Role

- **R1** The `osc-bridge` application puts the engine in a person's hands: a command line, an MCP server for agents, an orchestrator that runs several devices with routes between them, and a relay door that lets the SyntHub service reach the person's devices.

## Receives

- **R2** Commands and options from the person; MCP requests over stdio, or over HTTP with a token; OSC from software over UDP or WebSocket; MIDI from devices through `synth-io`.
- **R3** Drivers from the service, through `synthhub-protocol`, or from a local folder the person gives.

## Returns

- **R4** MIDI and OSC to devices and software; MCP results to agents; relay frames to the service; text and exit codes to the person.

## Knows

- **R5** The four crates of the workspace, the `osd` reader, the MCP specification, the installation's settings (privacy, server, cache).

## Does not know

- **R6** The service's inside (its database, its domains); it reaches the service only through `synthhub-protocol`.
- **R7** How a device is encoded: that belongs to `synth-core`.

## Refuses

- **R8** An MCP request over HTTP without the token the person set.
- **R9** Sending a SysEx that may overwrite a device's memory without the person's consent at the door.
- **R10** Sharing with the service what the person keeps private (a device, a port).

## Invariants

- **R11** Offline, it keeps working with the drivers in its cache.
- **R12** An installation keeps the id the service issued it, linked to its person once they sign in.
- **R13** The MCP server listens locally unless the person sets another address.

## Cost

- **R14** The start of the MCP server, offline, under 200 ms; the lookup of a driver in the cache under 10 ms.
