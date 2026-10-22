---
title: osc-bridge — control any synth by name over OSC, MIDI and MCP
description: >-
  Open-source OSC ↔ MIDI/SysEx bridge and MCP server for 963 synthesizers,
  drum machines and DAWs. Drive hardware by parameter name from Claude,
  SuperCollider, Sonic Pi, TouchDesigner or a web page.
schema_type: WebPage
hide:
  - navigation
---

# osc-bridge

**osc-bridge turns every synthesizer, drum machine and DAW in its catalogue
into one named OSC surface.** Send `/minilab3/pad/0/color 127 0 0` or
`/drm1_mkiv/drum/kick 110` instead of looking up CC numbers and SysEx bytes: the
bridge translates both ways, over MIDI, SysEx or OSC. It ships an
[MCP server](MCP.md), so Claude and other AI assistants can find, read and
play your gear without any mapping on your side. Free and open source
(GPL-3.0), a single binary for Linux, macOS and Windows.

[Get started](getting-started.md){ .md-button .md-button--primary }
[Browse the 963 devices](catalogue.md){ .md-button }

## What can you do with it?

- **Ask Claude to play your synth.** `detect_hardware` finds what is plugged
  in, `list_routes` reads its parameters, `send` moves them.
  → [Control a hardware synth with Claude](blog/posts/control-hardware-synth-with-claude-mcp.md)
- **Live-code hardware by parameter name** from SuperCollider, Sonic Pi or
  TouchDesigner — the bridge sends the right CC, NRPN or SysEx.
  → [SuperCollider and Sonic Pi](blog/posts/supercollider-sonic-pi-hardware-synth.md)
- **Play drum machines by sound name** — 15 drum maps, 476 named sounds.
  → [Trigger drum sounds by name](blog/posts/drum-machine-sounds-by-name-osc.md)
- **Back up and restore presets** as `.syx` files.
  → [SysEx preset backup](blog/posts/backup-synth-presets-sysex.md)
- **Drive hardware from a web page** — OSC over WebSocket, no Web MIDI.
  → [From a web page to a synth](blog/posts/browser-web-audio-hardware-synth-websocket.md)
- **Run several devices in one process** and route a knob on one to a
  parameter on another (`osc-bridge orchestrate`).

## How does it work?

Each device is one JSON file: its CC and NRPN numbers, SysEx frame templates
and drum sounds, each under a readable OSC address. The bridge loads the file,
listens for OSC on `127.0.0.1:7777`, sends MIDI to the device, and turns what
the device sends back (knobs, pads, SysEx replies) into OSC for your clients.

```bash
osc-bridge detect                      # what is plugged in, and which driver fits
osc-bridge run --device devices/arturia/minilab3.json --out-port 4 --in-port 0
osc-bridge osc-send /minilab3/display/text "Hello" "World"
```

## How far can you trust a driver?

Each driver states its source. ✅ means tested on the device; 📘 comes from
the manufacturer's documentation; 📡 from a software's OSC API; 🎛️ from an
Electra One community preset; 📦 from [MIDI Guide](https://midi.guide)
community data (CC BY-SA 4.0). Most of the catalogue is 🎛️ or 📦 — useful,
not yet tested on hardware. Every device page says which it is, and a short
test on your own unit [makes it ✅ for everyone](https://github.com/roomi-fields/osc-bridge/blob/main/CONTRIBUTING.md).
