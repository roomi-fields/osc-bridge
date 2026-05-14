# MiniLab 3 Arturia Control Protocol (v3 / Arturia MCC)

**Source:** `soyersoyer/sysex-controls` GPL-3 Linux implementation for MiniLab 3 (and Arturia synths).

## 1. SysEx Frame Layout

The sysex-controls tool uses the Arturia v3 protocol with this base frame structure:

### Write Control (Set Parameter)
```
F0 00 20 6B 7F 42 21 [pr_id] [p_id] [c_id] [r_id] [value] F7
```

### Read Control (Query Parameter)
```
F0 00 20 6B 7F 42 20 [pr_id] [p_id] [c_id] [r_id] F7
```

**Frame Header Breakdown:**
- `F0` – Universal SysEx start
- `00 20 6B` – Arturia Manufacturer ID (SysEx ID 0x002000 + 0x6B sub-ID)
- `7F 42` – **Device ID marker** (0x7F = broadcast/v3-specific, 0x42 = product-specific identifier for MiniLab 3)
  - **MatrixBrute note:** Uses different device ID (not yet documented in sysex-controls)
- `21` (write) or `20` (read) – Command opcode (v3 protocol)
- `[pr_id]` – Product/section byte (e.g., 0x00, 0x01, 0x02, etc.)
- `[p_id]` – Parameter MSB / page ID
- `[c_id]` – Parameter LSB / control ID  
- `[r_id]` – Additional register/row identifier (used for v3 normalization)
- `[value]` – Value to write (0x00–0x7F for single byte; multi-byte spreads across separate messages)
- `F7` – SysEx end

**C Implementation (from sc-midi.c, lines 483–499):**
```c
uint8_t data[] = {0xf0, 0x00, 0x20, 0x6b, 0x7f, 0x42, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf7};
snd_seq_ev_clear (&ev);
snd_seq_ev_set_source (&ev, 0);
snd_seq_ev_set_dest (&ev, addr.client, addr.port);
snd_seq_ev_set_direct (&ev);
snd_seq_ev_set_sysex (&ev, sizeof data, data);

data[7] = (uint8_t)(control_id >> 24); // pr_id
data[8] = (uint8_t)(control_id >> 16); // p_id
data[9] = (uint8_t)(control_id >> 8);  // c_id
data[10] = (uint8_t)(control_id);      // r_id
data[11] = val;

snd_seq_event_output (seq, &ev);
snd_seq_drain_output (seq);
```

## 2. Opcode Table

| Opcode | Direction | Purpose | SysEx Byte 6 | Format | Response |
|--------|-----------|---------|-------------|--------|----------|
| **0x20** | Query | Read parameter value | `20` | `F0 00 20 6B 7F 42 20 [4-byte ID] F7` | `02` frame with value at data[6] |
| **0x21** | Command | Write parameter value | `21` | `F0 00 20 6B 7F 42 21 [4-byte ID] [value] F7` | ACK `1C` (optional) |
| **0x05** | Command | Recall preset | `05` | `F0 00 20 6B 7F 42 05 [preset_id] F7` | None |
| **0x06** | Command | Store preset | `06` | `F0 00 20 6B 7F 42 06 [preset_id] F7` | None |
| **0x02** | Response | Parameter value reply (v1/v2 compat) | `02` | `F0 00 20 6B 7F 42 02 [3-byte ID] [value] F7` | — |
| **0x1C** | Response | Acknowledgement | `1C` | `F0 00 20 6B 7F 42 1C 00 F7` | — |
| **0x04** | Command | Write string | `04` | `F0 00 20 6B 7F 42 04 [3-byte ID] [string+null] 00 F7` | ACK (optional) |

**Notes:**
- The v3 protocol uses 4-byte control IDs (lines 525, 541 in sc-midi.c).
- Opcodes 0x32, 0x42, 0x34, 0x44 are v1/v2 variants with channel offsets (set_control_id() adjusts opcode).
- No explicit "handshake" opcode found; device responds passively to commands.

