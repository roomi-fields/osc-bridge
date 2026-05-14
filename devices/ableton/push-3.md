# Ableton Push 3 — osc-bridge driver

Driver for the **Push 3 in Control Mode via USB-C** (class-compliant MIDI interface). Scope: full hardware surface (pads, encoders, buttons, ribbon), all LEDs (palette index via note-on, button LEDs via CC echo), and hardware configuration SysEx (palette, ribbon mode, brightness, MPE, preamps, pedals, audio routing).

**Out of scope**:
- LCD display (separate USB bulk interface, protocol not publicly RE'd for Push 3, handled by Ableton's display driver)
- Audio interface (separate USB audio class interface, handled by Thesycon driver)
- Standalone mode (the Push 3's internal Live owns the surface — USB-C only carries a heartbeat SysEx stream, pad/button presses are not forwarded)

The MIDI map below is extracted from **DrivenByMoss** (`git-moss/DrivenByMoss`, LGPL-3.0) which supports Push 3 natively. Hardware-tweaking SysEx are extracted from the same source (PushControlSurface.java). Every entry cites its source file+line.

## 1. Quick start

1. On the Push 3, press **Setup** (gear icon, upper-left corner) → navigate to the **Status** tab using the display buttons above the screen → press the highlighted button to switch to **Control Mode**. USB-C now exposes class-compliant MIDI.
2. Firmware ≥ 12.0 recommended (Live 12-era).
3. On Windows, ensure the **AbletonAudioks** service is enabled (`Set-Service AbletonAudioks -StartupType Manual` from admin PowerShell if it's been disabled).
4. Launch `osc-bridge run --device push-3.json --in-port <N> --out-port <N>` with the MIDI in/out ports matching `Ableton Push 3 MIDI` (the first cable on the USB-MIDI interface).
5. Input events flow on `/push3/note/on`, `/push3/cc/<num>`, `/push3/pitchbend`, `/push3/aftertouch`. Outbound LED feedback is plain raw-MIDI via the OSC paths below. Hardware config happens via `/push3/palette/...`, `/push3/ribbon/mode`, etc.
6. Device inquiry: send `F0 7E 7F 06 01 F7` via `/push3/raw/syx` → the driver parses the reply and emits `/push3/identity major minor build_lsb build_msb s0 s1 s2 s3 s4 board_rev`.

## 2. Surface map — pads (8×8 grid)

- **MIDI note range** `36..99` (`0x24..0x63`) on **channel 0**. Bottom-left pad = note 36, top-right pad = note 99.
- Formula: `note = 36 + row*8 + col` (row 0 = bottom, col 0 = left). (Source: `PadGridImpl.java:39, 97`.)
- Press → `/push3/note/on <note> <velocity> 0`. Release → `/push3/note/off <note> 0 0`.
- Aftertouch: in poly mode → `/push3/poly_aftertouch <note> <value> 0`. In MPE mode → pitch-bend + CC 74 per-channel.

### Pad row/col → note table (bottom-left origin)

```
row 7 (top)    |  92 93 94 95 96 97 98 99
row 6          |  84 85 86 87 88 89 90 91
row 5          |  76 77 78 79 80 81 82 83
row 4          |  68 69 70 71 72 73 74 75
row 3          |  60 61 62 63 64 65 66 67
row 2          |  52 53 54 55 56 57 58 59
row 1          |  44 45 46 47 48 49 50 51
row 0 (bottom) |  36 37 38 39 40 41 42 43
```

### Pad LED feedback

- **Solid colour**: `/push3/note/on <padNote> <paletteIndex> 0` — `paletteIndex` 0..127, resolved via device palette. Velocity 0 = off.
- **Slow blink**: `/push3/note/on <padNote> <paletteIndex> 10` (channel 10).
- **Fast blink**: `/push3/note/on <padNote> <paletteIndex> 14` (channel 14).

### Default palette indices observed (from danielknng/push3-protocol-docs)

`0 = OFF · 5 = RED · 9 = ORANGE · 13 = YELLOW · 17 = GREEN · 33 = CYAN · 37 = BLUE · 41 = PURPLE · 45 = PINK · 117 = DARK GRAY · 118 = LIGHT GRAY · 119 = WHITE`

These are the factory defaults; override any slot via `/push3/palette/entry/set` + `/push3/palette/commit`.

## 3. Surface map — encoders

All encoders emit **relative values, two's complement**: bytes 1..63 = +1..+63, 65..127 = -63..-1, 64 = zero crossing.

| Encoder | Turn (CC ch 0) | Touch (note-on ch 0) | Press |
|---|---|---|---|
| Knobs 1..8 | CC 71..78 | note 0..7 | — |
| Master volume | CC 79 | note 8 | — |
| Tempo (left of master) | CC 14 | note 10 | CC 15 |
| Central jog (Push 3) | CC 70 | — | CC 94 (+ CC 93 left step, CC 95 right step, CC 91 centre) |

Sources: `PushControlSurface.java:62-66, 163-181, 203-211, 252-270`; `PushControllerSetup.java:732-754`.

## 4. Surface map — buttons

All buttons are **momentary** Control Changes on **channel 0**. Press → CC 127, release → CC 0. LED feedback: write the same CC (0=off, 127=on, intermediate = brightness/colour).

Source: `PushControlSurface.java:50-249`, bindings at `PushControllerSetup.java:547-722`. *(P3)* entries differ from Push 2.

### Full button table

| CC | Name | CC | Name |
|---|---|---|---|
| 3 | Tap Tempo | 57 | Accent |
| 9 | Metronome | 58 | Scale |
| 14 | Tempo knob (turn) | 59 | User / Setup |
| 15 | Tempo knob press | 60 | Mute |
| 20..27 | Row 1 display buttons (above screen) | 61 | Solo |
| 28 | Master | 62 | Device-Left / Page-Left |
| 29 | Stop Clip | 63 | Device-Right / Page-Right |
| 30 | Setup *(P3)* | 64 | Footswitch 1 |
| 31 | Layout *(P3)* | 65 | Capture MIDI / Create Scene *(P3)* |
| 32 | Add Track *(P3)* | 69 | Footswitch 2 |
| 33 | Hot Swap / Browse *(P3)* | 70 | Central encoder turn *(P3)* |
| 34 | Session Display toggle *(P3)* | 71..78 | Knobs 1..8 turn |
| 35 | Convert *(P3)* | 79 | Master knob turn |
| 36..43 | Scene 1..8 | 80 | Files / Load *(P3)* |
| 44 | Left | 81 | Help *(P3)* |
| 45 | Right | 82 | Save *(P3)* |
| 46 | Up | 83 | Lock *(P3)* |
| 47 | Down | 85 | Play |
| 48 | Select | 86 | Record |
| 49 | Shift | 87 | New *(P1/P2)* |
| 50 | Note | 88 | Duplicate |
| 51 | Session | 89 | Automate |
| 52 | Add Effect | 90 | Fixed Length |
| 53 | Add Track *(P1/P2)* | 91 | Cursor Centre *(P3)* |
| 54 | Octave Down | 92 | New *(P3)* |
| 55 | Octave Up | 93 | Central encoder left *(P3)* |
| 56 | Repeat | 94 | Central encoder press *(P3)* |
| 102..109 | Row 2 display buttons (below screen) | 95 | Central encoder right *(P3)* |
| 110 | Device | 115 | Pan/Send *(P1)* |
| 111 | Browse / Toggle Master-Cue *(P3)* | 116 | Quantize |
| 112 | Track / Mix | 117 | Double Loop |
| 113 | Clip | 118 | Delete |
| 114 | Volume *(P1)* | 119 | Undo |

Scene buttons CCs (36..43) and pad notes (36..43) share the same numeric values but are different MIDI message types (CC vs note-on) — no routing conflict.

## 5. Surface map — touch strip (ribbon)

- **Touch** → `/push3/note/on 12 127 0` (down), `/push3/note/off 12 0 0` (up).
- **Position** → `/push3/pitchbend <0..16383> 0` (14-bit pitch-bend).
- **Mode switch** via SysEx `/push3/ribbon/mode <status>`:
  - `1` = volume-fader
  - `9` = discrete / stepped
  - `17` = pan (bipolar)
  - `122` = pitch-bend (default, 14-bit)
- Drive the strip LEDs manually by sending `/push3/pitchbend <value> 0` outbound.

## 6. SysEx commands — hardware configuration

All SysEx frames below use the header `F0 00 21 1D 01 01` and footer `F7` (added automatically by the driver). Every opcode and its layout is cited against DrivenByMoss for audit.

### Palette (pad LED colours)

| OSC path | Opcode | Description |
|---|---|---|
| `/push3/palette/entry/set` | `0x03` | Rewrite palette slot 0..127 with RGB + white. Each channel 14-bit split LSB+MSB. |
| `/push3/palette/entry/get` | `0x04` | Query one slot. |
| `/push3/palette/commit` | `0x05` | Apply pending palette writes. |

Example — set palette index 0 to pure white then commit:
```
/push3/palette/entry/set 0  127 127  127 127  127 127  127 127
/push3/palette/commit
```

### Brightness

| OSC path | Opcode | Range |
|---|---|---|
| `/push3/led/brightness` | `0x06` | 0..127 |
| `/push3/display/brightness` | `0x08` | value 0..255 split into `lsb` (low 7 bits) + `top_bit` (bit 7 isolated) |

### Pads — pressure mode and MPE (Push 3 specific)

| OSC path | Opcode | Range |
|---|---|---|
| `/push3/pressure/mode` | `0x1E` | 0=channel pressure, 1=poly aftertouch, 2=MPE |
| `/push3/pad/per_pad_pitchbend` | `0x26 07 08` | 2=enable, 0=disable |
| `/push3/pad/in_tune_location` | `0x26 07 0E` | 0=Pad centre, 1=Finger position |
| `/push3/pad/in_tune_width` | `0x26 07 14` | MPE pitch-bend deadzone |
| `/push3/pad/slide_height` | `0x26 07 24` | MPE Y-axis deadzone |

### Ribbon

| OSC path | Opcode | Range |
|---|---|---|
| `/push3/ribbon/mode` | `0x17` | 1=volume, 9=discrete, 17=pan, 122=pitch-bend |

### User Mode (pad MIDI forwarding)

| OSC path | Opcode | Effect |
|---|---|---|
| `/push3/user_mode 1` | `0x0A 0x01` | Enter User Mode — pads forward their MIDI events to the host, **LED feedback becomes your responsibility** |
| `/push3/user_mode 0` | `0x0A 0x00` | Exit User Mode — Push 3 handles pad LEDs automatically in response to presses (the default behaviour in Live-integrated Control Mode) |

Source: `0x0A 0x01` documented in `danielknng/push3-protocol-docs`, `0x0A 0x00` confirmed via hardware sniff. Neither is exposed by DrivenByMoss — this is a Push 3-specific SysEx.

**When to use**: if you want to use the Push 3 purely as a controller for another DAW/soft (pads → OSC → any consumer), send `/push3/user_mode 1` first, then implement your own LED feedback strategy (echo note-on back to the pad to light it, decay with a timer for a "hit" effect, etc.). Otherwise stay in non-User mode to keep the stock responsive pad lighting.

### Audio hardware

Push 3 has a built-in 2-in / 2-out audio interface with independent digital preamps on each input, and three output routings.

| OSC path | Opcode | Range |
|---|---|---|
| `/push3/preamp/1/type` | `0x37 1A` | 0=Line, 1=Instrument, 2=High |
| `/push3/preamp/2/type` | `0x37 1B` | idem |
| `/push3/preamp/1/gain` | `0x37 02` | 0x00 (+20 dB) … 0x28 (no gain), step 2 per dB |
| `/push3/preamp/2/gain` | `0x37 03` | idem |
| `/push3/audio/output_config` | `0x37 11` | 0=Headphones 1/2 + Speaker 1/2, 2=Headphones 3/4 + Speaker 1/2, 3=Headphones 1/2 + Speaker 3/4 |

### Pedals / CV

| OSC path | Opcode | Value |
|---|---|---|
| `/push3/pedals/config` | `0x37 26` | `0x50`=both footswitches (default), `0x43`=pedal1 CV + pedal2 footswitch, `0x1C`=pedal1 footswitch + pedal2 CV, `0x0F`=both CV |

### Device inquiry

Universal, not device-specific:
```
/push3/raw/syx F0 7E 7F 06 01 F7
```
Reply parsed as:
```
/push3/identity <major> <minor> <build_lsb> <build_msb> <s0> <s1> <s2> <s3> <s4> <board_rev>
```

## 7. Pad velocity sensitivity curve (advanced — opcode 0x43)

The Push 3 accepts a full **128-byte custom velocity curve** via SysEx `0x43 <128 bytes>`. DrivenByMoss computes this curve from four high-level params (threshold, drive, compand, range) using Schlick bias/gain functions — too specific to expose as a simple "preset 0..5" like earlier Pushes.

If you need a custom curve, craft it yourself and send via:
```
/push3/raw/syx F0 00 21 1D 01 01 43 <128 hex bytes> F7
```

The reference algorithm is in `PushControlSurface.java:795-845`. This driver doesn't expose a high-level wrapper for now — file an issue if you want one.

## 8. Observed heartbeats (Standalone mode SysEx)

When Push 3 runs in **Standalone mode**, it broadcasts status SysEx on the main MIDI port at ~3 Hz. These opcodes are **not documented** in DrivenByMoss — they are likely internal firmware state. Observed 2026-04-20 on firmware 12.x:

| SysEx payload | Frequency | Purpose (speculative) |
|---|---|---|
| `38 18 00 00 19 00 00 00 00 00 00 00 00 00 00 00` | ~3 Hz | battery / thermal / activity |
| `38 0D 00 00 3A 00 00 3B 00 00 3C 00 00 00 00 00` | ~3 Hz | status with internal counter (0x3A / 0x3B / 0x3C) |
| `3A 21 XX` | ~1.6 Hz | short notification, `XX` alternates `0x59` / `0x5A` |

This driver doesn't parse them — they fall through to `/push3/sysex/raw <hex>` so clients can filter/log as they see fit. If you RE them further, file a PR.

## 9. Known constraints

- **Control Mode required** for bidirectional use. In Standalone mode, only osc-bridge→Push direction works (LED writes, SysEx config). Pad/button presses are consumed by the internal Live and never forwarded.
- **Display (LCD 960×160)**: not exposed. Push 3 display uses a separate USB bulk interface with a proprietary framebuffer protocol. No public RE at 2026-04 confirms the Push 3 framebuffer format (Push 2 is documented but Push 3 differs).
- **Audio interface**: not exposed here. Use the Thesycon driver (Ableton UB Audio Control Panel) for DAW audio I/O. Independent from this MIDI driver.
- **Windows driver note**: if the Thesycon/Ableton driver service `AbletonAudioks` is disabled, the Push 3 USB enumeration will fail silently even though the device is powered. Re-enable with `Set-Service AbletonAudioks -StartupType Manual` (admin PowerShell), then unplug/replug.
- **MPE channel-rotation**: when `/push3/pressure/mode 2` (MPE), pads emit pitch-bend + CC 74 per-channel. The bridge forwards them as-is; no semantic voice allocator here.

## 10. For LLM integrators (Kanopi, ShowControl, ...)

- Fetch this file via `/bridge/docs`.
- Pad grid formula: `note = 36 + row*8 + col` (row 0 = bottom, col 0 = left). Always compute; don't hardcode.
- Batching: `/push3/note/on` + `/push3/cc/` messages are serialised by the bridge at the device's native USB-MIDI rate (~1 kHz). 64 pad updates fit in <1 ms.
- `/push3/identity` gives a clean startup handshake after `F0 7E 7F 06 01 F7`.
- Palette semantics: 0 = off, factory defaults in §2, override with `/push3/palette/entry/set` + `/push3/palette/commit` for deterministic colours.
