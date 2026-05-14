# Ableton Live — driver companion (Live 12.2.7 + AbletonOSC 0ca6821)

This driver pilots Ableton Live through the
[AbletonOSC](https://github.com/ideoforms/AbletonOSC) Remote Script. AbletonOSC
is a community project — Ableton itself doesn't ship an OSC server. The driver
is tagged `📡 third-party-osc` and was verified end-to-end against **Live
12.2.7** + **AbletonOSC commit 0ca6821** on 2026-05-13 (transport play/stop,
set tempo, tempo subscription — all green). For other Live versions, expect a
sibling file (`live.third-party-osc.fw-12.1.json` etc.).

## One-time setup

1. Install AbletonOSC: clone the repo into Live's `Remote Scripts` folder.
   - macOS: `~/Music/Ableton/User Library/Remote Scripts/AbletonOSC`
   - Windows: `Documents\Ableton\User Library\Remote Scripts\AbletonOSC`
2. **Restart Live entirely.** Remote Scripts are scanned at startup; an already
   running Live won't see the new folder. You'll only see `AbletonOSC` in the
   Control Surface dropdown after a fresh launch.
3. In Live → **Preferences** → **Link/Tempo/MIDI** → Control Surface →
   pick **AbletonOSC** in one of the slots. Input/Output don't need to be set.
4. Confirm the script loaded: bottom of Live's window should show
   `AbletonOSC: Listening for OSC on port 11000`.

## Ports (factory defaults)

| Direction | Port | Notes |
|---|---|---|
| Bridge → Live | UDP 11000 | declared in `transport.port` |
| Live → Bridge | UDP 11001 | declared in `transport.reply_port` |

If you change AbletonOSC's ports (it has no UI for this — edit the script's
constants), update the JSON accordingly or override via `orchestrate.toml`.

## OSC surface exposed to the bridge's clients

All addresses prefixed by `/ableton`.

| Send to bridge | Effect | Args |
|---|---|---|
| `/ableton/transport/play` | start playback | — |
| `/ableton/transport/stop` | stop playback | — |
| `/ableton/transport/tempo` | set tempo (BPM) | `bpm` (float, 20..999) |
| `/ableton/track/{n}/volume` | set track volume | `v` (float, 0..1) |
| `/ableton/track/{n}/arm` | arm / disarm track | `on` (bool) |
| `/ableton/track/{n}/listen/volume` | subscribe to volume changes for one track | — |
| `/ableton/scene/{n}/fire` | fire scene by index | — |
| `/ableton/scene/fire_selected` | fire the currently selected scene | — |

Track and scene indices are **0-based** in AbletonOSC (track 0 = the leftmost
track in Session view). Live's UI shows 1-based indices in the inspector;
remember to subtract one.

## What you receive from the bridge

Subscriptions wired automatically at startup, plus replies on demand.

| From bridge to client | Triggered by | Args |
|---|---|---|
| `/ableton/transport/tempo` | tempo change in Live (subscribed at startup) | `bpm` (float) |
| `/ableton/track/{n}/volume` | volume change on a track you subscribed to | `v` (float) |
| `/ableton/track/{n}/arm` | arm toggle on any track | `on` (bool) |

## Argument conventions

- **Floats are floats.** `/ableton/transport/tempo 120` will silently miss its
  target because AbletonOSC expects an OSC `f` (float32) tag, and integer-only
  values get rejected on the Live side. Send `120.0`.
- **Bools are 0 / 1.** AbletonOSC accepts `0` and `1`; some clients send the
  OSC `T` / `F` tags — the bridge accepts both.
- **Volume is 0..1 normalized.** AbletonOSC's internal scale matches Live's
  fader (0 = -inf dB, ~0.85 = 0 dB, 1.0 = +6 dB). You may want to remap on
  the client side.

## Troubleshooting

- **Nothing happens.** Confirm Live shows `AbletonOSC: Listening for OSC on
  port 11000` and that no firewall blocks UDP 11000/11001 on localhost.
- **Bridge log says `OSC SUB → ...` but Live ignores updates.** AbletonOSC
  rebuilds its observer wiring on each preset/session change — if you load a
  set after the bridge started, the bridge's `start_listen` registrations may
  have been dropped. Restart the bridge.
- **Tempo updates from Live not received.** Check that the bridge was
  launched with `--bind`/`--osc-client` *and* that `transport.reply_port`
  (11001) is free on the bridge host. The bridge logs a `WARN: device
  declares OSC replies but no transport.reply_port — they will never fire`
  if the reply port isn't bound — that's the giveaway.

## Out of scope (v1)

Clip slots, device parameters and chains, MIDI clip notes, browser
navigation, undo/redo, named-track lookup. AbletonOSC supports much more; if
you need any of those, open a PR adding the routes — keep the same
`/ableton/...` prefix convention.

## References

- AbletonOSC repo: <https://github.com/ideoforms/AbletonOSC>
- AbletonOSC reference: <https://github.com/ideoforms/AbletonOSC/blob/master/docs/osc-api.md>
