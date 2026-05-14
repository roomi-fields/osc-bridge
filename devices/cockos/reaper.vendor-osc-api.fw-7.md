# Reaper — driver companion (native OSC, Default.ReaperOSC mapping)

Reaper ships with native OSC support via the **Default.ReaperOSC** pattern
file. This driver matches that default mapping. If you've customized the
pattern file in Reaper's resource directory, adapt the driver paths accordingly.

## One-time setup

1. In Reaper: **Preferences → Control/OSC/web → Add → OSC (Open Sound Control)**.
2. **Mode**: *Configure device IP+local port*.
3. **Device IP**: `127.0.0.1` · **Device port**: `9000` (this is what the bridge
   listens on as `reply_port`).
4. **Local listen port**: `8000` (this is what the bridge sends to).
5. **Pattern config**: leave on `Default.ReaperOSC` (or whatever default ships
   with your Reaper version). No restart needed.

## Ports

| Direction | Port | Notes |
|---|---|---|
| Bridge → Reaper | UDP 8000 | "Local listen port" in Reaper |
| Reaper → Bridge | UDP 9000 | "Device port" in Reaper |

## OSC surface exposed to bridge clients

All addresses prefixed by `/reaper`. Track indices are **1-based** to match
Reaper's `@` placeholder convention.

| Send to bridge | Effect | Args |
|---|---|---|
| `/reaper/transport/play` | start playback | — |
| `/reaper/transport/stop` | stop playback | — |
| `/reaper/transport/record` | toggle record | — |
| `/reaper/transport/tempo` | set tempo | `bpm` (float, 20..999) |
| `/reaper/track/{n}/volume` | set track volume | `v` (float, 0..1 normalized) |
| `/reaper/track/{n}/mute` | toggle mute | `on` (bool) |
| `/reaper/track/{n}/solo` | toggle solo | `on` (bool) |
| `/reaper/track/{n}/arm` | toggle record arm | `on` (bool) |

## What you receive from the bridge

Reaper emits feedback on the same paths used to control it (round-trip mapping).

| From bridge to client | When |
|---|---|
| `/reaper/transport/tempo` | tempo changed in Reaper |
| `/reaper/track/{n}/volume` | volume change on track n |
| `/reaper/track/{n}/mute` | mute toggle |
| `/reaper/track/{n}/solo` | solo toggle |
| `/reaper/track/{n}/arm` | arm toggle |

## Argument conventions

- **Volume is 0..1 normalized** in Reaper's default pattern (`n/track/@/volume`).
  Multiply by your DAW's scale if needed on the client side.
- **Bools accept 0 / 1** (Reaper's `b/...` patterns are strict on this).
- **Track count default in Reaper is 8.** To extend, change `DEVICE_TRACK_COUNT`
  in your `.ReaperOSC` config and bump the `range` in the driver JSON.
- **Customised pattern files** will diverge from this driver — copy the JSON
  as a sibling variant (`reaper.vendor-osc-api.fw-7.custom-XXX.json`) if you
  need to ship a non-default mapping.

## Out of scope (v1)

FX parameters, sends/receives, automation, time selection, marker / region
navigation, master track, the wider Reaper OSC surface. Default.ReaperOSC is
~1000 lines; this driver picks the essentials. Extend in follow-up PRs.

## References

- Reaper OSC SDK: https://www.reaper.fm/sdk/osc/osc.php
- Default.ReaperOSC overview: https://konbear.com/articles/deep-dive-into-reaperosc-config-file
