# What a device's configuration holds, and what a driver file describes

A driver file should let a person, a program or an agent know every way a
device speaks MIDI or OSC, and share it in the formats other tools read.
This is the list of those aspects, what the schema (`DEVICE_JSON_SCHEMA.md`)
describes of each today, and the formats that carry it.

Status: **yes** described; **part** described in some ways only; **no** not
described yet (at most written in `_limitations`).

| Aspect | Schema today | Status | Carried by |
|---|---|---|---|
| Identity: maker, model, firmware, SysEx manufacturer and device id | `device`, `_sources.firmware`, `midi_of` | yes | MIDI-CI discovery, MIDNAM |
| Detection: Universal Identity Request and its reply | `replies` (patterns), no identity block | part | MIDI-CI, KnobKraft |
| Transport: DIN, USB, Bluetooth; port names; OSC host and port | OSC `transport`; MIDI ports chosen at run time | part | OSCQuery (OSC) |
| Channels and modes: basic channel, omni/poly/mono, multitimbral parts | `midi_out.default_channel`, trigger `channel_mode` | part | MIDNAM (channel name sets) |
| Performance: notes, velocity, release velocity, aftertouch, pitch bend | `midi_out`, `midi_in` | yes | MIDNAM, MIDI-CI |
| Pitch bend range and other RPNs | `cc_params` RPN entries | part | MIDI-CI PE |
| MPE: zones, manager and member channels, per-note bend, pressure, timbre | none | no | MPE spec (MCM, RPN 6), MIDI-CI profile |
| Controllers: CC (7 and 14 bit), NRPN, ranges, named values, nature | `cc_params`, `expects` | yes | MIDNAM, MIDI-CI PE, Electra, Ctrlr |
| SysEx parameters and commands, checksums, address maps | `params`, `commands`, frame grammar | yes | Electra, Ctrlr |
| Sounds: request, recognise, name, rename, identity, send back | none (KnobKraft adaptations on the hosted server) | no | KnobKraft, Edisyn |
| Programs and banks: bank select scheme, numbering, bank names, recall sequence | `/program_change` only | no | MIDNAM, MIDI-CI PE ProgramList |
| Sound names: factory banks, user banks | shared sounds on SynthHub | part | MIDNAM |
| Drum and note maps | `triggers` | yes | MIDNAM (note name lists) |
| What the device sends: knob CCs, replies, state broadcasts | `midi_in`, `replies` | part | MIDI-CI PE |
| Timing: message gap, reply wait, rate limit | `rate_limit_hz` | part | KnobKraft |
| Clock and transport: MIDI clock in/out, start/stop, song position, MMC, MTC | none | no | MIDI 1.0 |
| Global settings: master tune, local control, MIDI filters | as `cc_params` or `params` when mapped | part | MIDI-CI PE |
| Tuning: MIDI Tuning Standard (scales, per-note tuning) | none | no | MTS (MIDI 1.0) |
| Sample and file transfer: Sample Dump Standard, file dumps | none | no | SDS (MIDI 1.0) |
| MIDI 2.0: UMP, function blocks, profiles, property exchange, per-note controllers | none | no | MIDI 2.0 |
| OSC address space: addresses, types, ranges, access | OSC `commands`, `replies` | yes | OSCQuery |

## What a device holds and exchanges

Beyond its controls, a device stores contents it can send and receive,
each with its own request, format and place. A driver should name each one
the device has, how to ask for it, how to recognise it and how to send it
back (to its edit buffer or a given place), as it does for sounds.

| Content | Examples | Schema today | Status |
|---|---|---|---|
| Sounds (single, multi, performance, combination) | DX7 voice, M1 program, Blofeld multi | none (KnobKraft on the hosted server) | no |
| Drum kits | TR-8S kit, Digitone kit | none | no |
| Banks: many sounds at once | DX7 32-voice bank | none | no |
| Arpeggiator patterns | user arpeggios of Yamaha Motif, Roland JD-Xi | none | no |
| Sequences: patterns, songs, chains | Elektron patterns and songs, electribe patterns, Circuit sessions | none | no |
| Wave sequences and step modulation | Korg Wavestation and wavestate | none | no |
| Wavetables and user waveforms | Waldorf Blofeld wavetables, MicroFreak user waves | none | no |
| Samples | Sample Dump Standard, maker transfer protocols | none | no |
| Tunings and scales | MIDI Tuning Standard, scale memories | none | no |
| Chord and scale memories | NDLR, Hapax | none | no |
| Controller templates and mappings | Novation Components, Arturia MIDI Control Center, Electra One | none | no |
| Global settings | system dump, MIDI settings | none | no |
| Effect presets, amp profiles | Strymon, Chase Bliss, Kemper rigs | none | no |
| Scripts | Electra One Lua, norns scripts | norns scripts as firmware variants | part |
| The whole device state | MIDI-CI Property Exchange State resource, full backups | none | no |

SynthHub already shares contents by kind (sound, kit, sequence, bank,
project, clip); what is missing is each device's way to ask for, recognise
and send back each kind.

## Formats to read and write

| Format | Read into drivers | Write from drivers |
|---|---|---|
| MIDI Guide CSV | yes (`pencilresearch` tier) | — |
| Electra One preset | yes (`electra-*` tier) | planned |
| KnobKraft adaptation | sounds, on the hosted server | — |
| OSCQuery | — | planned |
| MIDNAM | planned | planned |
| Ctrlr panel | planned | later |
| Cakewalk instrument definition | planned | — |
| MIDI-CI Property Exchange | later | later |

Each file written from a driver carries the credit and licence of the
sources its data came from.
