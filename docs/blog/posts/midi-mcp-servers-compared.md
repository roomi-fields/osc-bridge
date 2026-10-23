---
date: 2026-10-22
authors: [romain]
title: "MIDI MCP Servers in 2026: What Each One Can Drive"
description: >-
  Six MIDI and music MCP servers compared: virtual-port note senders, single-device controllers, a deep multi-device editor, an Ableton server, osc-bridge.
slug: midi-mcp-servers-compared
---

# MIDI MCP Servers in 2026: What Each One Can Drive

A MIDI MCP server lets an AI model such as Claude send MIDI, and sometimes receive it, through the Model Context Protocol. The open-source servers differ mainly in what they drive: some send notes to a virtual port for your DAW, some control one or a few specific instruments in depth, one controls Ableton Live itself, and osc-bridge covers a large catalogue of synths with shallower, mostly untested mappings. This comparison reads each project's README as of October 2026; pick by the device you own and the job you want done.

<!-- more -->

## Which servers are compared?

Six open-source projects, chosen because each is a working MCP server with public documentation about music or MIDI control:

| Server | Drives | Messages (documented) | MIDI in | License |
|---|---|---|---|---|
| sandst1/mcp-server-midi | apps, via a virtual port | notes, CC, sequences | no | MIT |
| nanassound/midi_ctrl | Arturia MicroFreak | CC, oscillator type | no | MIT |
| TheAndrewStaker/mcp-midi-control | Fractal, Hydrasynth… | CC, NRPN, SysEx, PC, notes | read-back | Apache-2.0 |
| thias42/midi-mcp | any port, listen and play | notes, chords, CC, PC, `.mid` | yes | MIT |
| ahujasid/ableton-mcp | Ableton Live itself | none (socket to Live) | n/a | MIT |
| roomi-fields/osc-bridge | 963 drivers + 8 DAWs/tools | CC, NRPN, notes, SysEx, OSC | via OSC | AGPL-3.0 |

"MIDI in" means the server reports incoming MIDI to the model; osc-bridge sends it to OSC clients, not to the model. Languages: Python (sandst1, thias42, ahujasid), Elixir (nanassound), TypeScript (TheAndrewStaker), Rust (osc-bridge). Columns only list what each README states; a blank in a README is not counted as a feature.

## What does each server do?

### sandst1/mcp-server-midi

