# Device JSON schema (v0.1)

Every supported instrument is described by a single JSON file, its driver: `<vendor>/<device>.json` in the [SynthHub](https://synthhub.roomi-fields.com/) catalogue (its id is that path without `.json`), or any file of yours.
The `osc-bridge` engine reads this file, registers OSC routes, and builds MIDI/SysEx frames at runtime.

## Top level

```jsonc
{
  "device": {
    "name":           "string, human-readable",
    "vendor":         "string",
    "revision":       "string (firmware version we verified against)",
    "osc_prefix":     "/minilab3",           // all routes prefixed by this
    "manufacturer_id":[0x00,0x20,0x6B],      // SysEx MID (may be 1 or 3 bytes)
    "device_id":      [0x7F,0x42],           // product bytes following MID (0..n bytes)
    "rate_limit_hz":  800                     // optional MIDI-out throttle
  },
  "sysex": {
    "header": [0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42],
    "footer": [0xF7]
  },
  "commands":   [ ...Command... ],
  "params":     [ ...Param... ],
  "midi_in":    { ...MidiInMap... },
  "triggers":   { "entries": [ ...Trigger... ] },
  "replies":    [ ...ReplyPattern... ],
  "identity":   { ...Identity... },
  "programs":   { ...Programs... },
  "loads_sounds_of": ["dx7"]
}
```

`loads_sounds_of` (optional): the slugs (`osc_prefix` without the slash) of
other devices whose sounds this one loads as they are, byte for byte — an
M-VAVE FM-1 on its stock firmware takes a Yamaha DX7 single voice. Libraries
offer those sounds for this device too. Say in `_limitations` what format it
takes and where the sound goes.

## Firmwares sharing one MIDI interface — `midi_of`

Each firmware has its own file (its sources, author, version, licence,
README), but when several speak the same MIDI, only one file holds it. The
others name that file instead of copying it:

```jsonc
{
  "_sources": [{ "type": "vendor-doc", "firmware": "grimm-1.0", "url": "…" }],
  "device": { "name": "FM-1", "vendor": "M-VAVE", "revision": "Grimm 1.0", "osc_prefix": "/fm1_grimm" },
  "midi_of": "m-vave/fm-1.vendor.fw-dinghy-0.4"
}
```

- `midi_of` is the catalogue id of the file shared: maker folder and file
  name without `.json`. It is a driver of the same device (same maker folder,
  same `device.name`). That file may itself name another: JJOS Light shares
  JJOS OS3's MIDI, which adds three CCs to the stock MPC1000's. Up to 8 links
  are followed, never in a circle.
- The file keeps its `_` keys and the `device` keys `name`, `vendor`,
  `revision`, `osc_prefix` and `kind`; everything else (transport, SysEx,
  identity bytes, MIDI in and out, routes, `loads_sounds_of`) is the shared
  file's. As loaded, it also holds `_midi_of_sources`: the shared file's
  `_sources` (and those of the files it shares from), so what is written
  from it credits where its MIDI came from (`EXPORT.md`).
- It may hold its own `identity` and `programs`, which then replace the
  shared file's: a firmware that answers in its own way, or whose program
  changes recall other things.
- It may add routes of its own: `commands`, `custom_commands`, and the
  `entries` of `params`, `cc_params` and `triggers` (a script's parameters on
  the instrument that runs it). They come after the shared file's routes; on
  the same OSC address its own wins. An address may appear only once in the
  file, and a table's framing (`channel`, frames) stays the shared file's.
- `osc-bridge lint` refuses a broken link. The tests refuse two files of one
  device whose MIDI, as loaded, is the same unless both take it from one file
  that has it, and two files adding the same routes of their own.

## Maker profiles — what a maker's models share, written once

A maker's identity request, reply pattern, firmware rule, bank style or dump
request pattern repeats across its models. It is written once, in a profile
kept with the maker's drivers, and each model names it, like `midi_of`:

```jsonc
// <maker folder>/_profile.json — profile "roland"
{
  "_about": "Roland's Universal Identity Reply …",
  "identity": {
    "device_id": { "default": 16, "min": 16, "max": 31 },
    "reply": [240, 126, "{dev}", 6, 2, 65, "{family}", "{member}"],
    "firmware": { "decode": "raw_bytes", "at": [10, 11, 12, 13] }
  }
}
// a model
{ "identity": { "profile": "roland", "family": [89, 1], "member": [0, 0] } }
```

- Profile `roland` is the file `roland/_profile.json`; `korg/logue` is
  `korg/_profile.logue.json`. A profile is the same maker's (the driver's
  folder); its blocks may name another profile of that maker (`korg/logue`
  takes `korg`), up to 8 links, never in a circle. Files starting with `_`
  are not drivers.
