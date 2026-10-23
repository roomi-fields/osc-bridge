---
date: 2026-10-22
authors: [romain]
title: "OSC to MIDI and SysEx: Drive Hardware Synths from Any OSC Software"
description: >-
  osc-bridge converts OSC to MIDI CC, NRPN and SysEx using one JSON driver per synth, sends incoming MIDI back as OSC, and paces output. How it works, limits.
slug: osc-to-midi-sysex-bridge
---

# OSC to MIDI and SysEx: Drive Hardware Synths from Any OSC Software

An OSC to MIDI bridge listens for OSC messages on a UDP port and turns them into MIDI bytes for a hardware port. osc-bridge does this per device: each synth has a JSON driver that maps named OSC addresses (`/minilogue/filter/cutoff`) to CC, NRPN or SysEx frames, so any OSC-capable software (SuperCollider, Pure Data, TouchDesigner, a Python script, a browser) can drive the synth without knowing its MIDI implementation. MIDI coming back from the synth is decoded and sent to your OSC clients.

<!-- more -->

## Why not just send raw CC numbers?

You can, but you then carry the synth's MIDI chart in your code. A filter cutoff is CC 43 on one synth, CC 74 on another, a 14-bit CC pair or an NRPN on a third, and a SysEx parameter on a fourth. osc-bridge moves that knowledge into a driver file, one per device under `devices/<vendor>/`, and gives you one address per control. The catalogue has 963 devices from 212 vendors; browse it in the [device catalogue](../../catalogue.md).

Start a bridge for one synth:

```bash
osc-bridge run --device devices/korg/minilogue.json \
  --out-port 1 --in-port 1 \
  --bind 127.0.0.1:7777 --osc-client 127.0.0.1:8888
```

Port numbers come from `osc-bridge list` or `osc-bridge detect`. Then send OSC from anything:

```supercollider
~mg = NetAddr("127.0.0.1", 7777);
~mg.sendMsg("/minilogue/filter/cutoff", 64);   // CC 43 = 64  ->  B0 2B 40
~mg.sendMsg("/minilogue/note/on", 60, 100);    // Note On     ->  90 3C 64
```

For a quick test there is a built-in client: `osc-bridge osc-send /minilogue/filter/cutoff 64`.

## How do named addresses map to MIDI?

Each driver entry pairs an OSC path with a MIDI address and a value range, and the bridge builds the message. Three forms cover most synths:

| Driver entry | OSC sent | MIDI out |
|---|---|---|
| `"cc": 43, "range": [0,127]` | `/minilogue/filter/cutoff 64` | `B0 2B 40` |
| `"cc": 19, "cc_lsb": 51, "range": [0,16383]` | 14-bit value | CC 19 (MSB), then CC 51 (LSB) |
| `"nrpn_msb":3, "nrpn_lsb":115, "range":[0,16383]` | `/sub37/filter/cutoff 8192` | CC99=3, CC98=115, CC6=64, CC38=0 |

Values are the device's own numbers, clamped to the declared range; floats are truncated to integers, so send `64`, not `0.5`. A second argument overrides the channel for one call (`/minilogue/filter/cutoff 64 3`). When an entry declares both NRPN and CC addresses, the bridge uses NRPN.

Drivers with a `midi_out` section also accept performance routes under the device prefix: `/note/on`, `/note/off`, `/pitchbend`, `/aftertouch`, `/poly_aftertouch`, `/cc/{num}` and `/program_change`, each with an optional trailing channel. Drum machines with a drum map expose each voice by name: `/drm1_mkiv/drum/kick 110` sends Note On, note 36, channel 10, velocity 110. Velocity defaults to 100 and is clamped to 1-127; only Note On is sent.

Each entry may also carry an `expects` field (continuous, switch, momentary, trigger, clock, discrete). It describes the control on the device side and changes nothing in the bytes sent. The [device schema](../../DEVICE_JSON_SCHEMA.md) documents every field.

## How do you send SysEx from OSC?

SysEx is built from frame templates in the driver: the bridge substitutes your OSC arguments into placeholder bytes between the device's SysEx header and footer. The MiniLab 3 pad-color command, for example, is declared as:

```jsonc
{
  "osc": "/pad/{pad}/color",
  "args": [{"name":"pad","type":"u7","range":[0,7]},
           {"name":"r","type":"u7"}, {"name":"g","type":"u7"}, {"name":"b","type":"u7"},
           {"name":"mode","type":"enum","values":{"user":0,"arturia":1,"daw":2},"default":"user"}],
  "pre":   [{"bind":"pad_id","expr":"pad + 4"}],
  "frame": [2, "{mode}", 22, "{pad_id}", "{r}", "{g}", "{b}"]
}
```

With the header `F0 00 20 6B 7F 42` and footer `F7`, the message `/minilab3/pad/0/color 127 0 0` becomes:

```
F0 00 20 6B 7F 42 02 00 16 04 7F 00 00 F7
```

The path segment `{pad}` is captured, `mode` falls back to its default, and `pre` computes `pad_id`. Placeholders also cover 14-bit values (`{name:u14_msb}`, `{name:u14_lsb}`), zero-terminated ASCII (`{name:ascii}`, used for display text) and raw byte arrays. Devices with one opcode for hundreds of parameters declare the get/set frame once and a flat `params` table of addresses.

