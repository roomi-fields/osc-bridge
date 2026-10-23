# osc-bridge MCP server — a MIDI MCP and OSC MCP

`osc-bridge` ships a [Model Context Protocol](https://modelcontextprotocol.io)
server. It lets Claude — or any MCP client — discover a catalogue of **963
music devices**, read each device's OSC surface, and send OSC to control
synthesizers and DAWs.

It is both:

- a **MIDI MCP** — 963 hardware synthesizers exposed over MIDI / SysEx
  (every Prophet, Oberheim, Moog, Yamaha DX family, MatrixBrute, Hydrasynth,
  Virus TI, Digitone…), and
- an **OSC MCP** — DAWs and live-coding environments over OSC (Ableton Live,
  Bitwig Studio, Reaper, Sonic Pi, SuperCollider, Pure Data, TouchDesigner,
  VCV Rack).

The MCP server is one consumption mode of osc-bridge — the same bridge is also
usable from OSC clients, the CLI, and the multi-device orchestrator. See the
[main README](../README.md) for the whole picture.

## Install

No global install needed — the MCP server runs straight from npm:

```bash
npx -y @roomi-fields/osc-bridge mcp
```

`npm install` downloads the prebuilt native binary for your platform
(Linux x64, Windows x64, macOS arm64 / x64) from the matching GitHub release.

## Configure your MCP client

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

- `--devices-dir <path>` — root to scan for device drivers (default `devices`).
- `--default-target <host:port>` — where `send` / `get_status` point when no
  `target` argument is given (default `127.0.0.1:7777`, the standard
  `osc-bridge run` bind).
- `--offline` — do not contact the osc-bridge server (also
  `OSC_BRIDGE_OFFLINE=1`); see *Linked to the osc-bridge server* below.
- `--http <host:port>` — serve MCP over HTTP (endpoint `/mcp`) instead of
  stdio, for agents that reach the catalogue over the network. Only the
  catalogue tools and the usage reports (`report_result`, `get_trust`) are
  offered, and `--data-dir <dir>` keeps the records on disk; see
  [API for agents](api.md).

## The eight tools

| Tool | What it does |
|---|---|
| `list_devices` | Find device drivers — name, vendor, slug, kind (hardware/software), source tier, documented firmware. Optional `query` (words matched in vendor, name and slug), `vendor`, `kind`, `limit`. |
| `get_device_docs` | Fetch the markdown companion guide for one device (by slug). |
| `list_routes` | Return a device's full OSC surface — declarative commands, CC/NRPN params, SysEx params, named drum sounds, reply patterns. |
| `send` | Send a single OSC message to a running `osc-bridge` instance (typed args: int, float, string, bool). |
| `get_status` | Round-trip `/bridge/status` to a running bridge and report the replies. |
| `detect_hardware` | Find the MIDI devices plugged into this computer: list ports, ask each device who it is (standard MIDI Identity Request — changes nothing on the device), match the catalogue, and return a ready-to-run `osc-bridge run` command with the right port numbers. |
| `backup_presets` | Record a device's SysEx dump (its presets) into a `.syx` file — the dump is started from the device's menu, or by a dump-request message. |
| `restore_presets` | Send a `.syx` file back to a device. Overwrites its memory, so the tool refuses unless `confirm_overwrite: true` (the client should ask the user). |

The catalogue is indexed once at startup, so `list_devices` and slug lookups
are instant — no per-call directory walk.

## Linked to the osc-bridge server

Your copy stays current with what the community builds: unless `--offline`,
it is linked to the hosted osc-bridge server.

- **An up-to-date catalogue.** At startup it fetches the list of the server's
  drivers and downloads, in the background, the ones that changed since your
  copy was released — drivers other users and agents added or fixed. They are
  kept in `<cache>/osc-bridge/catalogue` and used from then on. (A catalogue
  you point to with `--devices-dir` is left alone.)
- **Trust.** `list_devices` gives each driver's `usage_status`: how far other
  users confirmed it on the real device.
- **Six more tools**, run on the server under an anonymous account kept on
  your machine: `report_result` (does this driver work on your device and
  firmware?), `report_mapping` (which message a control really sends),
  `get_trust`, `submit_driver`, `submit_fix` and `get_contribution`. What you
  report improves the drivers for everyone; see
  [API for agents](api.md#reports-and-trust).

Your MIDI ports and your devices' traffic never leave your machine. Details:
[privacy](https://github.com/roomi-fields/osc-bridge/blob/main/PRIVACY.md).

## Typical workflow

The path from cold start to controlling a device:

1. `detect_hardware` — see what is plugged in and which driver matches
   (or `list_devices` to search the catalogue by name).
2. `list_routes("minilogue")` — see every OSC address it accepts.
3. (separately) start the bridge with the command `detect_hardware` returned,
   e.g. `osc-bridge run --device devices/korg/minilogue.json --out-port 3 --in-port 2`
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

- [`DEVICE_JSON_SCHEMA.md`](DEVICE_JSON_SCHEMA.md) — how device drivers are written.
- [`TUTORIAL_FIRST_DEVICE.md`](TUTORIAL_FIRST_DEVICE.md) — write your first driver in 30 minutes.
- [Main README](../README.md) — osc-bridge as a standalone bridge, the orchestrator, the device catalogue.
- Interactive device browser: <https://osc-bridge.roomi-fields.com/>