- `identity` and `programs` each may name a profile. As loaded, the block is
  the profile's, completed from the profiles it names, with the block's own
  keys in place of the profile's; a key set to `null` drops the profile's
  (a model without the maker's firmware rule). The loaded block keeps the
  profile's id in `profile`.
- `{family}` and `{member}` in `request`, `reply` and the `programs` frames
  stand for the model's `identity.family` and `identity.member` bytes
  (`{family:0}`: its first byte): the maker's pattern, the model's bytes.
- A firmware sharing another file's MIDI (`midi_of`) takes its blocks, and
  their profiles, unless it has its own.
- The engine fetches the profiles a driver names with it (from
  `/api/v1/devices/<maker>/_profile….json`, kept in the same cache), one at
  a time like a `midi_of` target; the hosted server serves them there.
- The catalogue refuses a block that two files of one maker hold alike: it
  belongs in a profile.

## Identity — how the device is recognised

What `osc-bridge detect` (and the server's `/v1/detect`) sends to find the
device, and how it reads the answer. Every key is optional; a device known
only by its USB port name has `ports` alone.

```jsonc
{
  "identity": {
    "request":   [0xF0, 0x7E, "{dev}", 0x06, 0x01, 0xF7], // absent: the Universal Identity Request
    "device_id": { "default": 0x10, "min": 0x10, "max": 0x1F }, // the byte {dev} stands for
    "to":        "every_port",          // or "named_ports": only ports matching `ports`
    "reply":     [0xF0, 0x7E, "{dev}", 0x06, 0x02, 0x41, 0x59, 0x01, 0x00, 0x00],
    "family":    [0x59, 0x01],          // the model's bytes, for {family} / {member}
    "member":    [0x00, 0x00],
    "reply_text": "FM-1_9",             // text the reply must also hold
    "script":    "return reply[7] == 0x59", // Lua: is this reply the device's?
    "unpack":    "lsb_8to7",            // the reply is packed 8-bit data (M-VAVE)
    "firmware":  { "decode": "raw_bytes", "at": [10, 11, 12, 13] },
    "ports":     ["PlinkySynth"]        // fragments of USB port names, any case
  }
}
```

- `request` is the whole message, F0 to F7; `{dev}` is the only placeholder,
  set to `device_id.default` (give `device_id` when it is used). Without a
  `request`, the device answers the Universal Identity Request, which every
  scan sends with device byte 7F. A maker's own request (Elektron, Behringer,
  M-VAVE…) is sent after it, to every port, or with `"to": "named_ports"` only
  to ports whose name matches `ports` — for a message another device could
  misread.
- `reply` is the start of the answer, F0 included: each number must match,
  each placeholder (`"{dev}"`, `"{any}"`) matches any one byte. The answer
  may go on past it (the version bytes).
- `"unpack": "lsb_8to7"`: the bytes between F0 and F7 are 8-bit data packed
  into 7-bit bytes, least significant bit first; `reply`, `reply_text` and
  `firmware.at` then read the unpacked data, offset 0 its first byte.
- `firmware.at` gives byte offsets from F0; `decode` reads them:

| `decode` | Bytes at `at` | Version |
|---|---|---|
| `raw_bytes` | any number | each byte in decimal, joined by dots: Roland revisions, Arturia (major first), Korg major/minor/release (`00 03 00 00` → `0.3.0.0`) |
| `korg_minor_major` | 4: minor LSB, minor MSB, major LSB, major MSB | `major.minor` |
| `yamaha_tenths` | 1: n | `1.0 + n / 10` (SEQTRAK) |
| `ascii` | 1: where the text starts | the printable text from there |
| `lua` | none: `"script"` reads `reply` (its bytes, from 1) | what the script returns |

- `script` (identity) and `firmware.script` are Lua (`scripting.md`): they
  read `reply`, the bytes of one answer (from 1), and return true or false,
  or the version. A maker whose replies no rule reads needs no new engine.

- A driver whose identity recognises the answer (or the port name) is put
  first, and among the variants of a device the one whose `_sources` firmware
  is the version read comes before those of unknown firmware, and those of
  another firmware last. Versions compare by their last number, trailing
  zeros dropped: `2.0` is `2.0.0`, `community-1.2.1` is `1.2.1`. A firmware file that
  answers differently carries its own `identity` (see `midi_of`).

## Programs — what a program change recalls

```jsonc
{
  "programs": {
    "selects":  "sound",            // or kit, pattern, scene, sequence, performance, project, snapshot, file…
    "banks": [
      { "name": "User", "msb": 81, "lsb": 0, "size": 128, "display": "001-128" },
      { "name": "B", "lsb": 1, "first_pc": 1, "size": 72, "first_number": 129, "selects": "kit", "channel": 9 }
    ],
    "requires": ["Rx.PG and Rx.Bank Sel on"],
    "before":   [[0xF0, 0x42, "{channel:u7+0x30}", 0x75, 0x4E, 0x02, 0xF7]],
    "after":    [],
    "program_change": true,
    "delay_ms": 10,
    "dump": { "kind": "request", "request": [0xF0, 0x42, "{channel:u7+0x30}", 0x00, 0x01, 0x51, 0x1C, "{index:u14_lsb}", "{index:u14_msb}", 0xF7] },
    "notes": "free text"
  }
}
```

- A bank sends CC 0 when it has `msb`, CC 32 when it has `lsb` (none, one or
  both), then the program change `first_pc` + the program's place in the
  bank. `size` programs, numbered from `first_number` (1 by default) as the
  device shows them; `display` says how the device writes them
  (`A01-A16`, `000-127`). A bank's `selects` and `channel` (0-15) override
  the block's when a bank recalls something else, or on its own channel.
- `requires`: device settings that must be on first (receive switches, a
  mode). `before` and `after`: whole messages sent before the bank select
  and after the program change (a mode change, a panel button). With
  `"program_change": false` no program change is sent: `before` and `after`
  recall on their own (a SysEx scene change, Song Select `[0xF3, "{program}"]`).
- `dump` says how a stored sound is read back: `request` (a request names the
  stored program), `edit_buffer` (recall it, wait `delay_ms`, then ask for the
  sound being played), `manual` (sent from the device's menu only) or `none`;
  `request` holds the message when its bytes are known.
- The frames may use `{channel}` (0-15), `{dev}` (`identity.device_id`),
  `{msb}`, `{lsb}`, `{bank}` (its place in `banks`, from 0), `{program}` (the
  program change value), `{index}` (the program's place over all banks, from
  0) and `{number}` (as the device shows it).
- `recall_script` (Lua) writes the recall in place of `before`, the bank
  select, the program change and `after`, and `dump.script` the dump
  request: they read `ctx` (`channel`, `dev`, `family`, `member`, `bank`,
  `bank_name`, `msb`, `lsb`, `program`, `index`, `number`) and return the
  messages as byte lists — a stored sound named in text, say.
- `{checksum:twos@N}` in a frame is the two's complement, 7 bits, of the bytes
  from offset N to it (Roland, Yamaha).
- The bridge answers `/<prefix>/program/recall <bank> <number> [channel]`:
  the bank by name (any case) or by its place from 0, the number as the device
  shows it; it sends `before`, the bank select, the program change and
  `after`. `/<prefix>/program/dump [<bank> <number>] [channel]` sends the
  dump request (for an edit buffer, recall first and wait `delay_ms`). Every
  MIDI device answers `/<prefix>/identity` (the request its identity block
  names, else the Universal one); the reply comes back as
  `/<prefix>/identity/reply <firmware> <hex>`. The addresses are the same
  whatever the maker. OSCQuery lists them; MIDNAM writes one `PatchBank` per
  bank with its bank select.
- A block the engine cannot use (an unknown `decode` or dump kind, a bank
  past program change 127, a placeholder it does not know) makes the driver
  fail to load.

## Command — fixed SysEx utility routes

Used for framed commands that are not individual parameter read/writes (init, preset recall,
display text, pad colors, etc.).

```jsonc
{
  "osc":    "/pad/{pad}/color",             // OSC path template, {} are captured args
  "args":   [                                // ordered list of OSC arguments
    {"name":"pad","type":"u7"},
    {"name":"r","type":"u7"},
    {"name":"g","type":"u7"},
    {"name":"b","type":"u7"},
    {"name":"mode","type":"enum","values":{"user":0,"arturia":1,"daw":2},"default":"user"}
  ],
  "frame":  [                                // bytes after header, before footer
    0x02, "{mode}", 0x16, "{pad_id}", "{r}", "{g}", "{b}"
  ],
  "pre":    [                                // optional — derive intermediate vars
    {"bind":"pad_id","expr":"pad + 4"}      // simple arithmetic on bound args
  ]
}
```

## Param — bulk read/write with shared opcode

For devices where one opcode family (like Arturia `20/21 <sec> 40 <id>`) addresses
hundreds of params, declare the framing ONCE and a flat table of addresses.

```jsonc
{
  "params": {
    "get_frame":  [0x20, "{sec}", 0x40, "{id}", 0x00],
    "set_frame":  [0x21, "{sec}", 0x40, "{id}", "{val_msb}", "{val_lsb}"],
    "value_type": "u14",                    // for SET value encoding: u7, u14 or s32
    "entries": [
      { "osc":"/global/backlight",     "sec":0, "id":0x07, "range":[0,1] },
      { "osc":"/knob/1/cc_number",     "sec":0, "id":0x22, "range":[0,127] },
      // ... one entry per logical parameter
    ]
  }
}
```

## CC params — pure MIDI CC devices

For devices that don't speak SysEx at all (like the Polyend Synth), parameters
can be declared as plain MIDI Control Change messages:

```jsonc
{
  "cc_params": {
    "channel": 0,                            // default MIDI channel (0..15)
    "entries": [
      { "osc":"/filter/cutoff", "cc":74, "range":[0,127], "orientation":"0-based" },
      { "osc":"/filter/resonance", "cc":71, "range":[0,127] },

      // 14-bit CC pair: `cc` is MSB, `cc_lsb` is LSB, range up to 16383
      { "osc":"/osc1/wave", "cc":9, "cc_lsb":41, "range":[0,16383] },

      // NRPN addressing: emits CC99/CC98/CC6 (+CC38 for 14-bit) automatically
      { "osc":"/osc1/octave_nrpn", "nrpn_msb":3, "nrpn_lsb":95, "range":[0,3] },
      { "osc":"/filter/cutoff_nrpn", "nrpn_msb":3, "nrpn_lsb":75, "range":[0,16383] },

      // NRPN whose LSB the caller picks (a drum note): `/drum/cutoff 90 36`
      { "osc":"/drum/cutoff", "nrpn_msb":20, "nrpn_lsb_range":[12,108], "range":[0,127] },

      // RPN addressing: emits CC101/CC100/CC6 (+CC38 for 14-bit)
      { "osc":"/pitch_bend_sensitivity", "rpn_msb":0, "rpn_lsb":0, "range":[0,24] }
    ]
  }
}
```

OSC calls `/<prefix>/filter/cutoff 64` send `[0xB0 | channel, cc_num, value]` over MIDI.
A second OSC argument can override the channel per call: `/<prefix>/filter/cutoff 64 3` sends on channel 3
(the third, after the LSB, for an entry with `nrpn_lsb_range`).

`_usage` (optional, from the source) says what the values mean, as
`N: name; N-M: name` (`"0: Off; 1-127: On"`). It changes nothing sent; a
MIDNAM export turns such a list into named values.

The device's `sysex.header`/`footer` can be empty (`[]`) when there is no SysEx.

## Control nature — `expects`

Every surface entry (`cc_params.entries[]` and SysEx `params.entries[]`) may carry
an **`expects`** field describing **what the control expects on the device side**.
It is **purely informative**: osc-bridge does *not* change any byte it emits based
on it, and adds no runtime behaviour (no timers, no held state). It just travels in
the JSON so a caller can decide how to drive the control.

> `expects` is **not** the signal. CV / Gate / Trig / Clock are *realizations* that
> belong to the caller's runtime (e.g. a live-coding language). osc-bridge stays
> universal and only states the device-side nature; the runtime maps nature →
> signal. This keeps the surface free of any one signal model.

The field is **open** (any string is accepted), defaults to **`continuous`**, and
the recommended vocabulary is:

| `expects`    | The device-side control is…                  | A runtime might realize it as |
|--------------|----------------------------------------------|-------------------------------|
| `continuous` | a value over a range (**default**)           | CV                            |
| `switch`     | an on/off state, **latched**                 | Gate                          |
| `momentary`  | active **only while held**                   | Gate (non-latching)           |
| `trigger`    | a **one-shot** event                         | Trig                          |
| `clock`      | a **steady pulse** (tempo)                   | Clock                         |
| `discrete`   | a **stepped / enumerated** choice            | stepped selection             |

```jsonc
{
  "cc_params": { "entries": [
    { "osc":"/filter/cutoff", "cc":74, "range":[0,127] },               // expects "continuous" (default)
    { "osc":"/arp/hold",      "cc":64, "range":[0,1], "expects":"switch"  },
    { "osc":"/env/trig",      "cc":109,                "expects":"trigger" }
  ] }
}
```

The `range` / `values` already on an entry carry the bounds or enum data; `expects`
adds only the *nature*. Unannotated entries (the vast majority) are `continuous`, so
existing drivers are unchanged.

## Midi-out — performance MIDI (OSC → MIDI notes)

When present, this section enables standard performance-MIDI OSC routes
prefixed by `device.osc_prefix`:

```jsonc
{
  "midi_out": {
    "default_channel": 0,    // 0..15 (MIDI channel 1..16)
    "note_offset":     0     // i8, applied to every inbound note
  }
}
```

Enabled routes (the trailing `{channel}` is optional; omit to use
`default_channel`):

| OSC                                        | MIDI bytes                                  |
|--------------------------------------------|---------------------------------------------|
| `/note/on {note} {vel} [{channel}]`        | `0x90\|ch, note+note_offset, vel`           |
| `/note/off {note} [{vel}] [{channel}]`     | `0x80\|ch, note+note_offset, vel (default 0)` |
| `/pitchbend {value_u14} [{channel}]`       | `0xE0\|ch, lsb, msb`                        |
| `/aftertouch {value} [{channel}]`          | `0xD0\|ch, value`                           |
| `/poly_aftertouch {note} {val} [{ch}]`     | `0xA0\|ch, note, value`                     |
| `/cc/{num} {value} [{channel}]`            | `0xB0\|ch, num, value`                      |
| `/program_change {prog} [{channel}]`       | `0xC0\|ch, prog`                            |

Omit the `midi_out` section entirely for pure-configuration devices
(controllers, hosts) that should not accept performance routes.

## Triggers — named drum sounds (OSC → MIDI note)

Drum machines and samplers fire sounds from MIDI **notes**, not CCs. A
`triggers` block names each sound, so `/drm1_mkiv/drum/kick` plays the kick
without the caller knowing it is note 36 on channel 10.

```jsonc
"triggers": {
  "entries": [
    { "osc": "/drum/kick", "sound": "Kick", "section": "General",
      "channel": 9,                 // 0-15; omit when the channel is user-set
      "note_range": [36, 36], "note": 36,
      "velocity_usage": "1~127: Accent level" },
    { "osc": "/drum/chromatic", "sound": "Chromatic",
      "channel_mode": "auto",       // "fixed" (default) | "basic" | "omni" | "auto"
      "note_range": [12, 59] }      // no natural note: the caller passes one
  ]
}
```

| OSC in                                     | MIDI out                                   |
|--------------------------------------------|--------------------------------------------|
| `<osc> [{velocity}] [{note}] [{channel}]`  | `0x90\|ch, note, velocity`                  |

- `velocity` defaults to 100 and is clamped to 1-127 (velocity 0 would be
  read as Note Off).
- `note` defaults to the entry's natural `note`; it must stay inside
  `note_range`, and is required when the entry has no `note`.
- `channel` defaults to the entry's `channel`, else to
  `midi_out.default_channel` (devices listening on a user-set basic channel,
  in omni, or on the active track).
- Only Note On is sent: drum voices are one-shots. Use `/note/off` (with
  `midi_out`) when a device gates on note length.

Drivers imported from MIDI Guide carry the device's drum map here, adapted
from its `<Device>.triggers.csv`.

## Midi-in map — inbound MIDI → OSC

```jsonc
{
  "midi_in": {
    "note_on":    "/note/on {note} {velocity} {channel}",
    "note_off":   "/note/off {note} {velocity} {channel}",
    "cc":         "/cc/{num} {value} {channel}",
    "pitchbend":  "/pitchbend {value_u14} {channel}",
    "aftertouch": "/aftertouch {value} {channel}"
  }
}
```

## Replies — match inbound SysEx, emit OSC

For device-initiated messages (GET replies, unsolicited state broadcasts).

```jsonc
{
  "replies": [
    {
      "match_frame": [0x21, "{sec}", 0x40, "{id}", "{msb}", "{lsb}"],
      "emit_osc":    "/param/value {sec} {id} {msb} {lsb}"
    }
  ]
}
```

## Byte placeholder grammar in `frame`

| Literal      | Meaning |
|--------------|---------|
| `0x5A`, `90` | fixed byte |
| `"{name}"`   | one byte from bound arg `name` (clamped 0..127) |
| `"{name:u14_msb}"` | high 7 bits of a u14 arg |
| `"{name:u14_lsb}"` | low 7 bits  |
| `"{name:u7+N}"` | the value plus N (decimal or `0x` hex): a channel in a status nibble, Korg `3g` = `{channel:u7+0x30}` |
| `"{name:s32}"`     | five bytes: a signed value in two's complement, 7 bits each, bits 28-31 first (Korg M3 and later); with `value_type: "s32"` the range may go below 0 |
| `"{name:ascii}"`   | zero-terminated ASCII bytes injected at this position |
| `"{name:bytes}"`   | raw byte array inlined |

## Arg types

| Type   | Range        | Notes                       |
|--------|--------------|-----------------------------|
| `u7`   | 0..127       | one byte |
| `u14`  | 0..16383     | produces `_msb` + `_lsb` helpers automatically |
| `s32`  | its `range` (may be negative) | signed; place it with `{name:s32}` |
| `enum` | see `values` | integer after lookup        |
| `string` | any text  | ASCII only, filtered |
| `bool` | 0/1          | |

## Software drivers (OSC transport)

Drivers can target a software OSC endpoint (a DAW, a live coding environment)
instead of a MIDI port. Same JSON schema, two extra blocks at the top, and the
`commands[].frame` is replaced by `commands[].forward`.

```jsonc
{
  "device": {
    "name": "Ableton Live", "vendor": "Ableton",
    "osc_prefix": "/ableton",
    "kind": "software",                            // optional, default "hardware"
    "transport": {
      "kind": "osc",                               // "midi" (default) or "osc"
      "host": "127.0.0.1",
      "port": 11000,                               // bridge → target
      "reply_port": 11001                          // target → bridge
    }
  }
}
```

`device.kind` is informational (filtering in the catalogue). `transport.kind`
is what flips the runtime branch. When absent, the driver behaves as a
classical MIDI driver — the hardware drivers ship without any of these
fields and stay unchanged.

### Command — OSC forward

When `transport.kind = "osc"`, commands use `forward` instead of `frame`:

```jsonc
{
  "osc": "/transport/tempo",                       // OSC path the client uses
  "args": [{"name": "bpm", "type": "float", "range": [20, 999]}],
  "forward": {
    "path": "/live/song/set/tempo",                // OSC path emitted to target
    "args": ["{bpm}"]                              // typed literals or {name}
  }
}
```

`forward.args` entries are either:
- `Int`, `Float`, `Bool` JSON literals (emitted as the corresponding OSC type)
- a string `"{name}"` resolved from the command's args / path captures
- a literal string (emitted as OSC string)

Placeholders in `forward.path` are interpolated similarly.

### Subscriptions

One-shot OSC messages emitted to the target at startup, typically to subscribe
to remote state changes:

```jsonc
{
  "subscriptions": [
    {
      "on": "startup",
      "forward": { "path": "/live/song/start_listen/tempo", "args": [] }
    }
  ]
}
```

Only `on: "startup"` is supported today. Future lifecycle events (per-track
selection, preset load) will reuse the same block shape.

### Replies — OSC mode

When the target pushes an OSC message back to the bridge's `reply_port`, the
replies table maps it back into the device's named surface:

```jsonc
{
  "replies": [
    {
      "match_osc":  "/live/song/get/tempo",
      "match_args": [{"name": "bpm", "type": "float"}],
      "emit_osc":   "/transport/tempo {bpm}"
    },
    {
      "match_osc":  "/live/track/get/volume",
      "match_args": [
        {"name": "n", "type": "int"},
        {"name": "v", "type": "float"}
      ],
      "emit_osc":   "/track/{n}/volume {v}"
    }
  ]
}
```

`match_osc` and `match_args` are mutually exclusive with `match_frame` (the
MIDI/SysEx form). The runtime picks the right branch per entry based on which
one is non-empty.

### OSC types currently supported

In `args` (incoming) and `forward.args` / `match_args` (outgoing/replies):
`int`, `float`, `string`, `bool`. The runtime is tolerant on int↔long and
float↔double width when reading replies. Other rosc tags (`blob`, `timetag`,
…) will be added on demand from a real driver.

### Candidates for software drivers

- Ableton Live via [AbletonOSC](https://github.com/ideoforms/AbletonOSC) —
  `📡 third-party-osc` (community Remote Script).
- Sonic Pi — `📡 vendor-osc-api` (native OSC input).
- SuperCollider — `📡 vendor-osc-api` (`NetAddr` / `OSCdef`).
- Pure Data — `📡 vendor-osc-api` (`netreceive`).
- Reaper — `📡 vendor-osc-api` (Control Surfaces / OSC).
- Bitwig via [DrivenByMoss](https://github.com/git-moss/DrivenByMoss) —
  `📡 third-party-osc` (officially partnered).

DAWs without a native OSC server (Logic Pro, FL Studio, Pro Tools, Cubase)
stay out of scope for this transport — they remain the territory of
specialised third-party MCPs.

## Extensibility

- Missing features degrade gracefully (engine logs and ignores unknown keys in the JSON).
- Device-specific exotic behaviour not covered by the schema can be implemented as a
  **plugin** — a Rust crate under `plugins/<vendor>-<device>/` that registers extra
  OSC routes. JSON-only devices don't need any Rust code.
