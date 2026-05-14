# Arturia MCC SysEx Protocol — Reverse-Engineered

Captured 2026-04-12 by sniffing MCC ↔ MiniLab 3 USB traffic (USBPcap + tshark). Full bidirectional capture.

## Common manufacturer framing

```
F0 00 20 6B <DEV_HI> <DEV_LO>  <OPCODE> <...>  F7
```

- `00 20 6B` — Arturia manufacturer SysEx ID (3 bytes).
- `<DEV_HI> <DEV_LO>` — 2-byte product identifier. Observed:
  - MiniLab 3: `7F 42`
  - MatrixBrute: `06 01` (per MBPV decompilation)

## Opcode table (decoded from MiniLab 3)

| Opcode | Direction(s) | Frame size | Purpose |
|--------|-------------|------------|---------|
| `03 00 20 3F`          | host→device | 11 bytes | Query current preset info |
| `04 00 20 <ASCII> 00`  | host→device | variable | Store preset with ASCII name (timestamp auto-generated) |
| `20 <SEC> 40 <ID> 00`  | host→device | 12 bytes | **GET parameter** (read) |
| `21 <SEC> 40 <ID> <MSB> <LSB>` | both | 13 bytes | **SET parameter** (write or reply) |

### GET / SET framing (the core protocol)

```
GET (host → device, 12 bytes):
  F0 00 20 6B <DEV_HI> <DEV_LO>  20 <SEC> 40 <PARAM_ID> 00  F7

SET (either direction, 13 bytes):
  F0 00 20 6B <DEV_HI> <DEV_LO>  21 <SEC> 40 <PARAM_ID> <VAL_MSB> <VAL_LSB>  F7
```

- `<SEC>` — section / bank (7-bit). Observed: `03`, `08`. Each section holds up to 128 parameters.
- `<PARAM_ID>` — parameter index within section (0..127).
- `<VAL_MSB> <VAL_LSB>` — 14-bit value (0..16383 effective).

When MCC opens a session, it iterates over all parameter IDs in each known section, sending a GET and expecting a SET reply. That's the "dump" seen as 371 SysEx messages in 24s.

When the user drags a control in MCC, MCC sends a single SET with the new value — **live per-parameter SysEx write confirmed**.

### Store preset (opcode 04)

```
F0 00 20 6B <DEV_HI> <DEV_LO>  04 00 20  <ASCII name bytes>  00  F7
```

Observed payload: `?User 1 2026_04_12 18.43.00` (leading `?` = 0x3F, MCC-generated timestamp form `YYYY_MM_DD HH.MM.SS`).

### Query preset (opcode 03)

```
F0 00 20 6B <DEV_HI> <DEV_LO>  03 00 20  3F  F7
```

Trailing `3F` = `?` (literal byte). Likely "give me current preset name/info" — the device is expected to reply with an `04`-opcode message containing the ASCII name.

## Live performance: not SysEx

Physical knobs / pads emit **standard MIDI CC** on the `Minilab3 MIDI` port — not SysEx. Consistent with the PencilResearch CSV for MatrixBrute: live control = CC, config/edit = SysEx.

## USB-MIDI 1.0 packet framing

Over USB, each SysEx byte stream is fragmented into 4-byte USB-MIDI packets:

```
byte 0: [cable_nibble | CIN_nibble]
bytes 1-3: payload (or padding)
```

CIN values seen:
- `4` = SysEx starts/continues, 3 valid payload bytes
- `5` = SysEx ends with 1 valid byte
- `6` = SysEx ends with 2 valid bytes
- `7` = SysEx ends with 3 valid bytes

## Known sections on MiniLab 3

| Section | Observed params | Probable content |
|---------|-----------------|------------------|
| `03`    | 0..7F (full sweep) | Main controls (128 params) |
| `08`    | 0..19 observed | Secondary bank (pads/keys/etc.) |

Section meanings to be confirmed by targeted MCC-edit experiments (task #8).

## Implications for MatrixBrute driver

1. **Live per-parameter SysEx is feasible.** The `21 <sec> 40 <id> <msb> <lsb>` frame, sent host→device, is how MCC modifies individual params on the MiniLab. MatrixBrute should behave the same way (same protocol family).
2. **Mod matrix control becomes plausible.** The 256-button matrix could be 1-2 sections of param writes. Needs verification when hardware arrives.
3. **No 7↔8bit bulk encoding required** for most driver operations — just bidirectional mapping of OSC addresses ↔ `(SEC, PARAM_ID, value)` tuples.
4. **Device ID for MatrixBrute SET:** `F0 00 20 6B 06 01 21 <sec> 40 <id> <msb> <lsb> F7`. To be tested live.

## Raw captures archived

- `captures/minilab3_usb/mcc_minilab.pcap` — MCC↔MiniLab 3, both directions, recall + edit + store.
- `captures/minilab3/session1.log` — MCC→MiniLab 3, legacy loopMIDI attempt (device→host only).