[mcp-server-midi](https://github.com/sandst1/mcp-server-midi) creates a virtual MIDI output port named "MCP MIDI Out" and lets the model send Note On, Note Off, Control Change and timed note sequences into it. You select that port as an input in your DAW or software synth. It is a small Python server reached over Server-Sent Events (`http://localhost:8123/sse`). It knows nothing about specific instruments: CC numbers are up to the model.

### nanassound/midi_ctrl

[midi_ctrl](https://github.com/nanassound/midi_ctrl) is built for one synth, the Arturia MicroFreak. Its tools are `list_ports`, `microfreak_cc` (one or several CCs, with an optional delay) and `microfreak_set_oscillator`, which switches between the MicroFreak's 22 named oscillator types. SysEx and program change are listed as future work. It runs as an HTTP MCP server written in Elixir, with a prebuilt release for Apple Silicon Macs.

### TheAndrewStaker/mcp-midi-control

[mcp-midi-control](https://github.com/TheAndrewStaker/mcp-midi-control) is the deepest of the group for the devices it supports. Its README reports 55 tools, values in front-panel units (knob positions, dB, ms) rather than raw bytes, and writes that count as done only after the device acknowledges them. It lists the Fractal AM4, Axe-Fx II XL+, ASM Hydrasynth Explorer and Boss VE-500 as hardware-verified, and a longer list (Axe-Fx III, FM3, FM9, Novation Circuit Tracks, Roland SPD-SX and others) as community beta. Preset handling is careful: byte-exact backups, a `confirm_overwrite` gate, and warnings before discarding unsaved edits. Generic tools (`send_cc`, `send_nrpn`, `send_sysex`, clock and song position) cover other USB MIDI devices at the raw level. The README describes it as pre-release.

### thias42/midi-mcp

[midi-mcp](https://github.com/thias42/midi-mcp) is about playing music with the model rather than editing sounds. The `listen` tool records what you play on a MIDI keyboard (notes, timing, velocity, CCs) until you pause; `play_notes`, `play_chords`, `replay` and `play_midi_file` send music back; `save_take` writes a `.mid` file. The README's examples are harmonizing a melody and looping a backing track while you improvise. It installs with `uvx` and is tested on macOS with an IAC bus; Windows is untested.

### ahujasid/ableton-mcp

[ableton-mcp](https://github.com/ahujasid/ableton-mcp) controls Ableton Live from the inside. A Remote Script in Live opens a socket and the MCP server sends it JSON commands: create MIDI and audio tracks, create and edit clips, add notes, control transport and tempo, load instruments and effects from Live's browser. It does not address external hardware. Its README notes anonymous telemetry is on by default and can be disabled with an environment variable.

### roomi-fields/osc-bridge

[osc-bridge](https://github.com/roomi-fields/osc-bridge) is a Rust bridge from OSC to MIDI and SysEx with an MCP server built in (`osc-bridge mcp`). Its eight tools are `list_devices`, `get_device_docs`, `list_routes`, `send`, `get_status`, `detect_hardware`, `backup_presets` and `restore_presets`. Each device is one JSON driver mapping named addresses (`/minilogue/filter/cutoff`) to CC, NRPN or SysEx frames; the catalogue holds 963 devices from 212 vendors, plus OSC drivers for Ableton Live, Bitwig, Reaper, Sonic Pi, SuperCollider, Pure Data, TouchDesigner and VCV Rack. Since v0.13.0, `detect_hardware` identifies connected synths with the standard MIDI Identity Request, and `backup_presets` / `restore_presets` save and restore SysEx dumps, with restore gated by `confirm_overwrite: true`.

The trade-off is depth. Five drivers are verified (MiniLab 3, Electra One MK2, Push 3, a partial MatrixBrute, Ableton Live via AbletonOSC); most others are derived from the Electra One preset library or from [MIDI Guide](https://midi.guide), and have not been tested on the instrument. The model sends one message per `send` call to a bridge you start separately (`osc-bridge run …`), and it gets no read-back of the synth's state. The setup is in the [MCP guide](../../MCP.md); the drivers are browsable in the [catalogue](../../catalogue.md).

## Which one should you pick?

Pick by the device and the task, not by tool count:

- **You want the model to write notes or chords into your DAW.** sandst1/mcp-server-midi is the smallest step: a virtual port, notes and CCs. If you also want the model to hear what you play, thias42/midi-mcp listens and plays back.
- **You produce in Ableton Live and want tracks, clips and arrangements built for you.** ahujasid/ableton-mcp works inside Live. osc-bridge also drives Live through AbletonOSC, but at the level of transport, tracks and scenes, not arrangement.
- **You own a Fractal unit, a Hydrasynth Explorer or another device on its list.** TheAndrewStaker/mcp-midi-control is the better choice: front-panel units, verified writes and preset management that osc-bridge does not attempt.
- **You own a MicroFreak and want only that.** nanassound/midi_ctrl is focused and simple; osc-bridge also has MicroFreak drivers.
- **You own a synth none of the above names, or several from different vendors.** osc-bridge is the one most likely to have a driver for it, and the same OSC surface works from live-coding environments and scripts as well as from the model. Check the driver's source tier first, and back up presets before experimenting.

## Limits of this comparison

- It is based on each project's README in October 2026, not on running every server on hardware. READMEs change; check the repositories.
- It is written by the osc-bridge author. The other projects' claims are reported as their READMEs state them, and osc-bridge's weaknesses are listed above.
- Only open-source MCP servers with public documentation are included. Related tools that are not MCP servers, such as [MIDI Guide](https://midi.guide) (a CC/NRPN database with a free read-only API) or the [Electra One](https://electra.one/) hardware controller and its preset library, are left out of the table.

## FAQ

**What is a MIDI MCP server?**
An MCP server whose tools send or receive MIDI, so a model such as Claude can play notes or change a synth's settings by calling tools instead of you writing code.

**Can Claude control a hardware synth directly?**
Yes, with a server that opens hardware MIDI ports: mcp-midi-control, midi_ctrl, midi-mcp or osc-bridge. Virtual-port servers reach hardware only through a DAW or a MIDI router.

**Which servers can send SysEx?**
Among the six, the READMEs of mcp-midi-control (device codecs and `send_sysex`) and osc-bridge (templated frames and raw `/raw/syx`) document SysEx sending.

**Does any of them work without a DAW?**
All except ableton-mcp can talk to MIDI ports without a DAW; sandst1/mcp-server-midi needs some program to receive its virtual port.
