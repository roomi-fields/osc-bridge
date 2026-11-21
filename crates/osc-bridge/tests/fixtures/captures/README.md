# Corpus of detection captures

One capture per bench session, named by day (`<day>.json`), with its neighbour file
`<day>.expected.json`: a map from port name to the device id a person says it is (written by hand).

- `2026-11-21.json` is the full capture of the bench of 2026-11-21 (Behringer BCR-32, Electra One
  mk2 on firmware 5.0.0f, Arturia MiniLab 3), taken with `osc-bridge detect --write`: the system's
  listing, the port names, the requests sent and every message received with its time.

A capture keeps raw replies: a maker's reply may carry a serial number; read a capture before
sharing it.
