---
date: 2026-10-22
authors: [romain]
title: How to Let Claude Control Your Hardware Synth (MCP)
description: >-
  Claude can control a hardware synth through osc-bridge's MCP server: detect the synth, read its named controls, start the bridge, send. Setup and limits.
slug: control-hardware-synth-with-claude-mcp
---

# How to Let Claude Control Your Hardware Synth (MCP)

To let Claude control a hardware synthesizer, give it an MCP server that knows your synth's MIDI implementation and can reach its MIDI port. osc-bridge does this with a catalogue of 963 device drivers: Claude finds the synth plugged into your computer, reads its named controls (`/filter/cutoff` rather than "CC 43"), and sends OSC messages that a running bridge turns into MIDI CC, NRPN or SysEx. This post walks through the setup, a real conversation, and the limits.

<!-- more -->

## What do you need?

- A synth connected over USB MIDI or a MIDI interface, with MIDI receive enabled on a known channel.
- osc-bridge v0.13.0 or later for the hardware tools (`detect_hardware`, `backup_presets`, `restore_presets`). The npm package currently on the registry is older and only has the first five tools, so build from source for now:

```bash
git clone https://github.com/roomi-fields/osc-bridge
cd osc-bridge
cargo build --release
./target/release/osc-bridge list        # lists your MIDI ports
```

The [getting-started guide](../../getting-started.md) covers other install options.

- An MCP client: Claude Desktop, Claude Code, Cursor or anything else that speaks MCP over stdio.

## How do you configure the MCP server?

You point your MCP client at the `osc-bridge mcp` subcommand. The configuration from the [MCP guide](../../MCP.md), for Claude Desktop's `claude_desktop_config.json`, is:

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

With a source build, call the binary directly. It finds the device catalogue on its own: in the current directory, then next to the executable and in its parent directories, so a binary in `osc-bridge/target/release/` uses `osc-bridge/devices` wherever the client starts it (`--devices-dir` overrides this):

```jsonc
{
  "mcpServers": {
    "osc-bridge": {
      "command": "/home/you/osc-bridge/target/release/osc-bridge",
      "args": ["mcp"]
    }
  }
}
```

In Claude Code, the same thing in one line:

```bash
claude mcp add osc-bridge -- /home/you/osc-bridge/target/release/osc-bridge mcp
```

The server exposes eight tools: `list_devices`, `get_device_docs`, `list_routes`, `send`, `get_status`, `detect_hardware`, `backup_presets` and `restore_presets`.

## How does Claude find which synth is plugged in?

The `detect_hardware` tool lists your MIDI ports, asks each device who it is, and matches the answers against the catalogue. The question is the standard MIDI Identity Request (`F0 7E 7F 06 01 F7`), the same "who are you?" message editors send on connection; it changes nothing on the device. The same report is available in a terminal as `osc-bridge detect`.

With a KORG minilogue on a port named `minilogue`, matching by port name only (`--no-probe`), the output is:

```
minilogue  (out 1, in 1)
  match: minilogue — devices/korg/minilogue.json (pencilresearch, score 47)
  match: generic_midi_mini — devices/generic-midi/generic_midi_mini.json (electra-preset, score 22)
  match: minilogue_xd — devices/korg/minilogue_xd.json (electra-preset, score 10)
  run:   osc-bridge run --device devices/korg/minilogue.json --out-port 1 --in-port 1
```

When the device answers the identity request, its manufacturer ID raises the score of drivers from the same maker, which separates models with similar names. Port names and numbers depend on your OS and on what else is plugged in.

## How does Claude learn what the synth can do?

The `list_routes` tool returns a device's whole OSC surface: commands, CC and NRPN parameters, SysEx parameters, named drum sounds and reply patterns. For the minilogue it returns 40 CC parameters such as `/filter/cutoff` (CC 43, range 0-127), `/filter/resonance` (CC 44) and `/envelopes/amp_eg_attack`, plus note routes (`/note/on`, `/note/off`, `/pitchbend`…). Each parameter carries an `expects` field (continuous, switch, discrete…) that tells the model whether it is a knob, a switch or a stepped choice. The [device schema](../../DEVICE_JSON_SCHEMA.md) describes every field.

`get_device_docs` returns the companion guide for drivers that have one, with device-specific quirks.

## How do you start the bridge?

You start the bridge yourself, in a separate terminal, with the command `detect_hardware` printed:

