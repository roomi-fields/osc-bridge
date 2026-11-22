# Corpus of detection captures

One capture per bench session, named by day (`<day>.json`), with its neighbour file
`<day>.expected.json`: a map from port name to the device id a person says it is (written by hand).

- `2026-11-21.json` is the full capture of the bench of 2026-11-21 (Behringer BCR-32, Electra One
  mk2 on firmware 5.0.0f, Arturia MiniLab 3), taken with `osc-bridge detect --write`: the system's
  listing, the port names, the requests sent and every message received with its time.

- `2026-11-21-banc-7-appareils.json` is the capture of the bench of seven devices (sixteen ports; PC3).
  The serial number of the Electra One in its reply is replaced by `EO2-00000000`; nothing else is
  changed. Its truth: the driver of the corpus catalogue (BCR-32, Electra One, MiniLab 3), or null for
  Midi Through and for the devices with no driver in that catalogue (Deluge, MatrixBrute, Moog Sub 37, Osmose).
- `2026-11-22-ports-logiciels.json` holds software ports (FLUID Synth, Microsoft GS Wavetable Synth, Controller,
  Audio Interface MIDI, TiMidity, Pure Data, Midi Through), written by hand: its truth is null for each.

A truth value is a driver id, null, or `{"wrong_today": {"truth": <id or null>, "ticket": "<ticket id>"}}`
for a port the detection gets wrong today; the test refuses the marker once the detection gives the truth.
Keys starting with `_` are notes.

A capture keeps raw replies: a maker's reply may carry a serial number; read a capture before
sharing it.