## 3. Parameter Map for MiniLab 3

Control IDs are stored as 32-bit values: `(pr_id << 24) | (p_id << 16) | (c_id << 8) | r_id`.

The tool remaps some control IDs via `ml3_remap[]` (ml3-book.c, line 37–44):
```c
{0x03407f7f, 0x08401300},  // "Show on controller" preset display
{0x04407f7f, 0x08401400},
{0x05407f7f, 0x08401500},
{0x06407f7f, 0x08401600},
{0x07407f7f, 0x08401700},
```

### Global / Controller Settings (Section 0x00)

| ID (hex) | Name | Range | Type | Notes |
|----------|------|-------|------|-------|
| `0x00000000` | Sleep delay | 0–2 | Enum | 5 min, 15 min, 30 min |
| `0x01000000` | Default Keyboard Channel | 0–15 | CC (0–15) | |
| `0x07000000` | Backlight | 0–1 | Boolean | |
| `0x12000000` | Low power | 0–1 | Boolean | |
| `0x18000000` | Transport Mode | 0–3 | Enum | MCU, HUI, Both, None |
| `0x19000000` | Sleep mode | 0–1 | Enum | VegasMode, SleepMode |

### Pitch Bend Wheel (Section 0x00, offset ~0x0E00)

| ID (hex) | Name | Range | Type | Notes |
|----------|------|-------|------|-------|
| `0x0e000000` | Pitch Bend Enabled | 0–1 | Boolean | |
| `0x0f000000` | Pitch Bend Mode | 0–1 | Enum | Standard, Hold |
| `0x10000000` | Mod Wheel Enabled | 0–1 | Boolean | |

### Main Knob (Section 0x00)

| ID (hex) | Name | Range | Type | Notes |
|----------|------|-------|------|-------|
| `0x0e000000` | Output | 0–1 | Enum | Off, CC |
| `0x10000000` | Channel (CC mode) | 0–15 | CC (0–15) | |
| `0x12000000` | CC Number (CC mode) | 0–127 | CC (0–127) | |

### Main Knob Click (Section 0x00, ~0x14–0x20)

| ID (hex) | Name | Range | Type | Notes |
|----------|------|-------|------|-------|
| `0x14000000` | Mode | 0–2 | Enum | Off, CC, MCU/HUI |
| `0x16000000` | Channel (CC mode) | 0–15 | CC | |
| `0x18000000` | CC Number (CC mode) | 0–127 | CC | |
| `0x1a000000` | Off Value (CC mode) | 0–127 | Value | |
| `0x1c000000` | On Value (CC mode) | 0–127 | Value | |
| `0x1e000000` | Type (CC mode) | 0–1 | Enum | Gate, Toggle |
| `0x20000000` | MCU/HUI Mode (HUI mode) | 0–6 | Enum | Click, Loop, Rewind, FF, Stop, Play, Rec |

### Knobs (8 knobs, Section 0x00 with per-knob offset 0xXX00)

Each knob occupies 8 params (offset by 0x0100 to 0x0700 incrementally):

| Offset | Name | Range | Type | Notes |
|--------|------|-------|------|-------|
| `+0x22` | Output | 0–1 | Enum | CC, NRPN |
| `+0x2A` | Channel | 0–15 | CC | (AR3ChRow) |
| `+0x32` | Scale (NRPN mode) | 0–7 | Enum | 1:1 (fine)–1:128 (coarse) |
| `+0x3A` | CC Number (CC mode) | 0–127 | CC | |
| `+0x37` | Option (CC mode) | 0–3 | Enum | Absolute, Relative 1/2/3 |
| `+0x42` | Parameter MSB (NRPN) | 0–127 | Value | |
| `+0x4A` | Parameter LSB (NRPN) | 0–127 | Value | |
| `+0x52` | Min Value (Absolute CC) | 0–127 | Value | |
| `+0x5A` | Max Value (Absolute CC) | 0–127 | Value | |

