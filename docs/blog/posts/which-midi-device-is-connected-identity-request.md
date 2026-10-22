---
date: 2026-10-22
authors: [romain]
title: "Which MIDI Device Is Plugged In? The MIDI Identity Request Explained"
description: >-
  Send F0 7E 7F 06 01 F7 and most MIDI devices reply with their manufacturer, model and firmware. Byte layout, what osc-bridge detect prints, and limits.
slug: which-midi-device-is-connected-identity-request
---

# Which MIDI Device Is Plugged In? The MIDI Identity Request Explained

To find out which MIDI device is on a port, send it the Universal Identity Request, `F0 7E 7F 06 01 F7`. Most devices built since the mid-1990s answer with an Identity Reply that holds their manufacturer ID, a model family and number, and a firmware version. The request changes nothing on the device. It is the standard "who are you?" that editors send when they connect. Since v0.13.0, `osc-bridge detect` sends this request on every port, decodes the replies, and matches them to its catalogue of device drivers.

<!-- more -->

## What does F0 7E 7F 06 01 F7 mean?

It is a Universal System Exclusive message, defined by the MIDI 1.0 specification and not owned by any manufacturer. Byte by byte:

| Byte | Meaning |
|---|---|
| `F0` | Start of SysEx |
| `7E` | Universal Non-Real-Time message |
| `7F` | Device ID "all call": every device should answer, whatever its own ID |
| `06` | Sub-ID 1: General Information |
| `01` | Sub-ID 2: Identity Request |
| `F7` | End of SysEx |

Because it is universal, the same six bytes work on a Roland, a Korg or an Arturia. You don't need to know what the device is in order to ask.

## What does the Identity Reply contain?

The reply has the same frame, with sub-ID 2 set to `02` (Identity Reply), followed by four fields:

```
F0 7E <dev> 06 02 <manufacturer: 1 or 3 bytes> <family: 2> <model: 2> <version: 4> F7
```

- **`<dev>`** is the device's own ID. It is often its MIDI channel or `7F`.
- **Manufacturer ID**, 1 or 3 bytes (see the next section).
- **Family** and **model**: two 14-bit numbers, each sent as two 7-bit bytes, low byte first. The family is the product line and the model is the variant within it. Each manufacturer assigns its own numbers.
- **Version**: four bytes of firmware revision. Their meaning is up to the manufacturer. osc-bridge shows them as dotted numbers, for example `1.0.4.0`.

osc-bridge's parser (`parse_identity_reply` in `src/detect.rs`) checks `F0 7E … 06 02`, reads the manufacturer ID, then decodes family and model as `lo | (hi << 7)`. It ignores anything that is too short. For example, from its tests:

```
F0 7E 7F 06 02 00 20 6B 02 00 04 00 01 00 04 00 F7
→ manufacturer 00 20 6B (Arturia), family 2, model 4, firmware 1.0.4.0
```

## Why are manufacturer IDs 1 or 3 bytes long?

The original one-byte IDs ran out, so the value `00` was reserved to mean "a three-byte ID follows". If the first byte is not zero, the ID is that single byte. If it is `00`, the ID is `00` plus two more bytes. Some IDs from osc-bridge's table:

| ID | Manufacturer | ID | Manufacturer |
|---|---|---|---|
| `01` | Sequential | `00 20 29` | Novation |
| `04` | Moog | `00 20 32` | Behringer |
| `41` | Roland | `00 20 3C` | Elektron |
| `42` | Korg | `00 20 6B` | Arturia |
| `43` | Yamaha | `00 21 09` | Native Instruments |
| `47` | Akai | `00 21 1D` | Ableton |

The table in the code is short on purpose. An ID it does not name prints as "unknown manufacturer", but it can still match a driver that declares the same `manufacturer_id`.

## How do you detect connected MIDI devices with osc-bridge?

Run `osc-bridge detect`. It finds the catalogue in the current folder or next to the binary; `--devices-dir` points it elsewhere:

```bash
osc-bridge detect
```

