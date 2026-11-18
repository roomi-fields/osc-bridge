# osc-bridge (application) — interface

**Langage** : cargo

The application offers three published surfaces: its command line, its MCP tools, and its OSC addresses. Each is a contract with people, agents and software outside the repository; a change to one goes up to the owner.

## The command line

| Command | Receives | Returns |
|---|---|---|
| `list` | — | the MIDI ports |
| `detect` | `--no-probe` | the devices recognised on the ports |
| `run` | a driver, ports, OSC and WebSocket addresses, OSCQuery | a bridge for one device |
| `orchestrate` | a `bridge.toml` | several devices with routes between them |
| `mcp` | `--offline`, `--devices-dir` | the MCP server |
| `connect` | the relay address, private ports | a relay door |
| `export` | a driver, a format | the exported file |
| `backup`, `restore` | ports, a `.syx` file | a dump taken or sent |
| `privacy` | `--offline`, `--private`, `--public` | the installation's settings |
| `inspect`, `lint` | a driver | its summary, its warnings |
| `osc-send`, `osc-listen` | an address and arguments, a bind address | one OSC message, the messages received |

## The MCP tools

The tools are described once, from Rust types; the list the server offers is generated from them and compared, at each change, with the published list. Local tools work offline; the others are the service's, forwarded through `synthhub-protocol`.

Three tools link devices of a running orchestrator, reached by its UDP address (`target`, else `--default-target`) through its control addresses, and answering with the orchestrator's replies, or saying that none answered:

| Tool | Does | Orchestrator address |
|---|---|---|
| `add_link` | links `from` to `to`, optionally scaling the first argument (`map`: input range, output range); effective at the next message; refused, naming the address or the chain, for an unknown device, an address its driver does not have, a loop or more than four hops | `/bridge/link/add` |
| `remove_link` | removes the link from `from` to `to`; refused, naming it, when it does not exist | `/bridge/link/remove` |
| `list_links` | lists the links of the configuration file and those added live, each with source, target, origin (`file` or `live`) and scaling | `/bridge/link/list` |

## The OSC addresses

Each device's addresses come from its driver, under its prefix; the uniform addresses (`/identity`, `/program/recall`, `/program/dump`) are the same on every device; `/bridge/status` and `/bridge/docs` describe the bridge itself. An orchestrator also answers, to any client (UDP or WebSocket), `/bridge/link/add <from> <to> [in_lo in_hi out_lo out_hi]` (`/bridge/link/added`, or `/bridge/link/error <text>`), `/bridge/link/remove <from> <to>` (`/bridge/link/removed`, or `/bridge/link/error <text>`) and `/bridge/link/list` (one `/bridge/link/list/item <from> <to> <origin> [scaling]` per link, then `/bridge/link/list/end <count>`). OSCQuery publishes the whole namespace.

## The time the core adds

The core adds under 1 ms to a message, MIDI excluded (the charter's bound). The night bench `tests/it_budget/orchestrator_message_time.rs` holds it with two measures, each judging the median and the worst case: a message that follows a link (`message_that_follows_a_link_adds_under_the_bound`) and a message that follows no link (`message_that_follows_no_link_adds_under_the_bound`). They run on a loaded MIDI device without a port, from the message entering the orchestrator to the frame reaching the device's emission queue; the OSC wire encoding, the socket and the MIDI driver stay outside.

## The Lua a driver declares

A command `script` sees, besides `ctx` and `ob`, Electra One's two APIs under Electra's names (<https://docs.electra.one/developers/luaext.html>), so a preset's function runs unchanged:

- `parameterMap.get(deviceId, PT_<TYPE>, number)` answers the value held for the command whose `_electra.parameter` is (type, number): last sent or received, else its default, else the low end of its range, else 0.
- `devices.get(deviceId):getChannel()` answers the channel the engine sends the driver on, 1..16.

## The faults

- `NoDriver` — no driver matches the name, in the cache or the service.
- `Unauthorized` — an MCP request over HTTP without the right token.
- `ConsentRefused` — the person refused a message at the door.
- `DeviceFault` — a device description that cannot be used as written (device, field, cause), held by the loaded device and read by `lint` and `inspect`; `mcp::fault_blocks(faults)` turns the faults of the identities `detect_hardware` set aside into one text block each, without MIDI.
- the faults of `synth-core`, `synth-io` and `synthhub-protocol`, passed on with their code.

## The guard

- A snapshot of the MCP tools list, compared at each change.
- End-to-end tests through the command line and the MCP server on a virtual device.

## Consumers

- Agents (MCP clients), music software (OSC), people (the command line).