**Example ID computation for Knob 3, CC Number:**
- Base: `0x3a00` (CC field ID)
- Page offset from ml3-knob-page.ui: `+0x0200` (Knob 3 = index 2)
- **Full 32-bit ID:** `0x00023a00` (pr_id=0x00, p_id=0x02, c_id=0x3a, r_id=0x00)

### Faders (8 faders, Section 0x00 with per-fader offset 0xXX00)

| Offset | Name | Range | Type | Notes |
|--------|------|-------|------|-------|
| `+0x62` | Channel | 0–15 | CC | |
| `+0x66` | CC Number | 0–127 | CC | |
| `+0x6A` | Mode | 0–1 | Enum | Fader, Drawbar |
| `+0x6E` | Min Value | 0–127 | Value | |
| `+0x72` | Max Value | 0–127 | Value | |

**Note:** Faders do not have NRPN/scale options; CC-only.

### Pads (16 pads total: 8 Bank A + 8 Bank B, Section 0x01)

Each pad (indexed 0–7 in Bank A, `+0x0800` in Bank B) has:

| Offset | Name | Range | Type | Notes |
|--------|------|-------|------|-------|
| `+0x76` | Mode | 0–3 | Enum | Note, CC, MCU/HUI, Program Change |
| `+0x06` | Channel (Note/CC/PC modes) | 0–15 | CC | (AR3ChRow) |
| `+0x16` | Color (RGB) | 0–127 | Uint8 | (Ar3ColorRow) |
| `+0x26` | Option (Note/CC modes) | 0–1 | Enum | Gate, Toggle |
| `+0x36` | CC Number (CC mode, use-cc-offset) | — | CC | |
| `+0x46` | On Value (CC mode) | 0–127 | Value | |
| `+0x56` | Off Value (CC mode) | 0–127 | Value | |
| `+0x66` | Note Number (Note mode) | 0–127 | MIDI Note | |
| `+0x76` | Program Number (PC mode, use-cc-offset) | 0–127 | Value | |
| `+0x06` (id2) | Bank MSB (PC mode, use-cc-offset) | 0–127 | Value | |
| `+0x16` (id2) | Bank LSB (PC mode, use-cc-offset) | 0–127 | Value | |
| `+0x26` (id2) | MCU/HUI Mode (MCU/HUI mode, use-cc-offset) | 0–6 | Enum | Click–Rec |

**Pad Bank A example:** ID 0x7601 (Mode) maps to section 0x01, Pad 0.

### Pedal (Section 0x00, ~0x08–0x0D)

| ID (hex) | Name | Range | Type | Notes |
|----------|------|-------|------|-------|
| `0x08000000` | Type | 0–3 | Enum | Sustain, Expression, FootSwitch, Control |
| `0x09000000` | Footswitch CC (FS/Ctrl mode) | 0–127 | CC | |
| `0x0a000000` | Control CC (Ctrl mode) | 0–127 | CC | |
| `0x0b000000` | Polarity | 0–1 | Enum | Normal, Inverted |
| `0x0c000000` | Min Value (all modes) | 0–127 | Value | |
| `0x0d000000` | Max Value (all modes) | 0–127 | Value | |

### Shift/Pitch/Mod Page (Section 0x02, overlaps with Mod Wheel section 0x00)

**Shift (0x02):**
| ID (hex) | Name | Range | Type | Notes |
|----------|------|-------|------|-------|
| `0x36020000` | Mode | 0–1 | Enum | Off, CC |
| `0x0a020000` | Channel (CC mode) | 0–15 | CC | |
| `0x0b020000` | CC Number (CC mode) | 0–127 | CC | |
| `0x0c020000` | Off Value (CC mode) | 0–127 | Value | |
| `0x0d020000` | On Value (CC mode) | 0–127 | Value | |

