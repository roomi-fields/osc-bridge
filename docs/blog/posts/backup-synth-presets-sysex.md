---
date: 2026-10-22
authors: [romain]
title: How to Back Up and Restore Your Synth's Presets with SysEx
description: >-
  Back up synth presets by recording the SysEx bulk dump to a .syx file, then send it back paced to restore. How it works, the commands, and its limits.
slug: backup-synth-presets-sysex
---

# How to Back Up and Restore Your Synth's Presets with SysEx

Most synthesizers can send their whole memory as System Exclusive (SysEx) messages, which is called a bulk dump. To back up your presets, you record that dump into a `.syx` file. To restore them, you send the same file back, slowly enough for the device to keep up. Since v0.13.0, osc-bridge does both from the command line (`osc-bridge backup` and `osc-bridge restore`) and from an AI assistant through its MCP server, on Linux, macOS and Windows.

<!-- more -->

## What is a SysEx bulk dump?

A bulk dump is the device sending its own memory (patches, a whole bank, sometimes global settings) as one or more SysEx messages. A SysEx message starts with the byte `F0`, carries the manufacturer's ID and the data, and ends with `F7`. The data inside is in the manufacturer's own format, and you don't need to understand it to back it up. You only need to record the bytes and play them back later.

A dump usually starts in one of two ways:

- **From the device's menu.** Many synths have a "MIDI dump", "bulk send" or "SysEx transmit" entry. You start the dump on the device while the computer listens.
- **On request.** The computer sends a dump-request message, and the device answers with its dump. The exact request bytes are device-specific and are documented in the MIDI implementation chapter of the manual.

## What is a .syx file?

A `.syx` file is the raw SysEx messages written one after another, `F0 … F7 F0 … F7 …`, with nothing around them. There is no header and no index. That is why the format is portable: almost every editor and librarian reads and writes it.

osc-bridge writes exactly that. When it reads a file back, it splits the bytes into complete `F0 … F7` messages. It drops stray bytes outside a message, a truncated last message, and any message with a status byte inside it, which would mean corrupt data. A file with no complete message is refused. So a `.syx` file made by another tool should load as long as it holds plain SysEx.

## How do you find the right MIDI port?

Run `osc-bridge detect`. It lists every MIDI port, pairs each output with its input by name, and asks each device to identify itself:

```bash
osc-bridge detect
```

Each device is printed with its port numbers, for example `Minilab3 MIDI  (out 1, in 1)`. `osc-bridge list` gives the raw list of inputs and outputs. Both `backup` and `restore` accept a port as an index (`3`) or as a case-insensitive fragment of its name (`minilab`). A fragment has to match exactly one port. If it matches several, the command stops and lists them so you can be more specific.

## How do you back up presets with osc-bridge?

Record the device's MIDI output into a file with `osc-bridge backup`, then start the dump. The basic form is:

```bash
osc-bridge backup --in-port minilab --out backups/minilab3.syx
# Listening — start the SysEx dump from the device's menu now.
```

Start the dump on the device. osc-bridge records every SysEx message it receives and stops 3 seconds after the last one. When it finishes, it prints how much it saved:

```
Saved <N> message(s), <B> bytes to backups/minilab3.syx
```

The output folder is created if it does not exist. If nothing arrives, the command gives up after 120 seconds with `no SysEx received within 120 s`.

If the device answers a dump request, send the request first. Give the request in hex, and give the output port to send it on:

```bash
osc-bridge backup --in-port 2 --out bank-a.syx \
  --request "F0 43 20 00 F7" --out-port 3
```

The bytes above only show the format. Use the request from your device's manual. Spaces and `0x` prefixes are accepted. `--request` without `--out-port` is an error.

The full set of options:

| Option | Meaning | Default |
|---|---|---|
| `--in-port` | MIDI input the device sends on (index or name fragment) | required |
| `--out` | `.syx` file to write | required |
| `--request` | Dump-request SysEx to send first, as hex | none |
| `--out-port` | MIDI output for `--request` | none |
| `--idle-secs` | Stop this many seconds after the last message | 3 |
| `--max-secs` | Give up after this many seconds in any case | 120 |

## How do you restore a .syx file?

Send it back with `osc-bridge restore`:

```bash
osc-bridge restore --out-port minilab backups/minilab3.syx
# Sent <N> SysEx message(s) from backups/minilab3.syx
```

