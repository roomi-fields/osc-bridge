---
date: 2026-10-22
authors: [romain]
title: Control Hardware Synths from SuperCollider or Sonic Pi by Name
description: >-
  Send OSC like /sub37/filter/cutoff from SuperCollider or Sonic Pi; osc-bridge turns it into the synth's MIDI CC, NRPN or SysEx, and sends knob turns back.
slug: supercollider-sonic-pi-hardware-synth
---

# Control Hardware Synths from SuperCollider or Sonic Pi by Name

To control a hardware synth from SuperCollider or Sonic Pi by parameter name, run osc-bridge with the synth's driver file and send it OSC messages such as `/sub37/filter/cutoff 8192`. The bridge looks up the address in the driver, builds the right MIDI bytes (a CC, a 14-bit CC pair, an NRPN sequence or a SysEx frame) and sends them to the synth's MIDI port. With `--osc-client`, knob turns and notes played on the device come back to your live-coding session as OSC.

<!-- more -->

## Why not send MIDI directly from the language?

You can: SuperCollider has `MIDIOut` and Sonic Pi has `midi_cc` and `midi_note_on`. For notes and a few CCs, that is often all you need.

The trouble is the rest of the MIDI implementation. You have to look up which CC is the cutoff, whether the synth wants a 14-bit pair or an NRPN, which order the NRPN bytes go in, and how its SysEx frames are built. That knowledge ends up hard-coded in your patch, once per synth. osc-bridge moves it into one JSON file per device, so your code says `/filter/cutoff` and the driver says "NRPN 3/115, 14-bit".

## How do you start the bridge?

1. Find the synth and its driver. `osc-bridge detect` lists the MIDI ports, asks each device who it is (the standard MIDI Identity Request, which changes nothing on the device), matches the catalogue and prints a ready `osc-bridge run` line. Many older synths do not answer the request; matching then falls back to the port name.

2. Start the bridge. For a Moog Subsequent 37 on MIDI out port 3 and in port 2, with SuperCollider listening on its default port 57120:

    ```bash
    osc-bridge run --device devices/moog/subsequent-37.json \
      --out-port 3 --in-port 2 \
      --bind 127.0.0.1:7777 --osc-client 127.0.0.1:57120
    ```

The bridge listens for OSC on UDP 7777 (the default `--bind`). Every address starts with the driver's prefix, here `/sub37`. `--osc-client` can be repeated; each listed client receives a copy of every incoming MIDI event.

To see what a driver accepts, open its JSON or ask the MCP server's `list_routes` tool. For the Subsequent 37, `/sub37/filter/cutoff` is declared with CC 19/51 (14-bit), NRPN 3/115 and a range of 0 to 16383.

## How do you send parameters from SuperCollider?

Plain `NetAddr` works, as in the repository's `examples/minilab3_demo.scd`:

```supercollider
~sub = NetAddr("127.0.0.1", 7777);
~sub.sendMsg("/sub37/filter/cutoff", 8192);      // half-way, 14-bit range
~sub.sendMsg("/sub37/note/on", 48, 100);          // C3, velocity 100
~sub.sendMsg("/sub37/note/off", 48);
```

The repository also ships a small SuperCollider class, `OscBridge`, in `examples/sc-quark`. Install it from the local folder (or copy `OscBridge.sc` into your Extensions folder) and recompile the class library:

```supercollider
Quarks.install("/path/to/osc-bridge/examples/sc-quark");
```

Then:

```supercollider
~sub = OscBridge("sub37");                       // NetAddr defaults to 127.0.0.1:7777
~sub.param("/filter/cutoff", 8192);              // prefix is added for you
~sub.send("/note/on", 48, 100);

// A slow filter sweep
Routine {
    64.do { |i|
        ~sub.param("/filter/cutoff", (i / 63 * 16383).round);
        0.05.wait;
    };
}.play;
```

