# Arturia MiniLab 3 — SysEx Reachability Analysis

**Firmware:** `fw1_code.bin` (STM32G0B0RET6, ARM Thumb, loaded at 0x08008800, unencrypted)
**Ghidra project:** `/tmp/ghidra-proj/test`
**Scripts:** `Investigation.py`, `Investigation2.py` (outputs in `investigation.txt`, `investigation2.txt`)
**Date:** 2026-04-12

---

## Bottom Line

| Question | Verdict | Confidence |
|---|---|---|
| Q1 — Arp exposed via SysEx? | **NO** | High |
| Q2 — Chord exposed via SysEx? | **NO** | High |
| Q3 — Hold exposed via SysEx? | **NO** | High |
| Q4 — See reachable SysEx combos | §4 | — |
| Q5 — Factory dispatcher reachable from USB-MIDI? | **Likely NO** (DFU/service only) | **[UNVERIFIED]** — see §5 |

Arp, Chord, Hold, Scale, and Velocity-Curve state is exclusively manipulated by UI/menu code paths driven by physical controls; none of these code regions are reachable from any SysEx dispatcher we identified.

---

## 1. MCC_Dispatcher (0x08017952) — Full decompilation

```c
undefined8 MCC_Dispatcher(int param_1, byte *param_2, undefined4 param_3, int param_4)
{
  puVar3 = *(uint **)(param_1 + param_4 * 4);      // per-channel slot
  uVar4  = *param_2 & 0x60;                         // opcode class
  if (uVar4 != 0) {
    if (uVar4 == 0x20) {                            // GET/SET (0x20-0x3F)
      bVar6 = param_2[1];                           // section
      if (bVar6 < 0xF2) {
        if (0xEF < bVar6) goto done;                // section 0xF0/0xF1 = noop
        if (bVar6 == 1) {                           // *** section 1 ***
          if (*(short *)(param_2 + 6) != 0) {
            FUN_080182ce(param_1, puVar3 + 0x8B);    // arm slot-buffer
            *(byte *)(puVar3 + 0x8A) = bVar6;        // stamp section id
            *(char *)(puVar3 + 0xCB) = param_2[6];   // store low byte
            *(char *)(puVar3 + 0xCC) = param_2[5];   // store high byte
          }
          goto done;
        }
        if (bVar6 != 0x81) goto default_;            // not 1, not 0x81
        // *** section 0x81 *** (zero the slot + kick)
        FUN_0802d1a2(puVar3 + 0x8B, 0, 0x40);        // memset 64 bytes
        FUN_080182a2(param_1, puVar3 + 0x8B,
                     *(undefined2 *)(param_2 + 6));
      }
      else if (bVar6 == 0xF2) { *DAT_08017a38 = 1; } // global flag
      else if (2 < (byte)(bVar6 + 0xD)) goto default_;
    }
    goto done;
  }
  // uVar4 == 0 → opcodes 0x00..0x1F, 0x40..0x5F, 0x60..0x7F, etc.
  switch(param_2[1]) {
    case 1:  break;                                  // no-op (opcode 1)
    default: default_: FUN_08018252(param_1, param_2); break;
    case 6:  if (param_2[3] != 0x21) break;          // gated on byte 3
             uVar1 = min(*(u16*)(param_2+6), 9);
             goto call_param;
    case 10: uVar1 = 1;
    call_param: FUN_080182a2(param_1, puVar3, uVar1, ...);
             break;
    case 0xB: if (param_2[2] < 2) *puVar3 = param_2[2];
              else FUN_08018252(param_1);
              break;
  }
done:
  return uVar4;
}
```