When a template is not enough (checksums, unusual scaling, stateful protocols), a driver can attach a Lua `transform` or `script`, sandboxed to 1 MiB of memory and 10 ms per call. `osc-bridge lint` warns on every use, because declarative entries are easier to review.

## Can you send raw SysEx?

Yes: every MIDI driver accepts `/<prefix>/raw/syx` with one string argument holding hex bytes.

```bash
osc-bridge osc-send /minilab3/raw/syx "F0 00 20 6B 7F 42 02 00 16 04 7F 00 00 F7"
```

The bridge parses whitespace-separated hex tokens (a `0x` prefix is accepted) and sends the message only if it starts with `F0` and ends with `F7`. Tokens that are not valid hex are skipped silently, so check the bridge log if a message seems lost. This route is the fallback for messages the driver does not describe; it bypasses all validation, so you are responsible for the bytes.

## Does it work in both directions?

Yes, when you pass `--in-port`: incoming MIDI is decoded and sent as OSC to every `--osc-client` (the option can be repeated). Channel messages follow the driver's `midi_in` templates, so turning the minilogue's cutoff knob produces:

```
/minilogue/cc/43 70 0
```

with the CC number in the address, then value and channel (0-15). Incoming SysEx is matched against the driver's `replies` patterns and emitted as named messages (the MiniLab 3 reports `/minilab3/param/value …` after a `/param/get`). SysEx that matches no pattern still arrives, as `/<prefix>/sysex/raw "F0 … F7"` in hex. `osc-bridge osc-listen --bind 127.0.0.1:8888` prints everything that comes back.

Browsers cannot send UDP, so `--ws-bind 127.0.0.1:7890` adds a WebSocket listener: binary frames carry the same raw OSC packets as UDP, and each connected page receives outbound events like an extra `--osc-client`. Under `orchestrate`, the same option is `ws_bind` in the `[osc]` section of `bridge.toml`.

## How does rate limiting work?

A driver can declare `rate_limit_hz` in its `device` block, and the bridge then leaves at least 1/rate seconds between MIDI messages it sends to that port. The MiniLab 3 driver sets 800 Hz and many catalogue drivers set 1000 Hz. An NRPN write counts as three or four messages. Outgoing messages wait in a queue of 1024; when the queue is full, new messages are dropped and the bridge logs `queue full`. This protects older synths from bursts, for instance a dense LFO stream from a live-coding loop, at the cost of latency under sustained load.

## How does it compare to generic OSC-MIDI converters?

Generic converters map messages one at a time, while osc-bridge ships the mapping per device. [OSCulator](https://osculator.net/) is a macOS application where you build OSC-to-MIDI mappings in a graphical editor. [TouchOSC Bridge](https://hexler.net/touchosc) is Hexler's utility that relays MIDI between the TouchOSC app and the computer's MIDI ports. Both are good tools for a hand-built control surface. Pure Data or Max patches are the do-it-yourself route.

osc-bridge differs in three ways: the mapping comes from a reviewable JSON file, often already in the catalogue; SysEx is templated rather than typed byte by byte; and it is a command-line program with no GUI, for Windows, Linux and macOS. If you want to draw a layout and map faders by hand, a GUI converter is the better fit.

## Limits

- **Most drivers are not tested on hardware.** Only five are verified; most others come from the Electra One preset library or from [MIDI Guide](https://midi.guide). Ranges taken from preset labels can differ from the raw values the synth expects.
- **Values are not normalised.** You send the device's numbers (0-127, 0-16383, or the driver's range), not 0.0-1.0.
- **OSC bundle timetags are ignored.** Messages in a bundle are forwarded on arrival, not scheduled.
- **No MIDI clock or transport generation**, and no sequencing: timing is whatever your OSC client sends.
- **MIDI ports may be exclusive.** On Windows a MIDI port can usually be opened by one program at a time, so the bridge and your DAW cannot both hold it. To drive several devices, run them all in one process with `osc-bridge orchestrate --config bridge.toml`.
- **Raw SysEx is unchecked.** A wrong byte in `/raw/syx` goes straight to the device.

## FAQ

**Is osc-bridge free?**
Yes, it is open source under AGPL-3.0-or-later. Binaries are on the [GitHub releases page](https://github.com/roomi-fields/osc-bridge/releases); see the [getting-started guide](../../getting-started.md).

**Can I add a synth that is not in the catalogue?**
Yes, by writing a JSON driver; no Rust code or rebuild is needed. The [device schema](../../DEVICE_JSON_SCHEMA.md) lists every field.

**Can it bridge OSC to a DAW instead of a synth?**
Yes. Software drivers forward OSC to Ableton Live (AbletonOSC), Bitwig (DrivenByMoss), Reaper, Sonic Pi, SuperCollider, Pure Data, TouchDesigner and VCV Rack.

**Can an AI model use it?**
Yes, through the built-in MCP server (`osc-bridge mcp`); see the [MCP guide](../../MCP.md).
