# osc-bridge MCP server — a MIDI MCP and OSC MCP

`osc-bridge` ships a [Model Context Protocol](https://modelcontextprotocol.io)
server. It lets Claude — or any MCP client — search the SyntHub catalogue of
music devices, read each device's OSC surface, and send OSC to control
synthesizers and DAWs.

It is both:

- a **MIDI MCP** — hardware synthesizers exposed over MIDI / SysEx
  (Prophets, Oberheims, Moogs, the Yamaha DX family, MatrixBrute, Hydrasynth,
  Virus TI, Digitone…), and
- an **OSC MCP** — DAWs and live-coding environments over OSC (Ableton Live,
  Bitwig Studio, Reaper, Sonic Pi, SuperCollider, Pure Data, TouchDesigner,
  VCV Rack).

The MCP server is one consumption mode of osc-bridge — the same bridge is also
usable from OSC clients, the CLI, and the multi-device orchestrator. See the
[main README](../README.md) for the whole picture.

## Hosted, nothing to install

The SyntHub server runs this MCP server for you, with the catalogue, trust
from real use, and a relay to the hardware on your machine:

- **Claude** (web, desktop): Settings → Connectors → *Add custom connector* →
  `https://mcp.synthub.roomi-fields.com/mcp`
- **Claude Code**: `claude mcp add --transport http synthub https://mcp.synthub.roomi-fields.com/mcp`
- **Cursor and other clients**: `{"mcpServers": {"synthub": {"url": "https://mcp.synthub.roomi-fields.com/mcp"}}}`

Your synth joins through the relay page,
[synthub.roomi-fields.com/connect](https://synthub.roomi-fields.com/connect/).
Tools and what the server records: [API for agents](https://synthub.roomi-fields.com/api/).

## On your machine

No global install needed — the MCP server runs straight from npm:

```bash
npx -y @roomi-fields/osc-bridge mcp
```

`npm install` downloads the prebuilt native binary for your platform
(Linux x64, Windows x64, macOS arm64 / x64) from the matching GitHub release.

### Claude Desktop

`claude_desktop_config.json`:

```jsonc
{
  "mcpServers": {
    "osc-bridge": {
      "command": "npx",
      "args": ["-y", "@roomi-fields/osc-bridge", "mcp"]
    }
  }
}
```

### Claude Code / Cursor / other MCP clients

Same shape — `command: "npx"`, `args: ["-y", "@roomi-fields/osc-bridge", "mcp"]`.
The server speaks JSON-RPC 2.0 over stdio.

### Optional flags

`osc-bridge mcp` accepts:

- `--devices-dir <path>` — serve the drivers of this folder, as they are,
  instead of those of the SyntHub catalogue.
- `--default-target <host:port>` — where `send` / `get_status` point when no
  `target` argument is given (default `127.0.0.1:7777`, the standard
  `osc-bridge run` bind).
- `--offline` — do not contact the SyntHub server (also
  `OSC_BRIDGE_OFFLINE=1`); see *Linked to the SyntHub server* below.

## The tools

| Tool | What it does |
|---|---|
| `list_devices` | Find device drivers — `device` (the catalogue id), name, vendor, `osc_prefix`, kind (hardware/software), source tier, documented firmware. Optional `query` (words matched in vendor, name and OSC prefix), `vendor`, `kind`, `limit`. |
| `get_device_docs` | Fetch the markdown companion guide for one device (`device`: a catalogue id or OSC prefix). |
| `list_routes` | Return a device's full OSC surface — declarative commands, CC/NRPN params, SysEx params, named drum sounds, reply patterns. |
| `export_device` | Write one driver (`device`: id or OSC prefix) in a `format` of `osd-formats`: `oscquery` (its OSC namespace, OSCQuery JSON), `midnam` (its MIDI names, MIDI Name Document XML), `deluge` (a Deluge definition file) and the others of the enum, with its sources' credit and licence. An unknown format is refused with the list. Reads only, takes no consent. See [EXPORT.md](EXPORT.md). |
| `install_export` | Put the export of a driver (`device`) on another device (`on`: its id or OSC prefix) plugged into this computer, in one gesture for every target: the target's own driver says the format it receives, the path of the file and the way it is sent (a file written by SysEx, step by step); nothing in osc-bridge names a device. A write may overwrite a file on the device, which is not read first, so without `confirm_overwrite: true` nothing is sent and the call returns the preview (format, way, path, size, what the format could not carry); the client should ask the user. Refused, sending nothing: a target that declares nothing it receives, a target not plugged in (named), a source whose export cannot be built in that format (for example without CC names). A send that fails on the way says the step reached and the messages already sent; nothing is retried or undone. See [EXPORT.md](EXPORT.md). |
| `send` | Send a single OSC message to a running `osc-bridge` instance (typed args: int, float, string, bool). The tool always asks the bridge for its acknowledgement (`/bridge/sync`) and returns on the first reply: `/bridge/done` gives `sent: true`; `/bridge/fault` gives an `isError: true` result carrying the message's address and the reason, as text and as structured content. If nothing replies within a guard delay, the result is an error saying osc-bridge does not answer at that address. |
| `list_roles` | List the roles of a driver's controls (OSD paths such as `filter/cutoff`), most important first, each with its address, range, default and steps. Arguments `device`, optional `limit`. A driver without roles (OSD 0.5) gives an empty list. |
| `set_by_role` | Set a control by its role, without knowing its address or numbers. Arguments `device`, `role`, `value` — normalised from 0 to 1 on the range of the role, scaled by osc-bridge on the range and steps the driver gives; the same request works on any driver that carries the role. Sent like `send` (same acknowledgement, same refusals). Refused, sending nothing: a value outside 0 to 1 (the interval is named); a role the driver does not have (`isError`, the role named, with the nearest roles); a role two controls carry (both addresses named); a role whose control writes the device's memory (names `restore_presets`, which carries `confirm_overwrite`). No consent argument. |
| `get_by_role` | Read a control's current value by its role: sends the read request the driver declares and returns the reply's value, from 0 to 1 on the range of the role. Arguments `device`, `role`. A driver that declares no read for the control says it does not report it; an unknown or doubled role is refused as for `set_by_role`. |
| `get_status` | Round-trip `/bridge/status` to a running bridge and report the replies. |
| `add_link` | Link an address of one device to an address of another on a running bridge, optionally scaling the first argument (`map`: input range, output range); effective at the next message, no restart. Refused, naming the address or the chain, for an unknown device, an address its driver does not have, a loop or more than four hops. |
| `remove_link` | Remove a link from a running bridge; refused, naming it, when it does not exist. |
| `list_links` | List a running bridge's links, from the configuration file and added live, each with its source, target, scaling and origin. |
| `detect_hardware` | Find the MIDI devices plugged into this computer: list ports, read the USB ids (`usb`: `vendor_id`, `product_id`) of each USB device's card (Linux), ask each device who it is (standard MIDI Identity Request, only that request leaves without consent; every maker request is held back and listed in `held_back` unless the person agreed for that port: `consent_ports`, to be passed only after asking them), have the SyntHub server match the answers against the catalogue (offline: the drivers cached here), with the firmware read from them (drivers matching equally are listed in `equally_matching`, the one the others share their MIDI with first), and return a ready-to-run `osc-bridge run` command with the right port numbers. |
| `backup_presets` | Record a device's SysEx dump (its presets) into a `.syx` file. Argument `device` is required (a catalogue id or OSC prefix; a device the catalogue does not know is refused, with the invitation to describe it). The driver says which messages are its sounds, taken on any channel or device number; the dump is started from the device's menu, or, with `out_port`, by the driver's dump request. |
| `restore_presets` | Send a `.syx` file back to a device. Argument `device` is required. Overwrites its memory, so the tool refuses unless `confirm_overwrite: true` (the client should ask the user). The driver sets the pauses between messages; a bank it says cannot be written is refused before any message, naming the bank; the channel and device number are written as configured. `bank` and `number` give the place of a sound that carries none. |

The drivers on this machine (the cache, or `--devices-dir`) are indexed at
startup, and the cache again each time a driver is fetched into it.

## Linked to the SyntHub server

The catalogue lives on the SyntHub server; unless `--offline`, your copy is
linked to it.

- **One driver at a time.** `list_devices` searches the catalogue on the
  server; `detect_hardware` asks it which identity requests to send, sends
  it the names of your MIDI ports and the devices' answers, and gets the
  matching drivers back; a tool that
  needs a driver (`list_routes`, `get_device_docs`, `export_device`, a report)
  fetches it once per session, with its notes and the driver it shares its
  MIDI with, into `<cache>/osc-bridge/drivers` (`~/.cache` on Linux,
  `%LOCALAPPDATA%` on Windows). Each file is kept there with the ETag the
  server gave; a later session asks whether it changed (`If-None-Match`) and
  downloads it again only then. Nothing else of the catalogue is asked
  for. These requests present the anonymous account of this machine (the server
  delivers one on first use). Offline, or when the server cannot
  be reached, the cached drivers are all there is. (A folder you point to
  with `--devices-dir` is used as it is.)
- **A newer osc-bridge.** When the server marks your version as deprecated
  (RFC 9745 `Deprecation`), osc-bridge says once, on standard error and in
  the result of the next tool call, that a newer osc-bridge exists and
  where to get it.
- **Trust.** `list_devices` gives each driver's `usage_status`: how far other
  users confirmed it on the real device.
- **More tools**, run on the server under an anonymous account kept on
  your machine: `report_result` (does this driver work on your device and
  firmware?), `report_mapping` (which message a control really sends),
  `get_trust`, `submit_driver`, `submit_fix` and `get_contribution`. What you
  report improves the drivers for everyone; see
  [API for agents](https://synthub.roomi-fields.com/api/#reports-and-trust).
- **Shared presets** (CC BY-SA 4.0): sounds, sequences, kits, projects,
  banks and MIDI clips other users made. `find_presets` and `get_preset`
  search them, `load_preset` plays one into a device plugged into your
  computer (device data only once you agreed), `report_preset` says how it
  went; `take_sound` takes the sound your synth is playing, with its name,
  straight from the synth (about 90 synths, through SyntHub's librarian);
  `share_preset` shares what you made (from a `.syx`, a `.mid`,
  settings by driver address, with an audio preview), `update_preset`
  publishes a new version, `withdraw_preset` takes it back; `comment_preset`,
  `star_preset`, `preset_collection`, `follow_author` and `set_profile`
  for the rest. See
  [shared presets](https://synthub.roomi-fields.com/api/#shared-presets).

Your MIDI ports and your devices' traffic never leave your machine. Sharing
is on by default; you choose what stays on your machine in a commented
settings file, `<config>/osc-bridge/settings.toml`, created the first time
osc-bridge starts: `offline = true` cuts the link, and `private = [...]` lists
devices (an id, an OSC prefix, or `vendor/*`) about which nothing is sent.
`osc-bridge privacy` shows the settings and the file's path. Details:
[privacy](https://github.com/roomi-fields/osc-bridge/blob/main/PRIVACY.md).

## Typical workflow

The path from cold start to controlling a device:

1. `detect_hardware` — see what is plugged in and which driver matches
   (or `list_devices` to search the catalogue by name).
2. `list_routes("minilogue")` — see every OSC address it accepts.
3. (separately) start the bridge with the command `detect_hardware` returned,
   e.g. `osc-bridge run --device korg/minilogue --out-port 3 --in-port 2`
   (a driver id: `run` fetches it, or takes it from the cache)
4. `send("/minilogue/filter/cutoff", [90])` — drive it (CC 43, range 0–127:
   values are the device's own integers, not 0–1).

Before an experiment on a device's sounds, `backup_presets` saves its memory
to a `.syx` file; `restore_presets` puts it back.

`get_device_docs` fills in the device-specific quirks (port conventions,
argument formats, host-side setup for software targets).

A concrete prompt: *"What can the minilogue do? Sweep its filter cutoff."* —
Claude calls `list_routes("minilogue")`, then `send` a few times. No glue
code.

## Driving software, not just hardware

Because osc-bridge treats DAWs and live-coding environments as first-class
devices, the same MCP tools control software targets:

- *"Set Ableton's tempo to 124 and arm track 1"* → `send("/ableton/transport/tempo", [124.0])`, `send("/ableton/track/1/arm", [true])`
- *"Trigger scene 3 in Bitwig"* → `send("/bitwig/scene/3/launch")`

The software-target drivers each carry a companion `.md` (fetchable via
`get_device_docs`) documenting the host-side setup — installing AbletonOSC,
DrivenByMoss, Reaper's OSC control surface, etc.

## See also

- [Open Synth Descriptors](https://github.com/roomi-fields/open-synth-descriptors) — how device drivers are written.
- [`TUTORIAL_FIRST_DEVICE.md`](TUTORIAL_FIRST_DEVICE.md) — write your first driver in 30 minutes.
- [Main README](../README.md) — osc-bridge as a standalone bridge, the orchestrator, the device catalogue.
- Interactive device browser: <https://synthub.roomi-fields.com/>
