# osc-bridge MCP server — a MIDI MCP and OSC MCP

`osc-bridge` ships a [Model Context Protocol](https://modelcontextprotocol.io)
server. It lets Claude — or any MCP client — search the SynthHub catalogue of
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

The SynthHub server runs this MCP server for you, with the catalogue, trust
from real use, and a relay to the hardware on your machine:

- **Claude** (web, desktop): Settings → Connectors → *Add custom connector* →
  `https://mcp.synthhub.roomi-fields.com/mcp`
- **Claude Code**: `claude mcp add --transport http synthhub https://mcp.synthhub.roomi-fields.com/mcp`
- **Cursor and other clients**: `{"mcpServers": {"synthhub": {"url": "https://mcp.synthhub.roomi-fields.com/mcp"}}}`

Your synth joins through the relay page,
[synthhub.roomi-fields.com/connect](https://synthhub.roomi-fields.com/connect/).
Tools and what the server records: [API for agents](https://synthhub.roomi-fields.com/api/).

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
  instead of those of the SynthHub catalogue.
- `--default-target <host:port>` — where `send` / `get_status` point when no
  `target` argument is given (default `127.0.0.1:7777`, the standard
  `osc-bridge run` bind).
- `--offline` — do not contact the SynthHub server (also
  `OSC_BRIDGE_OFFLINE=1`); see *Linked to the SynthHub server* below.

## The nine tools

| Tool | What it does |
|---|---|
| `list_devices` | Find device drivers — name, vendor, slug, kind (hardware/software), source tier, documented firmware. Optional `query` (words matched in vendor, name and slug), `vendor`, `kind`, `limit`. |
| `get_device_docs` | Fetch the markdown companion guide for one device (by slug). |
| `list_routes` | Return a device's full OSC surface — declarative commands, CC/NRPN params, SysEx params, named drum sounds, reply patterns. |
| `export_device` | Write one driver (`device`: id or slug) as `oscquery` (its OSC namespace, OSCQuery JSON) or `midnam` (its MIDI names, MIDI Name Document XML), with its sources' credit and licence. See [EXPORT.md](EXPORT.md). |
| `send` | Send a single OSC message to a running `osc-bridge` instance (typed args: int, float, string, bool). |
| `get_status` | Round-trip `/bridge/status` to a running bridge and report the replies. |
| `detect_hardware` | Find the MIDI devices plugged into this computer: list ports, ask each device who it is (standard MIDI Identity Request, then the makers' own requests the drivers' identity blocks name — changes nothing on the device), have the SynthHub server match the answers against the catalogue (offline: the drivers cached here), with the firmware read from them (drivers matching equally are listed in `equally_matching`, the one the others share their MIDI with first), and return a ready-to-run `osc-bridge run` command with the right port numbers. |
| `backup_presets` | Record a device's SysEx dump (its presets) into a `.syx` file — the dump is started from the device's menu, or by a dump-request message. |
| `restore_presets` | Send a `.syx` file back to a device. Overwrites its memory, so the tool refuses unless `confirm_overwrite: true` (the client should ask the user). |

The drivers on this machine (the cache, or `--devices-dir`) are indexed at
startup, and the cache again each time a driver is fetched into it.

## Linked to the SynthHub server

The catalogue lives on the SynthHub server; unless `--offline`, your copy is
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
  for. These requests carry no account. Offline, or when the server cannot
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
  [API for agents](https://synthhub.roomi-fields.com/api/#reports-and-trust).
- **Shared presets** (CC BY-SA 4.0): sounds, sequences, kits, projects,
  banks and MIDI clips other users made. `find_presets` and `get_preset`
  search them, `load_preset` plays one into a device plugged into your
  computer (device data only once you agreed), `report_preset` says how it
  went; `take_sound` takes the sound your synth is playing, with its name,
  straight from the synth (about 90 synths, through SynthHub's librarian);
  `share_preset` shares what you made (from a `.syx`, a `.mid`,
  settings by driver address, with an audio preview), `update_preset`
  publishes a new version, `withdraw_preset` takes it back; `comment_preset`,
  `star_preset`, `preset_collection`, `follow_author` and `set_profile`
  for the rest. See
  [shared presets](https://synthhub.roomi-fields.com/api/#shared-presets).

Your MIDI ports and your devices' traffic never leave your machine. Sharing
is on by default; you choose what stays on your machine in a commented
settings file, `<config>/osc-bridge/settings.toml`, created the first time
osc-bridge starts: `offline = true` cuts the link, and `private = [...]` lists
devices (an id, a slug, or `vendor/*`) about which nothing is sent.
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

- [`DEVICE_JSON_SCHEMA.md`](DEVICE_JSON_SCHEMA.md) — how device drivers are written.
- [`TUTORIAL_FIRST_DEVICE.md`](TUTORIAL_FIRST_DEVICE.md) — write your first driver in 30 minutes.
- [Main README](../README.md) — osc-bridge as a standalone bridge, the orchestrator, the device catalogue.
- Interactive device browser: <https://synthhub.roomi-fields.com/>
