# Bitwig Studio — driver companion (DrivenByMoss OSC)

This driver pilots Bitwig Studio 5 through the
[DrivenByMoss](https://github.com/git-moss/DrivenByMoss) controller extension,
which exposes Bitwig over OSC. DrivenByMoss is a community project, but it
is officially partnered with Bitwig (it's distributed in the Bitwig
controller library).

## One-time setup

1. **Install DrivenByMoss** in Bitwig: *Dashboard → Settings → Controllers →
   Add Controller → Generic → OSC*. DrivenByMoss ships as a built-in option
   in recent Bitwig versions; if not, install the
   [latest release](https://github.com/git-moss/DrivenByMoss/releases) into
   `~/Documents/Bitwig Studio/Extensions/` (or the OS-equivalent path).
2. **Configure the OSC ports** in the controller's settings panel : set
   *Port to receive on* to **8000** (this is what the bridge sends to) and
   *Host to send to* / *Port to send to* to **127.0.0.1** / **9000** (this is
   what the bridge listens on as `reply_port`).
3. **Restart** the controller (toggle it off/on in the Controller list).

## Ports

| Direction | Port | Notes |
|---|---|---|
| Bridge → Bitwig | UDP 8000 | configured in DrivenByMoss as "Port to receive on" |
| Bitwig → Bridge | UDP 9000 | DrivenByMoss "Port to send to" |

## OSC surface exposed to bridge clients

All addresses prefixed by `/bitwig`. Track and scene indices are **1-based**
to match DrivenByMoss's convention.

| Send to bridge | Effect | Args |
|---|---|---|
| `/bitwig/transport/play` | start playback | — |
| `/bitwig/transport/stop` | stop playback | — |
| `/bitwig/transport/tempo` | set tempo | `bpm` (float, 20..666) |
| `/bitwig/transport/tap` | tap tempo | — |
| `/bitwig/track/{n}/volume` | set track volume | `v` (float, 0..1) — `n` 1..8 |
| `/bitwig/track/{n}/mute` | toggle mute | `on` (bool) |
| `/bitwig/track/{n}/solo` | toggle solo | `on` (bool) |
| `/bitwig/track/{n}/arm` | toggle record arm | `on` (bool) |
| `/bitwig/scene/{n}/launch` | fire scene | — |

## What you receive from the bridge

DrivenByMoss **auto-emits** state changes — no `start_listen` subscription
needed. Any time a value moves in Bitwig (UI or another controller), the
bridge re-emits it.

| From bridge to client | When |
|---|---|
| `/bitwig/transport/tempo` | tempo change in Bitwig |
| `/bitwig/transport/playing` | play state changed (0 stopped, 1 playing) |
| `/bitwig/track/{n}/volume` | volume change on track n |
| `/bitwig/track/{n}/mute` | mute toggle |
| `/bitwig/track/{n}/solo` | solo toggle |
| `/bitwig/track/{n}/arm` | arm toggle |

## Argument conventions

- **Floats are floats.** `/bitwig/transport/tempo 120` will be coerced to
  120.0 by the bridge but explicit float is safer. Same for volume.
- **Bools accept 0 / 1 or T / F.** DrivenByMoss is tolerant on either.
- **Volume is 0..1** in DrivenByMoss; map your DAW fader values accordingly.
- **DrivenByMoss bank size is 8.** Track and scene routing covers the
  currently-banked range. If you address `/track/9/...`, change the bank in
  Bitwig first (or extend the driver — paths `/track/+` / `/track/-` shift
  the bank, not exposed in v1).

## Out of scope (v1)

Browser, devices, parameter pages, automation, send/return, cue markers,
the wider DrivenByMoss surface. Add them in a follow-up PR as needed.

## References

- DrivenByMoss OSC docs: https://github.com/git-moss/DrivenByMoss-Documentation/blob/master/Generic-Tools-Protocols/Open-Sound-Control-(OSC).md
- DrivenByMoss releases: https://github.com/git-moss/DrivenByMoss/releases
