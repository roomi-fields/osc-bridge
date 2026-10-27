# Writing a driver in other formats — OSCQuery and MIDNAM

A driver file can be written in two formats other tools read:

- **OSCQuery** — the OSC addresses the bridge answers for the device, with
  their types, ranges and access, as the JSON namespace of the
  [OSCQuery proposal](https://github.com/Vidvox/OSCQueryProposal). OSC
  controllers and browsers build their controls from it.
- **MIDNAM** — the device's MIDI names as a MIDI Name Document 1.0: its
  controllers and drum notes, by name, for DAWs (Ardour, Logic, Digital
  Performer…).

The same text comes from three places:

| Where | How |
|---|---|
| Command line | `osc-bridge export --device korg/minilogue --format oscquery` (or `midnam`), on stdout |
| MCP server | tool `export_device` with `device` (id or slug) and `format`; the document is the text content |
| Running bridge | `osc-bridge run … --oscquery-bind 127.0.0.1:7891`, or `oscquery_bind` in the `[osc]` section of `bridge.toml` |

The library function is `osc_bridge::export::export(&device, format)`; the
hosted server serves the same tool, so the site and the API give the same
output.

## Credit and licence

Both documents carry the credit and licence of the driver's `_sources`
(attribution or author, firmware, licence, address). A firmware that shares
another file's MIDI (`midi_of`) is credited for its own sources and for the
shared file's ("MIDI shared from …").

- OSCQuery: in the `DESCRIPTION` of the root and of the device's node, and
  as data in an extra `SOURCES` key (`sources`, `midi_of`,
  `midi_of_sources`).
- MIDNAM: in the `<Author>` element and in a comment at the top.

## OSCQuery

The root is `/`; the device's routes are under its `osc_prefix`, next to the
bridge's own `/bridge/status` and `/bridge/docs`.

| Node | TYPE | ACCESS |
|---|---|---|
| `cc_params` entry (CC, 14-bit CC, NRPN, RPN) | `i` (value), `ii` with the NRPN LSB when the caller gives it | 2 |
| SysEx `params` entry | `i` | 2 |
| `triggers` entry | `i` (velocity), `ii` with the note when it has no natural one | 2 |
| `commands` entry | one tag per declared argument (`i`, `f`, `s`, `T`) | 2 |
| `midi_out` routes (`/note/on`, `/pitchbend`, `/cc/0` to `/cc/127`…) | as the bridge reads them | 2 |
| `/raw/syx`, `/param/get` | `s`, `iiii` | 2 |
| `/program/recall` (a driver with `programs` banks) | `si` (bank name, number), `sii` with the channel; the bank names as `VALS` | 2 |
| `/program/dump` (a driver whose dump request is known) | `si` / `sii` for a stored sound, `i` (channel) for an edit buffer | 2 |
| `/identity` (every MIDI driver); `/identity/reply` (with an identity reply) | none; `ss` (firmware, hex) | 2; 1 |
| What the bridge sends: `midi_in`, `replies`, `/sysex/raw` | one tag per argument | 1 |
| An address both sent to and sent back | | 3 |
| `custom_commands` (Lua scripts) | none: the script reads its arguments | 2 |

- `RANGE` gives `MIN`/`MAX` (or `VALS` for named choices); `EXTENDED_TYPE`
  names each argument; `TAGS` give the entry's `section` and `expects`.
- A trailing optional argument (the MIDI channel, a trigger's note, a
  command argument with a default) is given as `OVERLOADS`.
- `DESCRIPTION` says what the address sends: `NRPN 3:75`, `CC 74`, the
  trigger's note and channel, the source's description of the values.
- A path template (`/track/{n}/volume`) is written once per value of its
  captures when their range is known (at most 128 addresses); otherwise it
  is named in the device's description ("Also answers: …").
- A passthrough software driver lists no routes: its description says that
  any address under its prefix is sent on.

The listener answers `GET /` (whole namespace), `GET /<path>` (that node
and what is under it), `GET /<path>?ATTRIBUTE` (one attribute, 204 when the
node has none, 400 for an attribute it does not know) and `?HOST_INFO`
(`OSC_IP`, `OSC_PORT`, `OSC_TRANSPORT` `UDP`, and `WS_IP`/`WS_PORT` when
`--ws-bind` is on). Under `orchestrate` the namespace holds every device.

Left out: `VALUE` (the bridge keeps no state), `LISTEN` and the other
WebSocket commands (the bridge's WebSocket sends every outbound message to
every client), routes a script registers at run time, and Zeroconf
advertising (`_oscjson._tcp`): give clients the address by hand.

## MIDNAM

```xml
<MIDINameDocument>
  <Author>MIDI Guide (https://midi.guide) by Pencil Research and contributors, licence CC-BY-SA-4.0, …</Author>
  <MasterDeviceNames>
    <Manufacturer>Elektron</Manufacturer>
    <Model>Model Samples</Model>
    <CustomDeviceMode Name="Default"> … one ChannelNameSetAssign per channel … </CustomDeviceMode>
    <ChannelNameSet Name="Channel 1"> … UsesNoteNameList, UsesControlNameList, PatchBank </ChannelNameSet>
    <NoteNameList Name="Notes Channel 1"> <Note Number="0" Name="Track 1"/> … </NoteNameList>
    <ControlNameList Name="Controls">
      <Control Type="7bit" Number="3" Name="Track Note"> <Values Min="0" Max="127"/> </Control>
      …
```

- Manufacturer is the driver's vendor; Model its name, with the firmware
  when its source states one.
- `ControlNameList`: every `cc_params` entry once per MIDI address —
  `7bit` (CC), `14bit` (CC 0-31 with its LSB 32 above), `NRPN` and `RPN`
  (number = MSB × 128 + LSB; one control per LSB for an entry whose caller
  gives it), with `Values Min/Max` from its range. The name is the OSC
  address read as words (`/filter/env_amount` → "Filter Env Amount").
- Named values: when the source's `_usage` lists them (`0: Off; 1-127: On`),
  each is a `<Value>`, named at the start of its range.
- `NoteNameList`: the `triggers`, per channel. Channels hearing the same
  notes share one channel name set; a trigger on the device's basic, omni or
  active channel names its notes on every channel. A trigger without a
  natural note names each note of its range.
- `PatchBank`: one per bank of `programs`, with its bank select as
  `MIDICommands` (CC 0, CC 32) and a `Patch` per program, its number as the
  device shows it and its program change. Programs are named by bank and
  number: sound names are not in drivers (shared sounds are on SynthHub). A
  driver without banks, or whose programs are recalled without a program
  change, has one empty bank (the DTD wants one); a bank on a fixed channel
  goes only in that channel's name set.

Left out: SysEx parameters and commands, OSC routes, the recall messages
other than bank select and program change (`before`, `after`), performance messages
(notes, bend, aftertouch need no names). A software driver (OSC only) has
no MIDNAM: the export fails.