The file can be one you recorded or one downloaded from a patch site. **Restoring overwrites the device's memory.** Whatever was in the slots the dump targets is replaced. If you care about what is on the device now, back it up first.

## Why does the restore pause between messages (gap-ms)?

A restore waits 50 ms between messages by default, because older devices drop data when a dump arrives faster than they can store it. A synth receiving a patch usually has to write it to memory before it can take the next one, and its receive buffer is small. If the messages arrive back to back, some are lost silently, and you end up with a half-restored bank.

`--gap-ms` sets the pause:

```bash
osc-bridge restore --out-port 3 --gap-ms 200 bank-a.syx
```

If presets are missing or garbled after a restore, raise the gap. `--gap-ms 0` sends at full speed. The pause goes between messages, so a file holding one large message is sent in a single write either way.

## Can an AI assistant do it for you?

Yes. osc-bridge ships an MCP server (`osc-bridge mcp`), and three of its eight tools cover this workflow. See the [MCP guide](../../MCP.md) for client setup.

- `detect_hardware` finds the ports, as `osc-bridge detect` does.
- `backup_presets` takes `in_port`, `file`, and optionally `request_hex` with `out_port`. It stops 3 seconds after the last message and gives up after `max_seconds` (default 60, between 1 and 600). It returns the file, the message count and the byte count.
- `restore_presets` takes `out_port`, `file`, `gap_ms` (default 50, capped at 2000) and `confirm_overwrite`. It refuses to run unless `confirm_overwrite` is `true`, so the client has to ask you before it overwrites anything. The tool is also marked as destructive for clients that display that.

A typical request: *"Back up my synth before we try new sounds."* The assistant calls `detect_hardware`, then `backup_presets`, and tells you when to start the dump on the device.

## What are the alternatives?

Dedicated librarians have been doing this for decades, and they are good tools:

- **macOS:** [SysEx Librarian](https://www.snoize.com/sysexlibrarian/) by Snoize, a free app for recording, storing and sending `.syx` files.
- **Windows:** [MIDI-OX](http://www.midiox.com/), a long-standing free MIDI utility with a SysEx view that can record and send dumps.
- **Linux:** `amidi` from alsa-utils. `amidi -p hw:1,0,0 -r dump.syx -t 3` records until 3 seconds of silence, and `amidi -p hw:1,0,0 -s dump.syx -i 50` sends with a 50 ms gap between messages.
- **Vendor editors.** Many manufacturers ship an editor that backs up their own devices, often with named patches. That is something a generic tool cannot offer.

osc-bridge works the same way on all three systems, takes ports by name, and can be driven by an AI assistant. It is a small generic librarian. It is not a patch editor.

## Limits

- **The device has to support dumps.** If it cannot send its memory over MIDI, there is nothing to record. Check the manual first.
- **Dump requests are not in the catalogue.** osc-bridge's device files don't carry dump-request messages for most devices, so `--request` needs bytes from the manual. Without one, start the dump from the device's menu.
- **The file is opaque.** osc-bridge doesn't decode, name or split presets. A bank stays a bank.
- **Restore is destructive.** It overwrites the target memory, with no undo beyond another backup.
- **Port problems are outside its control.** If the operating system doesn't show the device, osc-bridge can't see it either. On Windows, a port already held by another program may fail to open.
- **Not tested on every device.** The commands are generic. Timing quirks (a needed gap, a long pause between banks) depend on the device and are not documented per device.

## FAQ

**Can I use a .syx file downloaded from the internet?**
Yes, if it was made for your exact device. `osc-bridge restore` sends any file of complete SysEx messages, and the device decides what to do with it.

**Why did my backup stop early?**
The device probably paused for longer than `--idle-secs` (3 s by default) between parts of the dump. Run it again with a higher value, for example `--idle-secs 10`.

**Does it work through a USB-MIDI interface and 5-pin DIN?**
Yes, if the interface passes SysEx. Point `--in-port` and `--out-port` at the interface's ports.

**Does it need the bridge running?**
No. `backup` and `restore` open the MIDI ports themselves. On Windows, close any other program that holds the same port first.

Source and issues: [github.com/roomi-fields/osc-bridge](https://github.com/roomi-fields/osc-bridge). To get started, see [Getting started](../../getting-started.md).
