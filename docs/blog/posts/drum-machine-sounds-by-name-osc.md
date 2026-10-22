---
date: 2026-10-22
authors: [romain]
title: Trigger Drum Machine Sounds by Name over OSC
description: >-
  Send /drm1_mkiv/drum/kick over OSC and osc-bridge plays note 36 on MIDI channel 10. Drum maps for 15 devices, argument rules, code examples and limits.
slug: drum-machine-sounds-by-name-osc
---

# Trigger Drum Machine Sounds by Name over OSC

Drum machines play their sounds from MIDI notes. To trigger one by name over OSC, you need a table from sound name to note and channel. Since v0.13.0, osc-bridge ships that table for 15 devices: you send an address such as `/drm1_mkiv/drum/kick`, and it sends a Note On for note 36 on MIDI channel 10 to the Vermona DRM1 MKIV. Velocity, note and channel can be overridden per message. Any OSC client works: SuperCollider, Sonic Pi, Python, TouchDesigner, or another app.

<!-- more -->

## Why can't you just send "kick"?

MIDI has no idea of a sound name. A drum machine listens for a note number on a channel, and each manufacturer picks its own layout. The TR-8S plays its bass drum on note 36 in its default map and on note 35 in its alternate map. The drumlogue plays everything on one basic channel in one mode, and gives each part its own channel in the other. Elektron machines map tracks to channels. So code that says "play the kick" has to know the device's note map. That knowledge usually ends up hard-coded in every script.

osc-bridge keeps it in the device's JSON driver, in a `triggers` block.

## What is the triggers block?

`triggers` is an optional section of a device driver that gives each note-triggered sound an OSC address. It is documented in the [device JSON schema](../../DEVICE_JSON_SCHEMA.md). One entry from the real DRM1 MKIV driver (`devices/vermona/drm1_mkiv.pencilresearch.fw-unknown.json`):

```json
{ "osc": "/drum/kick", "sound": "Kick", "section": "General",
  "channel": 9, "note_range": [36, 36], "note": 36,
  "velocity_usage": "1~127: Accent level" }
```

- `osc` is appended to the device prefix (`/drm1_mkiv`), which gives `/drm1_mkiv/drum/kick`.
- `channel` is 0-based (0-15), so `9` is MIDI channel 10. It is left out when the channel is set by the user on the device.
- `channel_mode` says how the device listens: `fixed` (the default), `basic` (its user-set basic channel), `omni` (all channels) or `auto` (the active track).
- `note` is the sound's natural note. `note_range` is the range of notes that play it.
- `velocity_usage` and `comments` are documentation copied from the source.

## How are the OSC arguments read?

An address takes up to three optional arguments, `<address> [velocity] [note] [channel]`, and produces one Note On, `0x90 | channel, note, velocity`:

- **velocity** defaults to 100 and is clamped to 1-127. Zero is not allowed because a Note On with velocity 0 means Note Off.
- **note** defaults to the entry's natural note. It must stay inside `note_range`. If the entry has no natural note, the note is required.
- **channel** defaults to the entry's `channel`. When the entry has none, the default is the driver's `midi_out.default_channel`, which is `0` (MIDI channel 1) in all 15 drivers. Like in the JSON, it is 0-based.

The arguments are positional. To set the channel you also pass a velocity and a note. Integers and floats are both accepted, and floats are truncated. A note outside the range is not sent, and the bridge logs `note 70 outside 36..=36 for Kick`.

So with the DRM1 MKIV:

```text
/drm1_mkiv/drum/kick            → 99 24 64   (note 36, ch 10, vel 100)
/drm1_mkiv/drum/kick 127        → 99 24 7F
/drm1_mkiv/drum/snare 90        → 99 26 5A   (note 38)
/drm1_mkiv/drum/clap            → 99 27 64   (note 39)
```

## What do real drum maps look like?

They follow the device's manual, as recorded by MIDI Guide. Three examples from the catalogue:

**Roland TR-8S** (`devices/roland/tr_8s.json`): 22 sounds in two sections, both on channel 10.

```text
/tr_8s/drum/default_note_map/bass_drum      note 36
/tr_8s/drum/default_note_map/snare_drum     note 38
/tr_8s/drum/default_note_map/closed_hi_hat  note 42
/tr_8s/drum/alternate_note_map/bass_drum    note 35
/tr_8s/drum/alternate_note_map/snare_drum   note 40
```

Use the section that matches the note map set on your TR-8S.

**KORG drumlogue** (`devices/korg/drumlogue.pencilresearch.fw-unknown.json`): 22 sounds, in two modes.

```text
/drumlogue/drum/single_channel_mode/bass_drum   note 36, basic channel
/drumlogue/drum/single_channel_mode/snare_drum  note 40, basic channel
/drumlogue/drum/multi_channel_mode/bass_drum    channel 1, any note 0-127
/drumlogue/drum/multi_channel_mode/snare_drum   channel 2, any note 0-127
```

In single-channel mode, the channel comes from `default_channel` (MIDI channel 1). If your drumlogue listens on another channel, pass it: `/drumlogue/drum/single_channel_mode/bass_drum 100 36 9`. The multi-channel entries have no natural note, so you pass one: `/drumlogue/drum/multi_channel_mode/snare_drum 100 60`.

**Elektron Digitakt II** (`devices/elektron/digitakt_ii.json`): `/digitakt_ii/drum/track_1` to `track_16` on channels 1 to 16, plus `/digitakt_ii/drum/active_track` on the Auto Channel. None of these entries has a natural note, so the note is always given: `/digitakt_ii/drum/track_1 100 60`.