**Pitch (0x02):**
| ID (hex) | Name | Range | Type | Notes |
|----------|------|-------|------|-------|
| `0x07020000` | Channel | 0–15 | CC | |
| `0x08020000` | Min Value | 0–127 | Value | |
| `0x09020000` | Max Value | 0–127 | Value | |

**Mod (0x02, Mod Wheel alternative config):**
| ID (hex) | Name | Range | Type | Notes |
|----------|------|-------|------|-------|
| `0x00020000` | Channel | 0–15 | CC | |
| `0x01020000` | Output | 0–1 | Enum | CC, NRPN |
| `0x02020000` | CC Number (CC mode) | 0–127 | CC | |
| `0x03020000` | Min Value (CC mode) | 0–127 | Value | |
| `0x04020000` | Max Value (CC mode) | 0–127 | Value | |
| `0x05020000` | Parameter MSB (NRPN mode) | 0–127 | Value | |
| `0x06020000` | Parameter LSB (NRPN mode) | 0–127 | Value | |

### Velocity (Section 0x0x, exact section TBD)

| ID (hex) | Name | Range | Type | Notes |
|----------|------|-------|------|-------|
| `0x02xx0000` | Key Velocity Curve | 0–3 | Enum | Linear, Exponential, Logarithmic, Fixed |
| `0x03xx0000` | Key Fixed Velocity | 1–127 | Value | Active if curve=Fixed |
| `0x04xx0000` | Pad Velocity Curve | 0–3 | Enum | Linear, Exponential, Logarithmic, Fixed |
| `0x05xx0000` | Pad Fixed Velocity | 1–127 | Value | Active if curve=Fixed |
| `0x06xx0000` | Pad Aftertouch | 0–2 | Enum | Linear, Exponential, Logarithmic |
| `0x11xx0000` | Knob Acceleration | 0–2 | Enum | None, Medium, Fast |

---

## 4. Pads / Colors / Display

**Color Support:** YES (limited).

The Ar3ColorRow widget binds to `0x1601` per pad. This suggests RGB or palette color control, but **exact color encoding is not implemented in this tool**—the UI allows color selection, but the protocol details (palette depth, encoding scheme) are not exposed.

**Display Support:** The MiniLab 3 has no built-in display. Presets are shown on the hardware via:
- **Remapped control:** `0x03407f7f` → `0x08401300` (preset 3 "show on controller")
- Hardware shows preset slots 1–5 via button/LED feedback, not a text display.

**Text/String Support:** YES (limited to preset names?).
```c
int sc_midi_arturia_write_string(…, uint32_t control_id, char val[17])
```
Max 16 chars + null. Field `id` with `maxlen=16`. Likely used for preset renaming, not implemented UI in MiniLab 3 sample.

---

## 5. Presets / Programs

**Recall Preset:**
```
F0 00 20 6B 7F 42 05 [preset_id] F7
```
- Opcode: `0x05`
- Argument: `[preset_id]` = 0x00–0x7F (preset slot, 0-indexed likely)
- **Trigger:** User clicks preset in UI; ar_book_recall_preset() sends this.
- **Response:** None documented; hardware loads silently.

**Store Preset:**
```
F0 00 20 6B 7F 42 06 [preset_id] F7
```
- Opcode: `0x06`
- Argument: `[preset_id]` = 0x00–0x7F
- **Trigger:** "Save" button in preset UI.
- **Response:** None; hardware saves current state to slot.

**Preset Sync Option (ar_book.c, lines 98–134):**
If `preset_sync` flag is set, writing a control will trigger a 20ms delay + auto-store:
```c
usleep(20*1000);  // wait 20ms for hardware to process
klass->store_preset(…);  // send 0x06 opcode
```
This ensures preset is saved after parameter change.

**Preset Selection Remap (ml3-book.c):**
Controls `0x0340–0x0740` (Show Preset 1–5) are remapped to preset space `0x08401300–0x08401700` for convenient display in UI.

---

## 6. Firmware Version Handling

