---
date: 2026-10-22
authors: [romain]
title: Drive Ableton Live from Claude with AbletonOSC and MCP
description: >-
  Install AbletonOSC, run osc-bridge with its Ableton driver, add its MCP server: Claude can then set tempo, play, arm tracks, set volumes and fire scenes.
slug: ableton-live-from-claude-osc
---

# Drive Ableton Live from Claude with AbletonOSC and MCP

To drive Ableton Live from Claude, install the AbletonOSC Remote Script in Live, run osc-bridge with its Ableton driver, and add osc-bridge's MCP server to Claude. Claude can then start and stop the transport, set the tempo, arm tracks, set track volumes and fire scenes, by sending OSC such as `/ableton/transport/tempo 124.0`. The same bridge can also map a hardware controller's knob to a Live track, and drive hardware synths in the same session. The Ableton surface is small (eight commands), and the post ends with a fair comparison with ableton-mcp, which covers much more of Live.

<!-- more -->

## What is AbletonOSC?

[AbletonOSC](https://github.com/ideoforms/AbletonOSC) is a community Remote Script, by ideoforms, that gives Live an OSC interface. Ableton does not ship an OSC server itself. AbletonOSC listens on UDP 11000 and replies on UDP 11001, with addresses such as `/live/song/set/tempo`.

osc-bridge's driver, `devices/ableton/live.third-party-osc.fw-12.2.json`, maps a short set of readable addresses under `/ableton` onto those calls. It is marked software-verified: transport play/stop, tempo setting and the tempo subscription were tested end to end against Live 12.2.7 with AbletonOSC commit `0ca6821`, on Windows, on 2026-05-13.

## How do you set it up?

1. **Install AbletonOSC.** Clone the repository into Live's Remote Scripts folder:
    - macOS: `~/Music/Ableton/User Library/Remote Scripts/AbletonOSC`
    - Windows: `Documents\Ableton\User Library\Remote Scripts\AbletonOSC`
2. **Restart Live completely.** Remote Scripts are scanned at startup.
3. In **Preferences → Link/Tempo/MIDI**, pick **AbletonOSC** as a Control Surface. Input and Output can stay empty. Live's status bar should show `AbletonOSC: Listening for OSC on port 11000`.
4. **Start the bridge** with the Ableton driver. No MIDI port is needed for a software target:

    ```bash
    osc-bridge run --device devices/ableton/live.third-party-osc.fw-12.2.json \
      --osc-client 127.0.0.1:8888
    ```

    ```
    OSC REPLY 0.0.0.0:11001
    OSC SUB  /live/song/start_listen/tempo -> 127.0.0.1:11000
    Device:  Ableton Live (Ableton Live 12.2.7 + AbletonOSC 0ca6821)
    OSC TGT  127.0.0.1:11000
    OSC IN   127.0.0.1:7777
    OSC CLI  127.0.0.1:8888
    Ready. Ctrl-C to stop.
    ```

    At startup the bridge subscribes to tempo changes, so tempo moves in Live come back as `/ableton/transport/tempo <bpm>` to every `--osc-client`.

5. **Add the MCP server** to Claude Desktop (`claude_desktop_config.json`), Claude Code or Cursor:

    ```json
    {
      "mcpServers": {
        "osc-bridge": {
          "command": "npx",
          "args": ["-y", "@roomi-fields/osc-bridge", "mcp"]
        }
      }
    }
    ```

The MCP server does not start the bridge; it sends to a running one, on `127.0.0.1:7777` by default (`--default-target` changes that). The [MCP page](../../MCP.md) lists the eight tools.

## What can Claude do in Live?

These are the eight commands of the driver, all under `/ableton`:

| Address | Effect | Arguments |
|---|---|---|
| `/ableton/transport/play` | start playback | none |
| `/ableton/transport/stop` | stop playback | none |
| `/ableton/transport/tempo` | set tempo | `bpm` float, 20 to 999 |
| `/ableton/track/{n}/volume` | set a track's volume | `v` float, 0 to 1 |
| `/ableton/track/{n}/arm` | arm or disarm a track | `on` bool |
| `/ableton/track/{n}/listen/volume` | report a track's volume changes | none |
| `/ableton/scene/{n}/fire` | fire a scene | none |
| `/ableton/scene/fire_selected` | fire the selected scene | none |

Track and scene numbers are 0-based: track 0 is the leftmost track in Session view. Volume follows Live's fader scale (0 is minus infinity, about 0.85 is 0 dB, 1.0 is +6 dB).

A request such as *"Set the tempo to 124 and arm the second track"* becomes two calls of the `send` tool:

```
send(addr="/ableton/transport/tempo", args=[124.0])
send(addr="/ableton/track/1/arm", args=[true])
```

Claude finds these addresses itself: `list_routes("ableton")` returns the surface, and `get_device_docs("ableton")` returns the driver's companion guide (setup, ports, argument conventions). AbletonOSC expects floats for tempo and volume; the driver declares those types and the bridge converts integers, but `124.0` is the safe habit for other OSC clients.

## How do you map a MiniLab knob to a track volume?

Use the orchestrator, which runs several devices in one process and routes messages between them. This `bridge.toml` loads a MiniLab 3 and Live, and sends knob CC 74 to track 0's volume:

```toml
[osc]
bind = "127.0.0.1:7777"
clients = ["127.0.0.1:8888"]

[[devices]]
spec = "devices/arturia/minilab3.json"
midi_out_port = 4
midi_in_port = 0

[[devices]]
spec = "devices/ableton/live.third-party-osc.fw-12.2.json"

[[routes]]
from = "/minilab3/cc/74"
to = "/ableton/track/0/volume"
map.from = [0, 127]
map.to = [0, 0.85]
```

```bash
osc-bridge orchestrate --config bridge.toml
```

The MiniLab's MIDI input is decoded as `/minilab3/cc/74 <value> <channel>`. The route rescales the first argument linearly from 0-127 to 0-0.85, so the knob tops out at 0 dB instead of +6 dB, and forwards it to Live. Use the CC number your knob actually sends: `osc-bridge osc-listen --bind 127.0.0.1:8888` shows every incoming message. Routes accept `{captures}` in the path (`from = "/minilab3/cc/{n}"`) and are covered by `tests/orchestrator_routing_e2e.rs`. The MiniLab 3 driver is hardware-verified.

Claude can keep using `send` against the orchestrator on the same port 7777 while the route runs.

## How does it compare with ableton-mcp?

[ahujasid/ableton-mcp](https://github.com/ahujasid/ableton-mcp) is an MCP server dedicated to Ableton Live. It installs its own Remote Script, which runs a socket server inside Live and exchanges JSON commands over TCP with the MCP server. It is installed with `uvx ableton-mcp`, needs Live 10 or later and Python 3.8 or later, and is MIT-licensed. Its README notes anonymous usage telemetry, on by default and possible to disable.

It covers much more of Live than osc-bridge's driver: reading session and track information, creating MIDI and audio tracks, creating and editing clips, adding notes to MIDI clips, loading instruments and effects from Live's browser, and arrangement-view composition.

| | ableton-mcp | osc-bridge + AbletonOSC |
|---|---|---|
| Scope | Ableton Live only | Live, 7 other software targets, 963 hardware devices |
| Live features | tracks, clips, notes, devices, browser | transport, tempo, track volume and arm, scenes |
| Link to Live | own Remote Script, JSON over TCP | AbletonOSC, OSC over UDP |
| Routing between devices | not covered | `[[routes]]` between devices |

If you want Claude to compose inside Live, ableton-mcp is the better fit today. osc-bridge makes sense when Live is one part of a rig with hardware synths and controllers, and you want one MCP server and one OSC surface for all of it.

## What are the limits?

- **Eight commands.** Clip slots, device parameters, MIDI notes, browser navigation, undo and named-track lookup are out of scope for now, though AbletonOSC supports many of them. A pull request adding routes under `/ableton/...` is welcome.
- **Verified on one setup.** Live 12.2.7 with AbletonOSC `0ca6821` on Windows. Other 12.x versions are unconfirmed.
- **Third-party dependency.** AbletonOSC's compatibility with Live versions follows its own releases, not Ableton's.
- **Subscriptions can drop.** If you load another set after the bridge started, AbletonOSC may drop the bridge's listeners. Restart the bridge.
- **Volume feedback is opt-in.** Volume changes are reported only for tracks you subscribed to with `/listen/volume`; arm changes come back as `/ableton/track/{n}/arm`.

## FAQ

**Do I need Max for Live?**
No. AbletonOSC is a Python Remote Script; it does not use Max for Live.

**Can Claude control a hardware synth in the same conversation?**
Yes. Run the orchestrator with the synth's driver as an extra `[[devices]]` entry; each device has its own prefix. Browse the [catalogue](../../catalogue.md) for drivers.

**Does it work with Live running on another computer?**
Yes. In `bridge.toml`, the Ableton device entry accepts `host`, `port` and `reply_port` overrides. AbletonOSC must accept packets from that machine and no firewall may block UDP 11000/11001.

**Where do I start?**
The [getting started guide](../../getting-started.md). Source and issues: <https://github.com/roomi-fields/osc-bridge>.
