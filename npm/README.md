# osc-bridge

**Declarative OSC bridge for music hardware and software** — and an MCP server
for driving it from an LLM.

osc-bridge translates [OSC](https://opensoundcontrol.stanford.edu/) to and from
the music world:

- **849 hardware synthesizers** via MIDI / SysEx — every Prophet, Oberheim,
  Moog, Yamaha DX family, MatrixBrute, Hydrasynth, Virus TI, Digitone…
- **Software targets** via native OSC — Ableton Live, Bitwig, Reaper, Sonic Pi,
  SuperCollider, Pure Data, TouchDesigner, VCV Rack.

Each device is one declarative JSON file — no recompile to add a synth. Drive it
from a live-coding environment, a DAW, a script, the CLI — or from **Claude (or
any MCP client) via the built-in MCP server**.

## Install

As an MCP server (no global install needed):

```bash
npx -y @roomi-fields/osc-bridge mcp
```

Or install the whole CLI globally:

```bash
npm install -g @roomi-fields/osc-bridge
osc-bridge --help
```

`npm install` downloads the prebuilt native binary for your platform
(Linux x64, Windows x64, macOS arm64/x64) from the matching GitHub release.

## Use as an MCP server

Add to your MCP client config (e.g. Claude Desktop
`claude_desktop_config.json`):

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

The MCP server exposes five tools — `list_devices`, `get_device_docs`,
`list_routes`, `send`, `get_status` — so the model can discover the 849-device
catalogue, read a device's OSC surface, and send OSC to a running bridge.

It's both a **MIDI MCP** (hardware synths over MIDI/SysEx) and an **OSC MCP**
(DAWs and live-coding environments over OSC).

## Full documentation

This is the npm distribution wrapper. Source, device catalogue, schema
reference, the 30-minute "first device" tutorial, and the interactive device
browser all live in the main repository:

**https://github.com/roomi-fields/osc-bridge**

## License

GPL-3.0-or-later.