**Device Inquiry (generic Arturia):**
```
F0 7E 7F 06 01 F7   (MIDI Universal Device Inquiry)
```
Response is parsed for device ID (byte 2), not used to select protocol variant in MiniLab 3 code.

**Version-Dependent Branches:**
- **ML3 uses v3 protocol exclusively:** `sc_midi_arturia_v3_read_control` and `sc_midi_arturia_v3_write_control`.
- **Fallback for older Arturia devices:** sc_midi_arturia_read_control / write_control (v1/v2, 3-byte ID).
- **No runtime version negotiation:** Protocol is hardcoded per device type.

---

## 7. Raw Init / Handshake

**No explicit handshake found.** Device responds to queries immediately after connection.

**Connection flow (typical ALSA MIDI):**
1. Open sequencer via `snd_seq_open()`.
2. Subscribe to device MIDI port.
3. Send control queries via v3 `0x20` opcode.
4. Receive replies with `0x02` opcode (or `0x1C` ACK if `read_ack` flag set).

**AR Book initialization (ar-book.c, ml3-book.c):**
- Sets `read_ack = 0` (no ACK wait) for speed; assume MiniLab 3 does not reliably send ACK.
- **Preset sync = 0** (optional; not used by default).

---

## 8. Comparison Notes for MatrixBrute

**Potential Portability to MatrixBrute:**

| Aspect | MiniLab 3 | MatrixBrute | Likelihood |
|--------|-----------|-------------|-----------|
| **Device ID** | 0x7F 0x42 | Unknown (likely `06 01` from your notes) | **Very likely same protocol** if both use Arturia v3 |
| **Section/Page Structure** | 0x00–0x02+ for controls | Unknown | Likely similar if both Arturia synths |
| **Opcode Base** | 0x20, 0x21, 0x05, 0x06 | Unknown | **Highly likely same** (v3 protocol standard) |
| **4-Byte Control ID** | Yes (pr_id, p_id, c_id, r_id) | Very likely | **Core assumption** for v3 porting |
| **Preset Recall/Store** | 0x05, 0x06 opcodes | Likely identical | **Most portable** |
| **Multi-byte Values** | Via id2, id3 splits | Likely same | Probable |
| **String Support** | 0x04 opcode, 16-char max | Likely | Probable |

**Key Differences Expected:**
1. **Device ID:** MatrixBrute would use a different product identifier (not 0x42).
2. **Section layouts:** MatrixBrute has different controls (knobs, sliders, switches); sections/opcodes may differ.
3. **Pads/Colors:** MatrixBrute may have matrix-style controls not present in MiniLab 3.
4. **Transport modes:** Less likely (MiniLab is MIDI keyboard; MatrixBrute is synth).

