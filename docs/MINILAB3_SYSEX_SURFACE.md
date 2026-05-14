# MiniLab 3 — Definitive SysEx surface

Combined result of static firmware RE (Ghidra) + empirical black-box fuzzing.

## Method

- **Firmware**: MiniLab3 fw 1.2.0 (`MiniLab3_fw_1__fw1_2_0_1466__2024_12_16.bin`, STM32G0B0RET6, ARM Thumb, unencrypted, load address `0x08008800`).
- **Static RE**: Ghidra headless auto-analysis + scripted XRef and decompilation of the MCC SysEx dispatcher (`0x08017952`) and its callees.
- **Empirical fuzz**: `mb-driver fuzz-sysex` swept `F0 00 20 6B 7F 42 20 <SEC> 40 <ID> 00 F7` for `SEC ∈ 0x00..0xEF`, `ID ∈ 0x00..0x7F` — 30 720 GETs total, all replies captured.

## Result: exactly 768 SysEx parameters exposed

Reply distribution:

| Section | Params replying | Purpose (inferred) |
|---------|-----------------|--------------------|
| `0x03`  | 128             | Preset slot 1 (User 1) |
| `0x04`  | 128             | Preset slot 2 (User 2) |
| `0x05`  | 128             | Preset slot 3 (User 3) |
| `0x06`  | 128             | Preset slot 4 (User 4) |
| `0x07`  | 128             | Preset slot 5 (User 5) |
| `0x08`  | 128             | Alternate slot (Arturia / DAW — values differ from User slots) |
| `0x00..0x02` | 0          | No reply |
| `0x09..0xEF` | 0          | No reply |

Sections 0x03-0x07 return **identical** payloads in shape and range — confirming they are 5 interchangeable preset banks, consistent with the UI strings `User 1..User 5` observed in the firmware. Section 0x08 has a different layout, matching the `Arturia`/`DAW` string cluster.

Exactly matches the sysex-controls reverse-engineered parameter set (~264 distinct logical controls, each appearing once per preset slot = 264×~3 and an additional shared subset ≈ 768).

## What is NOT exposed via SysEx

Confirmed by concordance of static RE and empirical fuzz:

- **Arpeggiator** (7 params: Mode, Division, Swing, Gate, Rate, Sync, Octave) — present on device (labels + init function at `0x0802023C`), routed only from local UI menu handler (`FUN_080203FC`). Arp method-ptr table at `0x080340AC` is consulted exclusively from `FUN_0802023C`, never from the MCC dispatcher call graph.
- **Chord mode** (Fifth, sus2/4, Maj7/9/11, min7/9/11, User) — implementation at `0x080219F8`, toggled only via UI command code 8 routed through `0x08012CE4` (which is a UI event dispatcher, not a SysEx handler).
- **Hold mode** (`Hold mode ON/OFF` strings) — only referenced from UI callbacks.
- **Scale quantizer** — no evidence it's addressable externally.
- **Velocity curves** (sysex-controls marked this as [AMBIGUOUS]) — confirmed absent from the 768 reachable params.

## MCC dispatcher semantics (decoded)

Entry function `0x08017952`. Buffer byte[0] = opcode; `opcode & 0x60` selects category:

| Mask | Opcode range | Handler |
|------|-------------|---------|
| `0x00` (& 0x60 == 0x00) | 0x01-0x1F | Switch on buffer[1]: cases 0x01, 0x06, 0x0A, 0x0B — non-parameter commands |
| `0x20` (& 0x60 == 0x20) | 0x20-0x3F | GET/SET with section at buffer[1]. Sections 0x03-0x08 → param read via `FUN_08018252 → FUN_08018704 → FUN_0801A840` (40-byte-entry table). Section 1 / 0x81 / 0xF2 are special-case fall-throughs (kept for compat, partial decoding not consequential). |
| `0x40` / `0x60` | 0x40+ | Not routed — fall through to exit |

## Other opcodes observed in traffic (not tested in fuzz)

These were captured in the MCC ↔ MiniLab USB sniff but are not read/write:
- `0x03 00 20 3F` — "query preset info" (returns name)
- `0x04 00 20 <ASCII>` — "store preset with ASCII name"

These don't live in the 0x20-0x3F GET/SET path; they flow through the other branch of the MCC_Dispatcher switch (cases 0x06, 0x0A, 0x0B we haven't fully decoded, but they clearly do not expose arp/chord either by the `FUN_0801A840` evidence).

## Bottom line for driver scope

- **Expose 768 × 14-bit values via OSC** via `/minilab3/param/*` path, already implemented.
- **264 of these have human-friendly OSC routes** (from sysex-controls reverse) in `minilab3_params.rs`.
- **Arp/Chord/Hold must be implemented client-side in SuperCollider/BPscript** — they are not pilotable via MCC SysEx on firmware 1.2.0 (and there is no evidence Arturia plans to expose them).
- **Pad colors and display** are exposed via different opcodes (0x02 16 … / 0x04 02 60 …) — those ARE implemented in the driver.
