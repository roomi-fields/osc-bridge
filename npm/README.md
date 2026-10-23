# osc-bridge

**Declarative OSC bridge for music hardware and software** — and an MCP server
for driving it from an LLM.

osc-bridge translates [OSC](https://opensoundcontrol.stanford.edu/) to and from
the music world:

- **986 hardware synthesizers** via MIDI / SysEx — every Prophet, Oberheim,
  Moog, Yamaha DX family, MatrixBrute, Hydrasynth, Virus TI, Digitone…
- **Software targets** via native OSC — Ableton Live, Bitwig, Reaper, Sonic Pi,
  SuperCollider, Pure Data, TouchDesigner, VCV Rack.

Each device is one declarative JSON file — no recompile to add a synth. Drive it
from a live-coding environment, a DAW, a script, the CLI — or from **Claude (or
any MCP client) via the built-in MCP server**.

## Install

**Hosted MCP server, nothing to install** — add
`https://mcp.synthhub.roomi-fields.com/mcp` as a remote MCP server (a custom
connector in Claude; `claude mcp add --transport http synthhub https://mcp.synthhub.roomi-fields.com/mcp`
in Claude Code). It serves the up-to-date catalogue, trust from real use, and
a relay to your own hardware: <https://synthhub.roomi-fields.com/>.

**On your machine**, as an MCP server over stdio:

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

## Use as a local MCP server

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

Eight tools — `list_devices`, `get_device_docs`, `list_routes`, `send`,
`get_status`, `detect_hardware`, `backup_presets`, `restore_presets` — plus
the server's (reports, trust, contributions, shared presets), so the model can discover
the 986-device catalogue, read a device's OSC surface, and drive your devices.

It's both a **MIDI MCP** (hardware synths over MIDI/SysEx) and an **OSC MCP**
(DAWs and live-coding environments over OSC).

## Full documentation

This is the npm distribution wrapper. Source, device catalogue, schema
reference, the 30-minute "first device" tutorial, and the interactive device
browser all live in the main repository:

**https://github.com/roomi-fields/osc-bridge**

## License

AGPL-3.0-or-later.