```bash
./target/release/osc-bridge run --device devices/korg/minilogue.json --out-port 1 --in-port 1
```

The bridge listens for OSC on `127.0.0.1:7777`, which is where the MCP `send` tool points by default (`--default-target` changes it). It keeps running until you stop it with Ctrl-C. Claude does not launch it for you.

## What does a conversation look like?

You ask in plain language; Claude chains the tools. A typical exchange:

> **You:** My minilogue is plugged in. Make the current sound darker and slower.
>
> **Claude:** *calls `detect_hardware`* — found a minilogue on out port 1, in port 1, driver `devices/korg/minilogue.json`. Start the bridge with `osc-bridge run --device devices/korg/minilogue.json --out-port 1 --in-port 1` and tell me when it is running.
>
> **You:** Running.
>
> **Claude:** *calls `list_routes("minilogue")`, then:*
> `send("/minilogue/filter/cutoff", [45])`
> `send("/minilogue/filter/resonance", [30])`
> `send("/minilogue/envelopes/amp_eg_attack", [80])`
> `send("/minilogue/note/on", [57, 100])` to let you hear it.

Each `send` becomes one MIDI message; `/minilogue/filter/cutoff 45` goes out as `B0 2B 2D` (CC 43, value 45, channel 1). Values are the device's own numbers, clamped to the parameter's range, not 0-1 fractions. The bridge's terminal logs every message it forwards, which is the quickest way to confirm Claude's calls arrived.

## Should you back up presets first?

Yes: before letting a model edit sounds, save the synth's memory with `backup_presets`. It listens on the device's MIDI input and writes every SysEx message it receives to a `.syx` file, stopping 3 seconds after the last one. You start the dump from the synth's menu during the call, or Claude passes the device's dump-request message (`request_hex`, from its manual) with an output port. The terminal equivalent:

```bash
osc-bridge backup --in-port minilogue --out backups/minilogue.syx
```

`restore_presets` sends the file back, paced at 50 ms between messages by default because older devices drop fast dumps. It overwrites the device's memory, so the tool refuses to run unless the call includes `confirm_overwrite: true`; a well-behaved client asks you first. Only synths that dump their memory over SysEx can be backed up this way.

## Limits

- **Most drivers are not tested on hardware.** Five drivers are verified (MiniLab 3, Electra One MK2, Push 3, a partial MatrixBrute, and Ableton Live via AbletonOSC). Most of the rest are derived from the Electra One preset library (588) or from [MIDI Guide](https://midi.guide) (376). The mapping is usually right, but a CC number or range can be wrong for your firmware. The [catalogue](../../catalogue.md) shows each driver's source.
- **The bridge must be running.** `send` is a fire-and-forget UDP message; if no bridge listens on the target port, nothing happens and nothing reports an error.
- **MIDI ports must be free.** On Windows, a MIDI port can usually be opened by one program at a time. Close your DAW or the vendor's editor, and stop the bridge before `backup_presets` or `restore_presets`, which open the ports themselves.
- **Not every device answers the identity request.** Many older instruments stay silent; matching then relies on the port name alone, and behind a generic MIDI interface the port name says nothing about the synth.
- **No musical timing.** Each `send` is one message at the moment the model calls it. Claude can set sounds and play test notes, not perform a tightly timed sequence.
- **No read-back.** Claude does not see what the synth currently holds; it sets values blindly unless the driver declares reply patterns and you watch the bridge's OSC output.

## FAQ

**Can Claude control any MIDI synth?**
It can send to any device with a driver in the catalogue. For a synth without one, a driver is a JSON file you can write by following the [device schema](../../DEVICE_JSON_SCHEMA.md).

**Does it work with Claude Desktop and Claude Code?**
Yes. Both start `osc-bridge mcp` as a stdio server; only the configuration format differs.

**Can it break my synth?**
Sending CCs usually changes only the current sound, like turning its knobs. Saving a patch or restoring a `.syx` file overwrites memory, which is why `backup_presets` comes first and `restore_presets` requires explicit confirmation.

**Does it also control DAWs?**
Yes. The same `send` tool reaches Ableton Live, Bitwig, Reaper, Sonic Pi, SuperCollider, Pure Data, TouchDesigner and VCV Rack through their OSC drivers. Source and issues are on [GitHub](https://github.com/roomi-fields/osc-bridge).