`param` converts the value to an integer before sending. That matters: the bridge reads CC and NRPN values as raw device values, so a float such as `0.5` is truncated to `0`. The class also has helpers for the MiniLab 3's SysEx commands (`display`, `padColor`, `recallPreset`).

## How do you send parameters from Sonic Pi?

Point Sonic Pi's `osc` command at the bridge with `use_osc`:

```ruby
use_osc "127.0.0.1", 7777

live_loop :bass do
  osc "/sub37/note/on", 36, 100
  sleep 0.25
  osc "/sub37/note/off", 36
  sleep 0.25
end

live_loop :sweep do
  osc "/sub37/filter/cutoff", (line 2000, 14000, steps: 32).tick.to_i
  sleep 0.125
end
```

Keep the values integers (`.to_i`) for the same reason as above. For a MiniLab 3, the SysEx commands work the same way: `osc "/minilab3/display/text", "Hello", "Sonic Pi"`.

## How do you receive knob turns in your session?

Pass your session's port as `--osc-client` and listen for the decoded messages. The driver's `midi_in` block decides their shape; for most drivers a CC arrives as `/<prefix>/cc/<number> <value> <channel>`.

In SuperCollider (bridge started with `--osc-client 127.0.0.1:57120`):

```supercollider
~mini = OscBridge("minilab3");
~mini.on("/cc/18", { |msg| ("knob: " ++ msg[1]).postln });
```

In Sonic Pi, start the bridge with `--osc-client 127.0.0.1:4560` (Sonic Pi's default OSC input) and `sync` on the address. Sonic Pi prefixes incoming OSC with `/osc`, hence the wildcard:

```ruby
live_loop :knob do
  value, channel = sync "/osc*/minilab3/cc/18"
  set :cutoff, value
end
```

To watch the traffic without either language, run `osc-bridge osc-listen --bind 127.0.0.1:8888` and add `--osc-client 127.0.0.1:8888` to the bridge.

## What about TidalCycles?

Tidal can send OSC to targets other than SuperDirt, but that requires defining a custom OSC target in your Tidal boot file, which is beyond this post. Once a target sends to `127.0.0.1:7777` with the bridge's addresses, the rest works as above.

## What are the limits?

- **Most drivers are not tested on hardware.** Of 963 devices, 5 are verified (MiniLab 3, Electra One MK2, Push 3, MatrixBrute partially, Ableton Live via AbletonOSC). The Subsequent 37 driver used here comes from MIDI Guide data and has not been tested on the synth. Each file says where it came from in `_sources`.
- **Values are raw.** There is no normalised 0 to 1 scale for CC parameters: send the device's own range (0 to 127, or 0 to 16383 for 14-bit entries).
- **No scheduling.** OSC bundles are accepted, but their time tags are ignored and the messages are sent at once. Timing comes from your language's clock and UDP delivery.
- **NRPN costs bandwidth.** When an entry declares an NRPN, the bridge sends three or four MIDI messages per value (CC 99, 98, 6 and, for 14-bit, 38). Fast sweeps on many parameters can saturate a DIN MIDI cable.
- **Port numbers can change.** `run` takes port indexes; replugging a USB device can renumber them. Rerun `osc-bridge detect`.

## FAQ

**Is this the same as the SuperCollider driver in the catalogue?**
No, that one goes the other way. `devices/supercollider/` forwards OSC from the bridge to sclang (port 57120), so a controller or another client can trigger your `OSCdef`s through the bridge. This post uses SuperCollider as the client of a hardware driver.

**Can I drive two synths from one session?**
Yes. `osc-bridge orchestrate --config bridge.toml` runs several devices in one process, each under its own prefix. See `examples/bridge.toml`.

**Which synths are covered?**
963 devices from 212 vendors. Search the [device catalogue](../../catalogue.md).

**Do I need the npm package?**
No. The bridge is a single binary; the [getting started guide](../../getting-started.md) covers installation. The features shown here are in v0.13.0.

Source and issues: <https://github.com/roomi-fields/osc-bridge>.
