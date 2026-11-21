# Corpus of detection captures

One capture per bench session, named by day (`<day>.json`), with its neighbour file
`<day>.expected.json`: a map from port name to the device id a person says it is (written by hand).

- `2026-11-21.json` is the **system listing only, without messages** of the bench of 2026-11-21
  (BCR-32, Electra One mk2, MiniLab 3): the text of `/proc/asound/cards`, the `usbid` of each card
  and the port names, copied as observed (ticket OB-80g.3.2). The full captures, with messages,
  are taken with `osc-bridge detect --write` and replace it.

A capture keeps raw replies: a maker's reply may carry a serial number; read a capture before
sharing it.
