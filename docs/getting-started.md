---
title: Getting started with osc-bridge
description: >-
  Install osc-bridge, find your synth and its MIDI ports with osc-bridge
  detect, start the bridge and send your first OSC message — or plug it into
  Claude as an MCP server.
---

# Getting started

Five minutes from download to a synth answering your first OSC message.

## 1. Install

```bash
npm install -g @roomi-fields/osc-bridge     # downloads the prebuilt binary
```

Or download a binary from the [releases page](https://github.com/roomi-fields/osc-bridge/releases),
or build from source:

```bash
git clone https://github.com/roomi-fields/osc-bridge
cd osc-bridge && cargo build --release      # binary in target/release/
```

!!! note "Newest features from source"
    Hardware detection, preset backup, drum sounds by name and WebSocket
    clients arrived in v0.13.0. Until the next release is published, build
    from source to get them.

## 2. Find your device and its ports

```bash
osc-bridge detect
```

It lists MIDI ports, asks each device who it is (the standard MIDI Identity
Request — it changes nothing on the device) and prints the matching driver and
a ready-to-run line. Search the [catalogue](catalogue.md) if your device does
not answer.

## 3. Start the bridge

```bash
osc-bridge run --device devices/arturia/minilab3.json --out-port 4 --in-port 0 \
  --osc-client 127.0.0.1:8888
```

The bridge listens for OSC on `127.0.0.1:7777`. With `--in-port`, everything
the device sends (knobs, pads, SysEx replies) goes out as OSC to each
`--osc-client`. Add `--ws-bind 127.0.0.1:7890` for browser clients.

## 4. Send OSC

```bash
osc-bridge osc-send /minilab3/display/text "Hello" "World"
osc-bridge osc-listen --bind 127.0.0.1:8888    # watch what the device sends
```

Every device page lists its addresses; so does `osc-bridge inspect <json>`.

## 5. Or let Claude drive it

Add the MCP server to Claude Desktop, Claude Code or Cursor:

```jsonc
{ "mcpServers": { "osc-bridge": { "command": "npx", "args": ["-y", "@roomi-fields/osc-bridge", "mcp"] } } }
```

Then ask: *“What synth is plugged in, and what can it do?”* — see the
[MCP guide](MCP.md).

## Next

- [Add your own device](TUTORIAL_FIRST_DEVICE.md) in about 30 minutes.
- [Device JSON schema](DEVICE_JSON_SCHEMA.md) for everything a driver can say.
- [Blog](blog/index.md) for worked examples.
