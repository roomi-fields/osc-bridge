# osc-bridge MCP server — a MIDI MCP and OSC MCP

`osc-bridge` ships a [Model Context Protocol](https://modelcontextprotocol.io)
server. It lets Claude — or any MCP client — discover a catalogue of **849
music devices**, read each device's OSC surface, and send OSC to control
synthesizers and DAWs.

It is both:

- a **MIDI MCP** — 849 hardware synthesizers exposed over MIDI / SysEx
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

## The five tools

| Tool | What it does |
|---|---|
| `list_devices` | Enumerate every device driver — name, vendor, slug, kind (hardware/software), source tier. |
| `get_device_docs` | Fetch the markdown companion guide for one device (by slug). |
| `list_routes` | Return a device's full OSC surface — declarative commands, CC/NRPN params, SysEx params, reply patterns. |
| `send` | Send a single OSC message to a running `osc-bridge` instance (typed args: int, float, string, bool). |
| `get_status` | Round-trip `/bridge/status` to a running bridge and report the replies. |

The catalogue is indexed once at startup, so `list_devices` and slug lookups
are instant — no per-call directory walk.

## Typical workflow

The four-step path from cold start to controlling a device:

1. `list_devices` — find the device you want (e.g. search "matrixbrute").
2. `list_routes("matrixbrute")` — see every OSC address it accepts.
3. (separately) start the bridge: `osc-bridge run --device devices/arturia/matrixbrute.json --out-port <N>`
4. `send("/matrixbrute/filter/cutoff", [0.7])` — drive it.

`get_device_docs` fills in the device-specific quirks (port conventions,
argument formats, host-side setup for software targets).

A concrete prompt: *"What can the MatrixBrute do? Sweep its filter cutoff."* —
Claude calls `list_routes("matrixbrute")`, then `send` a few times. No glue
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
- Interactive device browser: <https://roomi-fields.github.io/osc-bridge/>
