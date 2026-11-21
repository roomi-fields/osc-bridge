# osc-bridge (application) — interface

**Langage** : cargo

The application offers three published surfaces: its command line, its MCP tools, and its OSC addresses. Each is a contract with people, agents and software outside the repository; a change to one goes up to the owner.

## The command line

| Command | Receives | Returns |
|---|---|---|
| `list` | — | the MIDI ports |
| `detect` | `--no-probe`; `--devices-dir`; `--learn` and `--listen-ms` (1000-30000, default 5000, only with `--learn`); `--write <file>`; `--read <file>` | the devices recognised on the ports; with `--learn`, the notice (it listens N seconds, move the controls), then per device (all its ports) its fingerprint and the measures beside it; with `--write`, the same report and a capture file; with `--read`, the report over a capture, with no device and nothing sent (it refuses `--no-probe` and `--write`) |
| `run` | a driver, ports, OSC and WebSocket addresses, OSCQuery | a bridge for one device |
| `orchestrate` | a `bridge.toml` | several devices with routes between them |
| `mcp` | `--offline`, `--devices-dir` | the MCP server |
| `connect` | the relay address, private ports | a relay door |
| `export` | `--device` (a driver) and either `--format` (any format of `osd-formats`; an unknown name lists them), or `--on` (the target device, a driver file, id or slug) with `--yes` | the exported file on the standard output; with `--on`, the export put on the plugged target by the way its descriptor declares, or, without `--yes`, what would be written (nothing is sent) |
| `backup`, `restore` | `--device` (required: a driver file, id or slug), ports, a `.syx` file; `restore` also `--bank` and `--number` for a sound that carries no place | a dump taken (the sounds the driver recognises, on any channel) or sent (the driver's pauses; a bank it says cannot be written refused before any message; channel and device number written as configured) |
| `privacy` | `--offline`, `--private`, `--public` | the installation's settings |
| `inspect`, `lint` | a driver | its summary, its warnings |
| `osc-send`, `osc-listen` | an address and arguments, a bind address | one OSC message, the messages received |

### The detection capture

`detect --write <file>` writes what the detection observed, raw, as JSON (`format_version` 1; a capture of another version is refused with an error naming it): `outputs` and `inputs` (the port names as the system gave them, in order), `cards` (the text of `/proc/asound/cards`), `usbids` (`card`, `text` of each card's `usbid`), `requests` (`port`, `bytes`) and `received` (`port`, `bytes`, `ms` since the first request). The pairing of inputs with outputs and the USB ids are never written: `--read` derives them again, through the derivation the scan uses. Only `/proc/asound/cards` and each card's `usbid` (maker:product) are read; every other file of a card, the USB serial field, the host name and the user name are left out. The messages received are kept raw, not redacted, because the replay depends on them: a maker's reply may carry a serial number, so the person reads a capture before sharing it. The option writes a local file and sends nothing more than the detection does. The corpus of captures is `crates/osc-bridge/tests/fixtures/captures/`: one capture per bench session named by day, beside `<day>.expected.json` (port → device id, written by a person).

## The relay frames

`crate::relay_protocol` publishes the frames of the relay door, one contract with the service, whose browser door reads the same reference frames (`tests/fixtures/relay_frames.json`):

- built: `ports_frame(outputs, inputs)`, `share_frame(enabled, private)`, `midi_in_frame(port, data)`, `confirm_reply_frame(id, accepted)`, each a JSON value;
- read: `parse_door_frame(text)` gives a `ServerFrame` (`Hello`, `MidiOut`, `Confirm`, `ConfirmCancel`, `Paired`, `Error`) or nothing for text that is not a known frame;
- the rule `needs_confirmation` and `is_identity_request` on a message.

## The MCP tools

`send` always asks for the acknowledgement and hands back at the first answer: `/bridge/done` gives `sent: true`; `/bridge/fault` gives `isError: true` with the address and the reason, in text and in `structuredContent` (`sent: false`, `address`, `reason`); when nothing answers within 3 s (a guard, not a delay) it gives an error saying osc-bridge does not answer at that address.

The tools are described once, from Rust types; the list the server offers is generated from them and compared, at each change, with the published list. Local tools work offline; the others are the service's, forwarded through `synthhub-protocol`.

`detect_hardware` takes `learn` (boolean) and `listen_ms` (1000-30000, default 5000; a value outside is refused with an error naming the range). In learn mode it matches no driver: it listens for that time on each port, sends nothing but the universal Identity Request (on every output of a device), says in its text that it listens N seconds and that the person can move the controls, and returns one element per device (the ports sharing an ALSA client number, else a client name), each with its `inputs` and `outputs` (raw port names), its `fingerprint`, exactly an OSD 0.6 fingerprint (`via: native`; `usb`, `ports` as lower-case words with no client or port numbers and no repeated words, `identity_reply`, `sends`, `controls` of 32 distinct entries at most, a field the device does not give being absent), and `measured`, a sibling object holding the local measures (`identity_reply_ms` with `identity_reply_port`, `periods_ms` with `periods_port`, the port each was measured on), never inside the fingerprint. The `detect` command's `--learn` returns the same, printed for the terminal. Without `learn`, the report is unchanged.

`export_device` writes one driver in a format of `osd-formats` and only reads. `install_export` (`device`, `on`, `confirm_overwrite`) is the tool that puts an export on a plugged device, the only one of the two that writes: the format, the path and the way come from the target's descriptor, nothing names a device, and without `confirm_overwrite: true` nothing is sent and the preview (format, way, path, size, losses) is returned. The library publishes `osc_bridge::install::install` (the gesture) and `osc_bridge::export::Format`, which wraps the `osd-formats` format.

Three tools link devices of a running orchestrator, reached by its UDP address (`target`, else `--default-target`) through its control addresses, and answering with the orchestrator's replies, or saying that none answered:

| Tool | Does | Orchestrator address |
|---|---|---|
| `add_link` | links `from` to `to`, optionally scaling the first argument (`map`: input range, output range); effective at the next message; refused, naming the address or the chain, for an unknown device, an address its driver does not have, a loop or more than four hops | `/bridge/link/add` |
| `remove_link` | removes the link from `from` to `to`; refused, naming it, when it does not exist | `/bridge/link/remove` |
| `list_links` | lists the links of the configuration file and those added live, each with source, target, origin (`file` or `live`) and scaling | `/bridge/link/list` |

## The OSC addresses

Each device's addresses come from its driver, under its prefix; the uniform addresses (`/identity`, `/program/recall`, `/program/dump`) are the same on every device; `/bridge/status` and `/bridge/docs` describe the bridge itself. An orchestrator also answers, to any client (UDP or WebSocket), `/bridge/link/add <from> <to> [in_lo in_hi out_lo out_hi]` (`/bridge/link/added`, or `/bridge/link/error <text>`), `/bridge/link/remove <from> <to>` (`/bridge/link/removed`, or `/bridge/link/error <text>`) and `/bridge/link/list` (one `/bridge/link/list/item <from> <to> <origin> [scaling]` per link, then `/bridge/link/list/end <count>`). A link follows the output of another link: with a→b and b→c, a message received at a reaches c, once per device address. A chain is followed for four links; a matching fifth link is not followed, and the cut is said by `/bridge/link/error` naming the chain followed. OSCQuery publishes the whole namespace.

The faults of a message come back to whoever sent it. A message that cannot be built, run or queued (a command or parameter whose frame does not build, a script that fails, an unknown address, a full emission queue) is answered, to its sender alone, `/bridge/fault <address> <reason>` (two strings: the message's address and why). A fault whose message has no real sender (re-injected by a link, set off by an incoming MIDI message, a MIDI port that refuses a frame) is broadcast to the declared clients, as `/bridge/status` is. A correct message answers nothing. A sender that wants to know its message is handled asks for it, in the same packet: `/bridge/sync <id>` is answered `/bridge/done <id>` once what precedes it in the packet is handled, after any `/bridge/fault`; nothing is answered to a sender that does not ask. Over WebSocket the answer goes through the WebSocket broadcast. The Rust door: `runtime::ReplyTo` (the route back to a sender, beside `SendOsc`), `runtime::dispatch_packet_to_device(state, packet, send_osc, reply_to, from)` (one packet, its `/bridge/sync` answered after the lot), and `reply_to` beside `send_osc` in `dispatch_to_device`, `dispatch_osc_to_device` and `BenchPath::dispatch`.

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