To list every address of a device, use the `list_routes` tool of the [MCP server](../../MCP.md), which returns the triggers along with the device's other routes, or read the device's page in the [catalogue](../../catalogue.md).

## Which drum machines are covered?

Fifteen drivers carry a drum map, with 476 named sounds in all. Thirteen are drum machines, grooveboxes or samplers. The other two are worth knowing about: the General MIDI percussion map, and a video synth that uses notes as switches.

| Device | Driver file | Sounds |
|---|---|---|
| Alesis SR-16 | `alesis/sr_16.pencilresearch.fw-unknown.json` | 132 |
| Roland TR-1000 | `roland/tr_1000.json` | 68 |
| Novation DrumStation | `novation/drumstation.pencilresearch.fw-unknown.json` | 66 |
| Behringer LM Drum | `behringer/lm_drum.pencilresearch.fw-unknown.json` | 32 |
| Roland TR-8S | `roland/tr_8s.json` | 22 |
| KORG drumlogue | `korg/drumlogue.pencilresearch.fw-unknown.json` | 22 |
| Elektron Digitakt II | `elektron/digitakt_ii.json` | 17 |
| Elektron Analog Rytm MKII | `elektron/analog_rytm_mkii.json` | 13 |
| Elektron Syntakt | `elektron/syntakt.json` | 13 |
| Elektron Model:Samples | `elektron/model_samples.json` | 13 |
| Vermona DRM1 MKIV | `vermona/drm1_mkiv.pencilresearch.fw-unknown.json` | 10 |
| Roland T-8 | `roland/t_8.pencilresearch.fw-unknown.json` | 7 |
| Teenage Engineering EP-133 K.O. II | `teenage-engineering/ep_133_k_o_ii.pencilresearch.fw-unknown.json` | 4 |
| Generic MIDI 1.0 (GM percussion, notes 35-81, ch 10) | `midi-association/generic_midi_1_0.json` | 47 |
| Sleepy Circuits Hypno (video synth) | `sleepy-circuits/hypno.pencilresearch.fw-unknown.json` | 10 |

The EP-133 entries are pad groups (12 notes each, omni). The T-8 includes its bass line (`/t_8/drum/bass`, notes 12-96 on channel 2). The Generic MIDI map works with any General MIDI sound module.

All 15 drum maps are adapted from [MIDI Guide](https://midi.guide) by Pencil Research and contributors ([pencilresearch/midi](https://github.com/pencilresearch/midi)), licensed CC BY-SA 4.0. The adapted data is under the same license.

## How do you send them from SuperCollider, Sonic Pi or Python?

Start the bridge for your device, then send OSC to `127.0.0.1:7777`, the default bind:

```bash
osc-bridge detect      # find the port number
osc-bridge run --device devices/vermona/drm1_mkiv.pencilresearch.fw-unknown.json --out-port 2
```

**SuperCollider:**

```supercollider
~drm = NetAddr("127.0.0.1", 7777);
~drm.sendMsg("/drm1_mkiv/drum/kick", 110);
~drm.sendMsg("/drm1_mkiv/drum/hihat_1_closed", 70);
```

**Sonic Pi:**

```ruby
use_osc "127.0.0.1", 7777
live_loop :beat do
  osc "/drm1_mkiv/drum/kick", 110
  sleep 0.5
  osc "/drm1_mkiv/drum/snare", 90
  sleep 0.5
end
```

**Python** (with [python-osc](https://pypi.org/project/python-osc/)):

```python
from pythonosc.udp_client import SimpleUDPClient
drm = SimpleUDPClient("127.0.0.1", 7777)
drm.send_message("/drm1_mkiv/drum/kick", 110)
drm.send_message("/drm1_mkiv/drum/clap", [100])
```

**From the shell**, for a quick test: `osc-bridge osc-send /drm1_mkiv/drum/kick 110`.

## Limits

- **Only Note On is sent.** Drum voices are one-shots, so no Note Off follows. If a device gates on note length, send `/note/off` through the driver's `midi_out` routes.
- **The channel is a default, not a reading of your device.** For `basic`, `omni` and `auto` entries, osc-bridge sends on `default_channel` (MIDI channel 1) unless you pass a channel. If the device's basic or auto channel is set to something else, pass it as the third argument (0-based). `omni` devices accept any channel.
- **Note maps can change on the device.** Sections such as TR-8S "Alternate note map" or drumlogue "Multi channel mode" follow the device's settings. Pick the one that matches yours. osc-bridge doesn't read the device's settings.
- **The drum maps are not tested on hardware.** They come from MIDI Guide, and the firmware is not pinned. Most files are named `fw-unknown`. Check them against your manual, and report fixes upstream.
- **Timing is whatever your sender does.** The bridge forwards each message when it arrives. It doesn't schedule or quantise.
- **Not every drum machine has a map.** A device without a `triggers` block can still be played with `/note/on {note} {vel} [{ch}]`, if its driver enables `midi_out`.

## FAQ

**How do I find the exact address of a sound?**
Call `list_routes` on the device through the MCP server, open the device's page in the catalogue, or read the `triggers` block in its JSON file.

**Why does my drum machine stay silent?**
Usually the channel is wrong. Check the device's receive channel, then pass it as the third argument, for example `/sr_16/drum/kick 100 36 9` for MIDI channel 10.

**Can I add a drum map for my own machine?**
Yes. Add a `triggers` block to its driver, following the [schema](../../DEVICE_JSON_SCHEMA.md), and open a pull request on [GitHub](https://github.com/roomi-fields/osc-bridge).

**Does it work from a web page?**
Yes, if you start the bridge with `--ws-bind 127.0.0.1:7890`: browsers can send the same OSC packets over WebSocket. No Web MIDI is involved, so it works in Safari too.
