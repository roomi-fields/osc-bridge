# osc-bridge (application) — frame

## Role

- **R1** The `osc-bridge` application puts the engine in a person's hands: a command line, an MCP server of local tools for agents, and an orchestrator that runs several devices with routes between them. Its library is what a program builds on.

## Receives

- **R2** Commands and options from the person; MCP requests over stdio, or over HTTP with a token; OSC from software over UDP or WebSocket; MIDI from devices through `synth-io`.
- **R3** Cards in the OSD format, each from a file, a folder of files, or a web address that returns a card (redirections followed), given by the person or by the program that calls it. A card from a web address is complete: it names no parent.

## Returns

- **R4** MIDI and OSC to devices and software; MCP results to agents; text and exit codes to the person.

## Knows

- **R5** The crates of the workspace, the `osd` reader, `osd-formats`, the MCP specification, and the configuration the person gives (options, `bridge.toml`).

## Does not know

- **R6** Any service: the account, the catalogue ids and short names, the cache of a service, the order between the sources of a card, the relay, the privacy settings and the learn mode belong to the program built on it.
- **R7** How a device is encoded: that belongs to `synth-core`.

## Refuses

- **R8** An MCP request over HTTP without the token the person set.
- **R9** Sending a SysEx that may overwrite a device's memory without the person's consent at the door.
- **R10** A card named by anything but a file, a folder or a web address: the error names the three forms.
- **R11** A web address that does not return a card: the error names the address and the cause, and no card is guessed.
- **R12** A card from a web address that names a parent: the error says a card read from an address is complete; the address of a parent waits for the OSD specification.
- **R13** Two cards of one folder that claim the same device: the error names both.
- **R14** Recognising a device or serving agents without a folder of cards: `detect` and `mcp` require it.

## Invariants

- **R15** With cards given as files or folders, it works with no network and opens no connection.
- **R16** It keeps a copy of each card read from a web address, and starts again from the copy without the network. The copy is no cache of a service and carries no catalogue id and no short name.
- **R17** The MCP server listens locally unless the person sets another address.
- **R18** No module of it reads a module, a crate or the address of a service; the guard of the frontier holds it (`docs/ARCHITECTURE.md` § 3.2).

## Cost

- **R19** The start of the MCP server over a local folder, under 200 ms.
