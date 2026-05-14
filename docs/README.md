# Documentation

| File | Purpose |
|------|---------|
| [`DEVICE_JSON_SCHEMA.md`](DEVICE_JSON_SCHEMA.md) | **Reference** — the full schema for `devices/<vendor>/<synth>.json` files. Read this before writing a new device spec. |
| [`ARTURIA_SYSEX_GENERIC.md`](ARTURIA_SYSEX_GENERIC.md) | Generic SysEx framing used by all Arturia devices (MiniLab, KeyLab, BeatStep, DrumBrute, MatrixBrute, etc.). |
| [`MINILAB3_PROTOCOL.md`](MINILAB3_PROTOCOL.md) | Complete parameter table (~350 logical controls) for the MiniLab 3, extracted from the reference GPL-3 implementation. |
| [`MINILAB3_SYSEX_SURFACE.md`](MINILAB3_SYSEX_SURFACE.md) | The 768 SysEx parameters actually reachable on the device — empirically validated. |
| [`MINILAB3_FIRMWARE_COMPLETE_RE.md`](MINILAB3_FIRMWARE_COMPLETE_RE.md) | Firmware-level reverse engineering write-up: dispatcher locations, call graph, proof that the arpeggiator/chord mode/hold are *not* exposed via MCC SysEx. |
| [`MATRIXBRUTE_SYSEX.md`](MATRIXBRUTE_SYSEX.md) | Patch file format for Arturia MatrixBrute, decoded from the MBPV viewer. Groundwork for the future MatrixBrute device JSON. |
