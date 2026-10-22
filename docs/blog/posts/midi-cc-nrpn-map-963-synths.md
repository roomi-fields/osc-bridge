---
date: 2026-10-22
authors: [romain]
title: A MIDI CC and NRPN Map for 963 Synths, Ready to Use over OSC
description: >-
  osc-bridge ships CC, NRPN and SysEx maps for 963 devices as JSON, each with its source and verification status, and sends any parameter by name over OSC.
slug: midi-cc-nrpn-map-963-synths
---

# A MIDI CC and NRPN Map for 963 Synths, Ready to Use over OSC

osc-bridge's device catalogue is a MIDI CC and NRPN chart for 963 devices (synths, drum machines, effects and controllers from 212 vendors), stored as one JSON file per device. Each parameter has a readable OSC address, such as `/sub37/filter/cutoff`, plus its CC number, 14-bit pair or NRPN address and its range. Each file says where its data came from, and most have not been tested on the hardware. Because the same files drive the bridge, you can look up a parameter and send it in the same step.

<!-- more -->

## Where does the data come from?

Every driver records its origin in a `_sources` field, and the catalogue counts them by tier:

| Tier | Devices | What it means |
|---|---|---|
| ✅ verified | 5 | Tested on the device or a running host: MiniLab 3, Electra One MK2, Push 3, MatrixBrute (partial), Ableton Live via AbletonOSC |
| 📘 vendor-doc | 3 | From a manufacturer's programmer's reference |
| 📡 OSC API | 7 | Software targets (Bitwig, Reaper, Sonic Pi, SuperCollider, Pure Data, TouchDesigner, VCV Rack) |
| 🎛️ Electra preset | 588 | From [Electra One community presets](https://app.electra.one/) |
| 📦 MIDI Guide | 376 | From [MIDI Guide](https://midi.guide) by Pencil Research and contributors |

Two sources do most of the work. [MIDI Guide](https://midi.guide) is an open dataset of MIDI CCs and NRPNs, maintained as one CSV per device in [pencilresearch/midi](https://github.com/pencilresearch/midi) and published under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/). osc-bridge converts those CSVs to its JSON format (OSC paths, ranges, `expects` hints, named drum sounds) and distributes the adapted files under GPL-3.0, which CC BY-SA 4.0 allows. Every such file records the licence, the attribution, the upstream CSV and the commit it came from.

The Electra One community has written hundreds of presets for its controller, and each preset encodes the parameters of one synth. Those drivers reflect the preset author's choices: which parameters are exposed, what they are called, which ranges are used.

The rule is one source per file. When two sources describe the same synth, they ship as two files (named `<device>.<source-tier>.fw-<version>.json`) instead of being merged, so contradictions stay visible.

## What does a driver file look like?

Here is an excerpt of `devices/moog/subsequent-37.json`, a MIDI Guide-derived driver:

```json
{
  "_sources": [{
    "type": "pencilresearch",
    "url": "https://github.com/pencilresearch/midi/blob/91299d6/Moog/Subsequent 37.csv",
    "firmware": "unknown",
    "license": "CC-BY-SA-4.0",
    "attribution": "MIDI Guide (https://midi.guide) by Pencil Research and contributors",
    "commit": "91299d6"
  }],
  "device": { "name": "Moog Subsequent 37", "osc_prefix": "/sub37" },
  "cc_params": {
    "channel": 0,
    "entries": [
      { "osc": "/filter/cutoff", "cc": 19, "cc_lsb": 51, "range": [0, 16383],
        "nrpn_msb": 3, "nrpn_lsb": 115, "section": "Filter" },
      { "osc": "/oscillator_2/beat_mode", "nrpn_msb": 3, "nrpn_lsb": 104,
        "range": [0, 1], "_usage": "0: Off; 1: On", "expects": "switch" }
    ]
  }
}
```

The file has 131 entries in all. `cc` alone is a 7-bit CC; `cc` plus `cc_lsb` is a 14-bit pair; `nrpn_msb`/`nrpn_lsb` is an NRPN address. Drivers from Electra presets and vendor docs can also carry SysEx `commands` and `params`. The full format is in the [device JSON schema](../../DEVICE_JSON_SCHEMA.md).

## How do you look up a synth's MIDI implementation?

There are three ways, from quickest to most complete.

- **The device browser.** The [catalogue](../../catalogue.md) is searchable by vendor, model, source and author.
- **`osc-bridge inspect`.** It prints a driver's header and its SysEx side without connecting anything:

    ```
    $ osc-bridge inspect devices/moog/subsequent-37.json
    Device:   Moog Subsequent 37 (v1.1 firmware)
    Vendor:   Moog
    OSC prefix: /sub37
    MID:      04 17
    Dev id:
    Rate limit: Some(1000) Hz
    Commands: 0 entries
    Replies:  0 patterns
    ```

    `inspect` does not list CC and NRPN entries yet, so for a CC-only driver like this one it says little. Use the next option, or open the JSON.

- **The MCP tool `list_routes`.** Run `osc-bridge mcp` in an MCP client (Claude Desktop, Claude Code, Cursor) and ask for a device by slug, its OSC prefix without the slash. `list_routes("sub37")` returns every address with its MIDI addressing, range and `expects`:

    ```json
    {
      "address": "/sub37/filter/cutoff",
      "cc": 19, "cc_lsb": 51,
      "nrpn_msb": 3, "nrpn_lsb": 115,
      "range": [0, 16383],
      "expects": "continuous"
    }
    ```

    It also returns commands, SysEx params, named drum sounds and reply patterns. The [MCP page](../../MCP.md) covers setup and the other seven tools.

## How do you send a parameter by name?

Start the bridge with the driver and send OSC to UDP 7777:

```bash
osc-bridge run --device devices/moog/subsequent-37.json --out-port 3
osc-bridge osc-send /sub37/filter/cutoff 8192
```

Values are the device's own range, not a normalised scale. When an entry has both a CC and an NRPN address, the bridge sends the NRPN: here CC 99 = 3, CC 98 = 115, CC 6 = 64, CC 38 = 0 on channel 1. A second integer argument overrides the channel: `/sub37/filter/cutoff 8192 3` sends on channel 4 (channels are 0-based).

## What is the `expects` field?

`expects` says what kind of control sits on the device side: `continuous` (the default), `switch`, `momentary`, `trigger`, `clock` or `discrete`. It is informational only. The bridge emits the same bytes whatever it says; it is there so a caller, a live-coding runtime or an LLM, can decide whether to sweep a value or toggle it.

On imported drivers it is set by a structural rule (a `[0, 1]` range becomes `switch`). So in the Sub 37 file, `/oscillator_2/beat_mode` is a `switch`, while `/oscillator_2/hard_sync`, documented as "0: Off; 1: On" but declared with a 0 to 127 range, stays `continuous`. Treat it as a hint.

## How do you report or fix a wrong CC?

Open an issue or a pull request on [GitHub](https://github.com/roomi-fields/osc-bridge). [`CONTRIBUTING.md`](https://github.com/roomi-fields/osc-bridge/blob/main/CONTRIBUTING.md) has templates for adding a device, updating one, and promoting one to verified after testing every route on the hardware. Cite a source: a vendor manual, a MIDI implementation chart, or your own captures.

For a MIDI Guide-derived file, also send the correction upstream to [pencilresearch/midi](https://github.com/pencilresearch/midi) (pull request, or email as described on midi.guide). The import script refreshes those files' CC and NRPN tables from upstream on each sync, so a fix made only in osc-bridge would be overwritten. Hand-added fields such as `expects` are kept.

## What are the limits?

- **Most entries are untested.** 5 of 963 devices are verified. A doc-derived map can be wrong, incomplete, or tied to a firmware version the source does not record (`"firmware": "unknown"` is common).
- **MIDI Guide drivers are CC and NRPN only** (plus named drum sounds on 15 drum machines). No SysEx, so no patch dumps or display text until someone adds a SysEx layer.
- **Electra-derived drivers are editorial.** Naming and ranges follow the preset author, and some parameters of the synth may be missing.
- **Names are not standardised across devices.** `/filter/cutoff` on one synth may be `/vcf/frequency` on another; the names follow each vendor's terms.

## FAQ

**Is this the same data as midi.guide?**
For the 376 MIDI Guide-derived drivers, yes, converted to JSON and pinned to an upstream commit. midi.guide also has its own read-only API at api.midi.guide. The other drivers come from Electra presets, vendor docs or reverse engineering.

**Can I use the JSON files without the bridge?**
Yes. They are plain JSON under `devices/<vendor>/`, licensed GPL-3.0 (MIDI Guide-derived data also under the CC BY-SA 4.0 terms of its source).

**Why are there more driver files (981) than devices (963)?**
Some devices have several variants, one per source or firmware.

**Does a driver tell me if it was tested?**
Yes: look for a `hardware-verified` or `software-verified` entry in `_sources`, or the ✅ marker in the catalogue.