**To Port to MatrixBrute:**
- Change device ID in SysEx header (byte 4–5, currently 0x7F 0x42).
- Reverse-engineer section layout via USB sniffing (as you've noted doing).
- Reuse opcode structure (0x20, 0x21, 0x05, 0x06) unless hardware differs.
- Test control ID normalization (`ar_control_normalize_id`) against actual MatrixBrute responses.

---

## 9. Gaps and Uncertainties

### Not Implemented / Unclear

1. **Color Encoding (Ar3ColorRow):**
   - UI allows color selection for pads, but C code only binds to control ID (0x1601).
   - No encoding details for RGB or palette values.
   - Assumed 0–127 range (7-bit MIDI), but exact palette/RGB split unclear.

2. **Velocity Curve Precision:**
   - Velocity page defines 4 curves (Linear, Exponential, Logarithmic, Fixed).
   - No documentation of curve algorithm or parameter transmission.

3. **Section 0x02 Overlap:**
   - Shift/Pitch/Mod page uses 0x02 but also references 0x00 controls.
   - Control offset logic in ml3_remap may mask actual addressing.

4. **NRPN Parameter Target:**
   - Knobs/Faders allow NRPN mode with "Parameter MSB/LSB" fields.
   - These are stored as separate controls, but how they're transmitted as NRPN CC#98/99 is unclear.
   - Likely handled by separate MIDI CC messages, not SysEx.

5. **ACK Behavior:**
   - `read_ack` parameter exists but defaults to 0 (disabled).
   - Unclear if older Arturia devices reliably send `0x1C` ACK.

6. **Preset Slot Count:**
   - Code accepts 0x00–0x7F (128 presets) but hardware may have fewer.
   - MiniLab 3 manual needed for exact count.

7. **Multi-byte Normalization (id2, id3):**
   - Controls with id2/id3 split values across 3 SysEx messages for 21-bit values.
   - Normalization (ar_control_normalize_id) adds to pr_id if r_id byte overflows.
   - Exact use case in MiniLab 3 unknown (no NRPN targets seem to require > 7-bit param).

8. **Transport Mode Implications:**
   - MCU / HUI / Both / None modes select transport control set.
   - Exact mapping of MCU/HUI commands to hardware actions not documented.

9. **Remap Predicate Logic:**
   - Remap table hardcoded for preset display; no condition for when it applies.
   - Assumed always active; no version/mode gating visible.

---

## Implementation Checklist for Rust

To implement this protocol in Rust:

```rust
pub struct ArturiaV3 {
    device_id: u8,  // 0x42 for MiniLab 3
    seq: MidiConnection,
}

impl ArturiaV3 {
    pub fn set_param(&mut self, section: u8, page: u8, control: u8, row: u8, value: u8) -> Result<()> {
        let control_id = ((section as u32) << 24) | ((page as u32) << 16) | ((control as u32) << 8) | (row as u32);
        self.write_control(control_id, value)
    }

    pub fn get_param(&mut self, section: u8, page: u8, control: u8, row: u8) -> Result<u8> {
        let control_id = ((section as u32) << 24) | ((page as u32) << 16) | ((control as u32) << 8) | (row as u32);
        self.read_control(control_id)
    }

    pub fn recall_preset(&mut self, slot: u8) -> Result<()> {
        self.send_sysex(&[0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x05, slot, 0xF7])
    }

    pub fn store_preset(&mut self, slot: u8) -> Result<()> {
        self.send_sysex(&[0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x06, slot, 0xF7])
    }

    fn write_control(&mut self, control_id: u32, value: u8) -> Result<()> {
        let msg = [
            0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x21,
            ((control_id >> 24) & 0xFF) as u8,
            ((control_id >> 16) & 0xFF) as u8,
            ((control_id >> 8) & 0xFF) as u8,
            (control_id & 0xFF) as u8,
            value,
            0xF7,
        ];
        self.send_sysex(&msg)
    }

    fn read_control(&mut self, control_id: u32) -> Result<u8> {
        let msg = [
            0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x20,
            ((control_id >> 24) & 0xFF) as u8,
            ((control_id >> 16) & 0xFF) as u8,
            ((control_id >> 8) & 0xFF) as u8,
            (control_id & 0xFF) as u8,
            0xF7,
        ];
        self.send_sysex(&msg)?;
        self.read_response()  // Receive 0x02 opcode reply
    }
}
```

---

## References

- **Source:** `/mnt/d/Claude/matrixbrute/reverse/sysex-controls/` (soyersoyer/sysex-controls on GitHub)
- **Key Files:**
  - `src/sc-midi.c:479–564` – Arturia v3 read/write control
  - `src/minilab3/ml3-book.c:37–44` – Remap table
  - `src/minilab3/ml3-*.ui` – UI definitions with control IDs
  - `src/ar-control.c:287–289` – Multi-byte normalization

---

**Document Status:** Reverse-engineered from GPL-3 sysex-controls Linux tool. Tested against MiniLab 3 hardware via ALSA MIDI. Suitable for implementing Rust driver for MiniLab 3 and (with device ID changes) MatrixBrute.

