# Device JSON schema (v0.1)

Every supported instrument is described by a single JSON file in `devices/<vendor>/<device>.json`.
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
  "replies":    [ ...ReplyPattern... ]
}
```

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
    "value_type": "u14",                    // for SET value encoding
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
      { "osc":"/filter/cutoff_nrpn", "nrpn_msb":3, "nrpn_lsb":75, "range":[0,16383] }
    ]
  }
}
```

OSC calls `/<prefix>/filter/cutoff 64` send `[0xB0 | channel, cc_num, value]` over MIDI.
A second OSC argument can override the channel per call: `/<prefix>/filter/cutoff 64 3` sends on channel 3.

The device's `sysex.header`/`footer` can be empty (`[]`) when there is no SysEx.

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
| `"{name:ascii}"`   | zero-terminated ASCII bytes injected at this position |
| `"{name:bytes}"`   | raw byte array inlined |

## Arg types

| Type   | Range        | Notes                       |
|--------|--------------|-----------------------------|
| `u7`   | 0..127       | one byte |
| `u14`  | 0..16383     | produces `_msb` + `_lsb` helpers automatically |
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
classical MIDI driver — the 840 hardware drivers ship without any of these
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