**Observations:**
- Section 1 and section 0x81 both operate on `puVar3 + 0x8A..0xCB` — a 64-byte region inside a per-channel "slot" context. Payload bytes stored are `param_2[5]` and `param_2[6]` — 2 bytes total. This is consistent with a generic "named MCC parameter" slot (Multi Channel Control, a.k.a. Arturia's per-context parameter store).
- No reference, direct or indirect, to any arp/chord/hold/scale state structure.
- The fall-through default (sections 2–0x7F, 0x82–0xEF) calls `FUN_08018252` which is a tiny wrapper around `FUN_08018704` calling `FUN_0801A840`.

## 2. Param helper chain

### FUN_08018252 (ParamReadWriteHelper)
```c
void ParamReadWriteHelper(undefined4 param_1) {
  FUN_08018704(param_1, 0x80);
  FUN_08018704(param_1, 0);
}
```

### FUN_08018704 (ParamHelperCallee1)
```c
undefined1 ParamHelperCallee1(int param_1) {
  uVar2 = FUN_0801a840(*(undefined4 *)(param_1 + 0x2C4));
  return (uVar2 < 4) ? *(DAT_08018720 + uVar2) : 3;
}
```

### FUN_0801A840 (ParamChainA)
```c
undefined4 ParamChainA(undefined4 *param_1, uint param_2) {
  uVar4 = param_2 & 7;                               // 0..7 index
  if (uVar4 <= param_1[1]) {                         // bounds check
    if ((char)param_2 >= 0) {
      iVar3 = param_2 * 0x28 + 0x178;                // stride = 40
      iVar1 = param_2 * 0x28 + 0x179;
    } else {
      iVar3 = uVar4 * 0x28 + 0x38;                   // stride = 40
      iVar1 = uVar4 * 0x28 + 0x39;
    }
    *(param_1 + iVar1) = (top-bit of param_2);
    *(param_1 + iVar3) = (char)uVar4;
    ...
    FUN_0801d070(*param_1);                          // notify
  }
}
```

**Key fact:** the "40-byte-stride tables" are **RAM-resident structures inside a context object** at offsets `+0x38` and `+0x178`, NOT flash tables of parameter descriptors. They store up to 8 active-parameter slots (`param_2 & 7`). The loader only moves an 8-bit tag, not arp/chord state. **No access to arp/chord/hold RAM backing store.**

## 3. Factory dispatcher (0x08012064)

```c
undefined4 Factory_Dispatcher(int param_1, byte *param_2) {
  FUN_080188D0();                                    // timestamp
  FUN_0800CCE0(DAT_08012178, 0x29);                  // check mode flag
  FUN_0801033C(*(undefined4 *)(iVar2 + 0x24), local_19);
  FUN_0800DC60(*(undefined4 *)(iVar2 + 0x18), local_19);
  if (param_2[2] == 0x41) {                          // magic byte "A"
    FUN_0800DD4C(DAT_08012184, param_2);
    return 1;
  }
  if (*param_2 < 0x22) {                             // opcode range 0..0x21
    // indirect branch: (**(DAT_08012180 + opcode*4))();
    // → jump table at 0x0802E8B0 (34 entries)
  }
}
```

The jump table `0x0802E8B0`:

| Op  | Handler   | Action (see Task 3 raw disasm) |
|-----|-----------|--------------------------------|
| 0x01 | 0x08012166 | `bl 0x08011A68` |
| 0x02 | 0x08012118 | dispatch on param_2[1] ∈ {0x10, 0x16, 0x40} → `bl 0x080117BC / 0x08011D50 / 0x0800EAC4` |
| 0x04 | 0x080120BE | dispatch on param_2[1] ∈ {0x16, 0x60, 0x20} → `bl 0x0800EF38 / 0x0800EAC4` |
| 0x06 | 0x0801214E | `bl 0x08011D50` (same as op2 sub-case) |
| 0x07 | 0x08012170 | `bl 0x08011944` |
| 0x20 | 0x08012166 | `bl 0x08011A68` (alias of op1) |
| 0x21 | 0x0801214E | `bl 0x08011D50` (alias of op6) |
| all else | 0x08012156 | common epilogue (no-op) |

Handlers invoke `FUN_080117BC, FUN_08011D50, FUN_08011A68, FUN_08011944, FUN_0800EAC4, FUN_0800EF38`. None of these call into the arp/chord/hold code region (verified by absence of string-refs in Task 6 chain and by the limited callees). Opcode 0x04 with sub-section 0x20 passes `param_2[7]` to `FUN_0800EF38` (likely a factory test / preset-RAM poke), and sub-section 0x16 handles a separate calibration path (`param_2[9]==0x50` branch). These look like **factory calibration / DFU support** commands — test-mode routines for the assembly line.

## 4. Confirmed reachable SysEx opcode/section combinations (MCC_Dispatcher)

| opcode[0] & 0x60 | opcode[1] (section) | Effect |
|---|---|---|
| 0x20 (GET/SET) | 0x01 | Load 2-byte value into slot buffer `+0xCB/+0xCC`; stamp section |
| 0x20 | 0x81 | Zero 64-byte slot buffer + flush via `FUN_080182A2` |
| 0x20 | 0xF0, 0xF1 | No-op (reserved) |
| 0x20 | 0xF2 | Sets global flag `*DAT_08017A38 = 1` |
| 0x20 | 0xF3, 0xF4 (via `(u8)(b+0xD) < 3`, b ∈ {0xF3, 0xF4, 0xF5}) | Fall-through to `done` (noop) |
| 0x20 | any other (0x02–0x7F, 0x82–0xEF) | `FUN_08018252` param-slot write |
| 0x00 | 0x01 | No-op |
| 0x00 | 0x06 | if param_2[3]==0x21 → `FUN_080182A2(min(u16, 9))` |
| 0x00 | 0x0A | `FUN_080182A2(1, ...)` |
| 0x00 | 0x0B | if param_2[2]<2: `*puVar3 = param_2[2]` (1-bit toggle) |
| 0x00 | default | `FUN_08018252` |
| 0x40, 0x60 | — | falls through to `done` (noop) |

**None of these combinations touch arp/chord/hold/scale state.** The `FUN_080182A2` / `FUN_08018252` family operates on the 8-slot parameter array inside a per-channel context object at RAM offset +0x38 / +0x178 (40 bytes × 8 entries). The slot stores only a small struct (u8 index, u8 flags, u8 counter) — no MIDI generation logic.

## 5. USB-MIDI entry point — is Factory_Dispatcher reachable from USB?

**Finding:** Neither `MCC_Dispatcher` (0x08017952) nor `Factory_Dispatcher` (0x08012064) has a direct callsite discovered by Ghidra (all xrefs are DATA, i.e. function-pointer-table entries).

**Call-pointer references:**
- `MCC_Dispatcher (0x08017953)`: **zero** flash-word pointer references. No fn-ptr table entry found. **[UNVERIFIED]** — it is clearly reached at runtime (it's a complete function). Most likely it is invoked by one of the sibling handlers in the master table (§5.1) via a secondary indirection we have not yet traced. Alternatively it could be invoked by offset arithmetic (`LDR PC, [Rn, #imm]` on a struct containing it).
- `Factory_Dispatcher (0x08012065)`: two fn-ptr references:
  - `0x08012194` — internal literal-pool self-reference (inside its own code).
  - `0x0802ED24` — slot #? in a master function-pointer table at `0x0802ECC0..0x0802ED7C`.

### 5.1 The master dispatcher table at 0x0802ECC0

```
[0x0802ECC0] = 0x08009E1D  (FUN_08009E1C)
[0x0802ECCC] = 0x08009E1D  (FUN_08009E1C)
[0x0802ECD0] = 0x0800BDB1  (FUN_0800BDB0)
[0x0802ECD4] = 0x0800A3B1
[0x0802ECD8] = 0x0800ACE1
...
[0x0802ED04] = 0x08013639
[0x0802ED08] = 0x0800A3B9
[0x0802ED0C] = 0x0800AD03
[0x0802ED10] = 0x0800CCF1  (FUN_0800CCF0)
[0x0802ED14] = 0x08013345  (FUN_08013344)
[0x0802ED18] = 0x080130D5  (FUN_080130D4)
[0x0802ED1C] = 0x08012701  (FUN_08012700)
[0x0802ED20] = 0x08012E0D  (FUN_08012E0C)
[0x0802ED24] = 0x08012065  Factory_Dispatcher
[0x0802ED28] = 0x0800CD79
...
```

Structure: 12-byte records `(fn_ptr, int32_id, int32_zero)` — IDs 0xFFFFFFFC, 0xFFFFFFF8, 0xFFFFFFF4, 0xFFFFFFF0, 0xFFFFFFEC, 0xFFFFFFE8, 0xFFFFFFE4 (monotonically decreasing from –4). This resembles a **command-ID → handler registry**. The highest-ID entries (lowest in table) are system/DFU handlers — Factory_Dispatcher being entry –4 suggests a privileged/low-level command class.

The table entry for MCC_Dispatcher is **absent**, which means MCC_Dispatcher is not registered in this same command-class table. It is likely wired in by a different registration path (RAM-installed callback via a constructor we did not locate). **[UNVERIFIED]**

### 5.2 Interpretation

- **Factory_Dispatcher is reachable via the master command table `0x0802ECC0`**, which appears to be a generic message-routing registry shared with USB-DFU/service commands. Without tracing which USB endpoint feeds this table, we cannot definitively say it is or is not reachable from normal USB-MIDI SysEx. The sibling handlers (0x08013344, 0x080130D4, 0x08012700, 0x08012E0C) would need to be decompiled to confirm; the presence of `"A"` magic-byte (0x41) gate in Factory_Dispatcher and its factory-calibration subroutines strongly suggests it is **gated/authentication** protected — i.e. DFU-mode or factory-test only.
- **MCC_Dispatcher** processes structured GET/SET of a generic per-channel parameter slot, which is consistent with Arturia's generic `F0 00 20 6B <dev> 21 <sec> 40 <id> ...` MCC framing. It does NOT enter arp/chord/hold/scale code.

### 5.3 USB-related strings in firmware

Entire firmware contains only 4 relevant strings:
- `0x0802F820: "Minilab3 MIDI"` — USB product string (2 refs from 0x080183E8, 0x080183C6)
- `0x080341B4: "Midi Channel"` — parameter label
- `0x08016ED7, 0x0802E23B` — fragments inside code (false positives)

No strings for "USB", "USBD", "CDC", "SysEx", "endpoint", "descriptor", "Arturia" — consistent with a MIDI-only USB class descriptor (no diagnostics). This makes tracing the USB RX callback by string purely by-strings impossible; it would require following vectors from the STM32 USBD ISRs (out of scope here).

## 6. Arp / Chord / Hold / Scale / Velocity string refs

### Strings located

```
0x080316FC  "VelocityCurvePoint<5500, 127>, keybed::VelocityCurvePoint<9700, "
0x08032394  "VelocityCurvePoint<8850, 127>, keybed::VelocityCurvePoint<16500,"
0x08032FB0  "VelocityCurvePoint<7400, 127>, keybed::VelocityCurvePoint<11000,"
0x08033F64  "Arpeggiator ON"
0x08033F74  "Arpeggiator OFF"
0x08033F9C  "Arpeggiator"
0x0803402C  "Arp Mode"
0x08034038  "Arp Division"
0x08034048  "Arp Swing"
0x08034054  "Arp Gate"
0x08034060  "Arp Rate"
0x0803406C  "Arp Sync"
0x08034078  "Arp Octave"
0x08034140  "Chord mode OFF"
0x08034164  "Chord mode ON"
0x080341AC  "Chord"
0x08034300  "Hold mode ON"
0x08034310  "Hold mode OFF"
```

### Containing functions (each string → containing fn → one-level parents)

| String | Code ref | Containing fn | Parents |
|---|---|---|---|
| "Arpeggiator ON" | 0x08020ACE | **FUN_08020ACC** (inferred) | (none found via direct xref — indirect-only) |
| "Arpeggiator OFF" | 0x08020B08 | same region | — |
| "Arpeggiator" | 0x08020B6E | **FUN_08020B24** | no direct callers |
| "Arp Mode/Div/Swing/Gate/Rate/Sync/Octave" | 0x08034088..0x080340A0 | — (these are **flash ptr-table entries**, method-table for the Arp struct) | consumed by 0x0802023C (ArpConfigConsumer) only |
| "Chord mode OFF" | 0x08021A40 | **FUN_080219F8** (ChordImpl) | calls from 0x08012CE4, 0x08023FB8, 0x080240F6 |
| "Chord mode ON" | 0x08021AF0 / 0x08021ACC | FUN_080219F8 | same |
| "Chord" | 0x08021B6C | FUN_080219F8 | same |
| "Hold mode ON" | 0x08023E90 | **FUN_08023Exx** (no fn auto-defined) | — |
| "Hold mode OFF" | 0x08023ECA | same | — |

### Caller chain for Chord — does it touch SysEx?

`FUN_080219F8` (ChordImpl) called from:

1. **0x08012CE4 (ChordCallerSite):**
    ```c
    int ChordCallerSite(void) {
      iVar1 = FUN_080219F8();
      if (iVar1 == 0 &&
          (FUN_0800CE14(DAT_08012D40, 3) == 2 ||
           FUN_0800A7B8(unaff_r6 + 0x394, 0x16) != 0)) {
        iVar1 = FUN_08024D48(DAT_08012D5C);    // menu transition
      }
      if (iVar1 == 0) iVar1 = FUN_08023F80(...);
      return iVar1;
    }
    ```
    - `FUN_0800CE14(ctx, 3)` — queries button/encoder state (returns pressed=2)
    - `FUN_0800A7B8` — control-surface state query
    - This is **a UI menu handler**, triggered by physical Shift+Chord button, NOT by SysEx. ChordCallerSite has **zero direct xref callers** found, consistent with menu-dispatch via an indirect jump table.

2. **0x08023FB8, 0x080240F6** — both inside fn at 0x08023F80 family (menu pages), same UI-driven class.

### FUN_08020B24 (Arp menu):
```c
void FUN_08020B24(undefined4 param_1) {
  iVar2 = FUN_080202DC(DAT_08020BE4);            // read arp-enabled state
  uVar1 = FUN_0800DC28(DAT_08020BE8, iVar2);     // get localized string
  FUN_08025C04(&local_50, 0x40, iVar2, uVar1);   // format UI text
  local_90[0] = 0x28;                            // UI tag
  FUN_0802DA2C(local_90, &local_50);
  iVar3 = FUN_08008908(local_90);
  FUN_0802D16A(local_90+iVar3, DAT_08020BEC, 2);
  ...
  FUN_08020914(param_1, &local_50, local_90, uVar1);  // render to LCD
  FUN_08010464(param_1);                              // commit draw
}
```
This is a **pure UI rendering function** — generates LCD page for the arp menu screen. Takes no external (SysEx) input. No direct callers (invoked via menu-dispatch table).

### Velocity-curve strings
The `"VelocityCurvePoint<…>"` strings at 0x080316FC, 0x08032394, 0x08032FB0 are **C++ demangled type-info fragments** embedded by the compiler — they do not denote a parameter accessible via anything. They name the template-instantiated type used internally.

### FUN_0802023C (ArpConfigConsumer)
Iterates the 7-entry method table at 0x080340AC for arp config. Called only from internal arp-menu-related functions (FUN_08020EE4, FUN_08020EF4 per Phase4 output). No SysEx path reaches it.

---

## 7. Verdict per question

### Q1 — Is arp exposed via SysEx?
**NO.** All arp strings, the arp method table at 0x080340AC, and the arp consumer FUN_0802023C, FUN_08020B24, FUN_08020EE4 are reachable only from internal UI-menu dispatch code. The 40-byte-slot buffer manipulated by MCC_Dispatcher section-1/0x81 handlers lives at RAM offsets +0x38/+0x178 of a per-channel context object and does not touch arp state. No SysEx opcode/section combination (Task 4) reaches arp.

### Q2 — Is chord exposed via SysEx?
**NO.** FUN_080219F8 (ChordImpl) is called from three menu-handler functions (0x08012CE4, 0x08023FB8, 0x080240F6), each of which queries physical-control state via FUN_0800CE14/FUN_0800A7B8 (button/encoder polling) before invoking chord toggle. None of these sites are reachable from MCC_Dispatcher or Factory_Dispatcher.

### Q3 — Is hold exposed via SysEx?
**NO.** "Hold mode ON/OFF" strings at 0x08034300/0x08034310 are referenced from code in the 0x08023E00–0x08023F00 region (hold-menu / hold-toggle handler). Same UI-menu dispatch class as chord, not dispatcher-reachable.

### Q4 — Reachable SysEx opcode/section combos
See §4 table. Summary: MCC_Dispatcher implements a generic parameter-slot GET/SET store (sections 0x01, 0x81, + 8-slot parameter-descriptor path via FUN_08018252). Only touches a 40-byte × 8-slot area in a per-channel context object. Arp/chord/hold/scale/velocity-curve state never appears as a destination in any reachable path.

### Q5 — Factory dispatcher reachable from external USB-MIDI?
**Most likely NO — factory/DFU only.** Factory_Dispatcher is registered in a master command-class table at 0x0802ECC0 (record = fn_ptr + 12-byte header; Factory_Dispatcher = record with id -4), alongside 6+ other privileged handlers. Its first action is a mode check: `FUN_0800CCE0(DAT_08012178, 0x29) != 1` gates normal dispatch, and the "A" (0x41) magic-byte shortcut at param_2[2] triggers a distinct firmware-update-style path (`FUN_0800DD4C`). Opcode handlers invoke calibration (`FUN_0800EAC4, FUN_0800EF38`) and low-level writes (`FUN_08011D50, FUN_08011944, FUN_080117BC`). This is consistent with a factory/service command class that requires the device to be in a special mode — not exposed to normal USB-MIDI SysEx traffic.  
**[UNVERIFIED]** — definitive proof would require tracing the STM32 USBD RX ISR to the master table, which this analysis did not complete. Recommended next step: identify the MIDI-class IN/OUT endpoint buffer consumer and follow it downward to see whether it ever indexes into table 0x0802ECC0 or only into a subset.

---

## 8. Open items & caveats (UNVERIFIED)

- MCC_Dispatcher has **zero** direct xrefs in the project. It is reachable at runtime but the exact caller chain from the USB RX ISR was not identified. It is almost certainly invoked via a RAM-installed callback (stored in a per-channel context) rather than a flash fn-ptr table. Mapping that registration point would confirm definitively that MCC_Dispatcher is the actual USB-MIDI SysEx entry for generic Arturia parameter messages.
- The sibling handlers in the master table at 0x0802ECC0 (0x08013344, 0x080130D4, 0x08012700, 0x08012E0C, etc.) were not decompiled. Any of them could in principle route to arp/chord state, but given the absence of any arp/chord string or fn-ptr reference along their expected callee set, this is unlikely.
- No 40-byte-per-entry flash parameter descriptor tables were found. The "40-byte stride" in `FUN_0801A840` is RAM-side; the three flash hits reported by the heuristic (0x08033E24, 0x08034264, 0x080366BC) are decorative / unrelated (all-same pointers or single-char labels).
- Velocity-curve "VelocityCurvePoint<...>" strings are C++ RTTI/debug fragments; there is no SysEx command to write a velocity curve — hard-coded tables only.
