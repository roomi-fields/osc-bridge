# Your first device JSON in 30 minutes

This is a hands-on walkthrough: by the end you'll have a working, linted,
tested `osc-bridge` driver for a CC-based synthesizer, and you'll know how to
extend it.

We'll build the driver step by step. The running example is a small CC-only
mono synth with a filter, an envelope, and an LFO — the kind of device whose
MIDI implementation fits on one page. **The CC numbers below are illustrative.**
When you follow along with a real device, swap in the values from *its* MIDI
implementation chart.

If you just want the reference (every field, every type), read
[Open Synth Descriptors](https://github.com/roomi-fields/open-synth-descriptors). This tutorial is the worked
example; that's the dictionary.

---

## Prerequisites (5 min)

```bash
git clone https://github.com/roomi-fields/osc-bridge
cd osc-bridge
cargo build --release
./target/release/osc-bridge --help
```

If `cargo build` succeeds, you're set. You do **not** need the physical synth
to write and lint a driver — only to promote it to ✅ later.

---

## Step 1 — Find your source (2 min)

Every driver must cite where its mapping comes from: the catalogue takes no
driver without one. Acceptable sources:

- the manufacturer's MIDI implementation chart / programmer's reference,
- a canonical community CSV (e.g. [pencilresearch/midi](https://github.com/pencilresearch/midi)),
- your own reverse-engineering log from the hardware.

For this tutorial, assume we have the synth's one-page MIDI chart in hand.

---

## Step 2 — Create the file (1 min)

In the SyntHub catalogue a driver's id is `<vendor>/<slug>`: the vendor
folder is the manufacturer in lowercase-kebab, the slug is the model. Write
yours wherever you like, under that name:

```bash
mkdir -p my-drivers/acme
$EDITOR my-drivers/acme/bassline.json
```

---

## Step 3 — The skeleton (3 min)

Every driver starts with a `device` block. For a CC-only synth, that's almost
all the boilerplate there is:

```jsonc
{
  "$schema": "https://w3id.org/osd/0.7/device.schema.json",
  "sources": [
    {
      "type": "vendor-doc",
      "title": "Acme Bassline MIDI Implementation Chart v1.2",
      "url": "https://acme.example/bassline/midi.pdf",
      "firmware": "1.2"
    }
  ],
  "device": {
    "name": "Acme Bassline",
    "vendor": "Acme",
    "osc_prefix": "/bassline"
  }
}
```

- `$schema` — the version of the format: 0.7. osc-bridge converts a driver of an
  earlier version as it reads it, and leaves the file as it is.
- `sources[]` — your provenance. `type` is one of `vendor-doc`,
  `hardware-verified`, `community-re`, `contributed`, … (see the schema doc).
- `osc_prefix` — every OSC route this device exposes will be prefixed with it.
  Keep it short and unambiguous.

A SysEx synth would also declare `identity.manufacturerId`, `device.device_id`
and a `sysex` header (osc-bridge closes the message with F7). A pure-CC synth
needs none of that.

---

## Step 4 — Parameters: the `cc_params` table (8 min)

This is the heart of the driver. Each entry maps one named OSC address to one
MIDI Control Change message.

```jsonc
{
  "cc_params": {
    "channel": 0,
    "entries": [
      { "osc": "/filter/cutoff",    "ways": [{ "ctrlType": "cc", "ctrlIndex": [74] }] },
      { "osc": "/filter/resonance", "ways": [{ "ctrlType": "cc", "ctrlIndex": [71] }] },
      { "osc": "/amp/attack",       "ways": [{ "ctrlType": "cc", "ctrlIndex": [73] }] },
      { "osc": "/amp/release",      "ways": [{ "ctrlType": "cc", "ctrlIndex": [72] }] },
      { "osc": "/lfo/rate",         "ways": [{ "ctrlType": "cc", "ctrlIndex": [76] }] },
      { "osc": "/lfo/depth",        "ways": [{ "ctrlType": "cc", "ctrlIndex": [77] }] }
    ]
  }
}
```

`channel` is the default MIDI channel (0–15, i.e. "channel 1"). Sending
`/bassline/filter/cutoff 64` now emits `B0 4A 40` — a CC message on channel 1.

A few things you'll reach for as your device needs them:

- **Ways** — a control lists the messages that reach it, in order of
  preference; the bridge sends the first one it can.
- **14-bit CC** — give the two numbers and the width:
  `{ "osc": "/osc/pitch", "ways": [{ "ctrlType": "cc", "ctrlIndex": [3, 35], "value_bits": 14 }] }`
- **NRPN** — a way of `"ctrlType": "nrpn"` with `"ctrlIndex": [msb, lsb]` and
  `"value_bits"` (7 or 14); the bridge emits the full NRPN sequence for you.
- **Per-entry channel** — add `"channel": 9` to one entry to override the
  default for that parameter only.
- **Grouping** — add a `"section": "Filter"` tag; it's free-form and only used
  for organisation in tooling.

Add one entry per parameter on your synth's chart. This is the bulk of the
work and the part worth getting right.

---

## Step 5 — Playing notes: `midi_out` (4 min)

If your device should also respond to note-on/off, pitch-bend, and friends,
add a `midi_out` block. Its mere presence switches on a standard set of
performance routes:

```jsonc
{
  "midi_out": {
    "default_channel": 0
  }
}
```

With that, the bridge accepts (all prefixed by `osc_prefix`):

| OSC | sends |
|---|---|
| `/note/on {note} {vel}` | Note-On |
| `/note/off {note}` | Note-Off |
| `/pitchbend {value_u14}` | Pitch-Bend |
| `/cc/{num} {value}` | raw CC |
| `/program_change {prog}` | Program Change |

Omit the whole block for a device that should not accept performance routes
(a pure effects unit, a controller).

---

## Step 6 — Lint it (2 min)

```bash
./target/release/osc-bridge lint my-drivers/acme/bassline.json
```

Expected:

```
my-drivers/acme/bassline.json: parsed OK — 0 commands, 0 replies, 0 params, 6 cc_params
```

The linter parses the file, counts what it found, and warns on any scripted
fallback (`transform` / `script`) — you have none, so it's silent. If the JSON
is malformed or a required field is missing, this is where you find out.

---

## Step 7 — Try it for real (3 min)

Inspect the driver without connecting anything:

```bash
./target/release/osc-bridge inspect my-drivers/acme/bassline.json
```

If you have the synth on a MIDI port, run the bridge and send it something:

```bash
# find your MIDI out port index
./target/release/osc-bridge list

# run the bridge (port 4 is an example)
./target/release/osc-bridge run --device my-drivers/acme/bassline.json --out-port 4 &

# send an OSC message
./target/release/osc-bridge osc-send /bassline/filter/cutoff 100
```

The filter should move. If it doesn't: double-check the CC number and channel
against the chart, and watch the bridge's stderr — it logs every message it
dispatches.

---

## Step 8 — Send it to SyntHub (1 min)

The catalogue lives on [SyntHub](https://synthub.roomi-fields.com/). From an
MCP client linked to it (`osc-bridge mcp`, or the hosted server), ask your
agent to contribute the driver: the `submit_driver` tool takes the file and
where its mapping comes from. It is parsed, linted and tested, then added to
the catalogue, where others find it under `acme/bassline` — and once you
tested it on the synth, `report_result` records that it works, on which
firmware.

No MCP client at hand? Open a
[device request](https://github.com/roomi-fields/osc-bridge/issues/new?template=new_device_request.yml)
with the file attached.

---

## Where to go next

You've covered the 90% case. The remaining 10%, in rough order of how often
you'll need it:

- **Fixed commands** — preset recall, display text, pad colours: the
  `commands[]` block builds a SysEx/OSC frame from a placeholder template.
- **SysEx parameters** — hundreds of params behind one opcode family: the
  `params` block declares the framing once and a flat address table.
- **Replies** — decode device-initiated messages back into OSC: the
  `replies[]` block.
- **Software targets** — driving a DAW or live-coding environment instead of
  hardware: `device.transport.kind = "osc"`. See the schema doc's
  "Software drivers" section.
- **Checksums** — `twos7`, `sum7` and `xor7` are declarative: a frame token
  `{ "checksum": "<algorithm>", "from": <offset> }` (and `"length"`), no Lua.
- **The Lua escape hatch** — other checksums, exotic encodings, conditional
  transforms: `transform` / `script`. Use sparingly; `lint` warns on every use.
  See [`scripting.md`](scripting.md).

All of it is in [Open Synth Descriptors](https://github.com/roomi-fields/open-synth-descriptors). And the
contributor checklist — folder conventions, the no-silent-fusion rule, how to
promote a driver to ✅ — is in [`../CONTRIBUTING.md`](../CONTRIBUTING.md).
