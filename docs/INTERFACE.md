# osc-bridge (application) — interface

The application offers three published surfaces: its command line, its MCP tools, and its OSC addresses. Each is a contract with people, agents and software outside the repository; a change to one goes up to the owner.

## The command line

| Command | Receives | Returns |
|---|---|---|
| `list` | — | the MIDI ports |
| `detect` | `--no-probe` | the devices recognised on the ports |
| `run` | a driver, ports, OSC and WebSocket addresses, OSCQuery | a bridge for one device |
| `orchestrate` | a `bridge.toml` | several devices with routes between them |
| `mcp` | `--http <addr> --token <file>`, `--offline`, `--devices-dir` | the MCP server |
| `connect` | the relay address, private ports | a relay door |
| `export` | a driver, a format | the exported file |
| `backup`, `restore` | ports, a `.syx` file | a dump taken or sent |
| `privacy` | `--offline`, `--private`, `--public` | the installation's settings |
| `inspect`, `lint` | a driver | its summary, its warnings |
| `osc-send`, `osc-listen` | an address and arguments, a bind address | one OSC message, the messages received |

## The MCP tools

The tools are described once, from Rust types; the list the server offers is generated from them and compared, at each change, with the published list. Local tools work offline; the others are the service's, forwarded through `synthhub-protocol`.

## The OSC addresses

Each device's addresses come from its driver, under its prefix; the uniform addresses (`/identity`, `/program/recall`, `/program/dump`) are the same on every device; `/bridge/status` and `/bridge/docs` describe the bridge itself. OSCQuery publishes the whole namespace.

## The faults

- `NoDriver` — no driver matches the name, in the cache or the service.
- `Unauthorized` — an MCP request over HTTP without the right token.
- `ConsentRefused` — the person refused a message at the door.
- the faults of `synth-core`, `synth-io`, `formats` and `synthhub-protocol`, passed on with their code.

## The guard

- A snapshot of the MCP tools list, compared at each change.
- End-to-end tests through the command line and the MCP server on a virtual device.

## Consumers

- Agents (MCP clients), music software (OSC), people (the command line).