The command lists every MIDI port, pairs each output with the input whose name is most similar, sends the Identity Request on every output, and listens on every input for 800 ms. It then scores each device against the catalogue's hardware drivers. The score is based on name words shared with the port, the model name or driver slug found in the port name, the vendor name, and a bonus when the manufacturer ID matches. It prints the top three matches and a ready-to-run `osc-bridge run` line for the best one.

A typical result on Linux looks like this. Port numbers depend on your machine, and the identity values here are illustrative:

```
Midi Through:Midi Through Port-0 14:0  (out 0, in 0)
  identity: no reply
Minilab3:Minilab3 MIDI 24:0  (out 1, in 1)
  identity: Arturia [00 20 6B], family 2, model 4, firmware 1.0.4.0
  match: minilab3 — devices/arturia/minilab3.json (hardware-verified, score 74)
  match: generic_midi_mini — devices/generic-midi/generic_midi_mini.json (electra-preset, score 22)
  run:   osc-bridge run --device devices/arturia/minilab3.json --out-port 1 --in-port 1
```

Each match shows the driver's source tier. `hardware-verified` means tested on the device. Most drivers come from community or vendor data and are not tested (see the [catalogue](../../catalogue.md)). The `run:` line is the next step: it starts the bridge for that device. See [Getting started](../../getting-started.md).

`osc-bridge detect --no-probe` sends nothing at all. It only lists ports and matches them by name, which is useful when you don't want any SysEx on the bus.

## Can an AI assistant detect the hardware?

Yes. The `detect_hardware` tool of the osc-bridge MCP server returns the same report as JSON. For each device it gives `port`, `out_port`, `in_port`, `identity` (manufacturer ID and name, family, model, firmware), `candidates` (slug, file, source tier, score) and `run_command`. It takes `probe` (default `true`) and `timeout_ms` (default 800, between 100 and 5000). With *"What synth is plugged in?"*, an assistant calls it and can then read the matched driver with `list_routes`. Client setup is in the [MCP guide](../../MCP.md).

## Why doesn't my device answer the Identity Request?

Answering is optional. When a device stays silent, detection falls back to the port name. Common reasons:

- **The device never implemented it.** Many older instruments, and some simple controllers and boutique devices, don't reply.
- **SysEx reception is turned off.** Some synths have a setting that ignores incoming SysEx.
- **The reply is slow.** `osc-bridge detect` waits 800 ms. The MCP tool can wait up to 5 seconds with `timeout_ms`.
- **The path drops SysEx.** Some USB-MIDI interfaces or MIDI routers filter SysEx, or don't pass it both ways.
- **The port is busy.** On Windows, a port held by another program (a DAW, an editor) may not open. That device is then not probed.

A port that loops output back to input, such as Linux's Midi Through, gets its own request back. The parser rejects it because the sub-ID is `01` and not `02`, so it shows as "no reply".

## Limits

- **Matching starts from the port name.** A driver is scored only if the port name points to it. The manufacturer ID adds weight but cannot find a match alone. A synth on 5-pin DIN behind a generic interface shows up under the interface's name and usually gets no match, even if it answered.
- **Family and model numbers are shown but not used for matching.** The catalogue does not record them, so two models from the same maker are told apart by name only.
- **The run line uses the top match.** Check it. When several drivers share a name (another maker's model, or a software version of the same synth), the top one can be wrong.
- **Most drivers are untested on hardware.** A match tells you which file describes the device. It doesn't tell you that the file was verified.
- **Detection is not monitoring.** It is a single pass. Plug in a device later and you need to run it again.

## FAQ

**Is sending the Identity Request safe?**
Yes. It is a read-only query defined by the MIDI specification. It doesn't change patches or settings, and editors send it routinely.

**How do I send it by hand?**
With any tool that can send raw SysEx. On Linux: `amidi -p hw:1,0,0 -S "F0 7E 7F 06 01 F7" -d`, which sends it and prints what comes back.

**Why are the family and model numbers meaningless to me?**
Each manufacturer assigns them, and few publish a list. The manufacturer ID and firmware version are usually the useful parts.

**Can I detect without sending anything?**
Yes: `osc-bridge detect --no-probe`, or `detect_hardware` with `probe: false`.

Source: [github.com/roomi-fields/osc-bridge](https://github.com/roomi-fields/osc-bridge).
