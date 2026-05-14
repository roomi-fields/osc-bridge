# MatrixBrute Binary Patch Format Specification

**Extracted from MBPV (MatrixBrute Patch Viewer) v0.5 decompiled Java sources**

---

## 1. File Container Format

### 1.1 Archive Types

The MatrixBrute supports three compressed file formats, all based on ZIP:

- **`.mbpz`** – Single patch file
- **`.mbbz`** – Single bank (16 patches + sequences)
- **`.mbprojz`** – Full project (multiple banks, up to 256 patches)

### 1.2 ZIP Structure

UnpackZip.java reads entries from a standard ZIP archive:

#### For `.mbpz` (single patch):
- Entry starting with `0_` → `.mbp` patch data
- Entry starting with `1_` → `.mbs` sequence data

#### For `.mbbz` or `.mbprojz` (banks/project):
- Files named `*.mbp` → patch data
- Files named `*.mbs` → sequence data
- Paths like `DEVICE/PATCH/<name>-A1.mbp` are preserved or remapped

All file entries are **text files** containing space-separated decimal values representing the patch data.

---

## 2. Overall Patch Data Layout

### 2.1 Serialization Format

Each patch is stored as a **text string** with a header followed by space-separated 7-bit-encoded integers:

```
22 serialization::archive 10 0 4 [VERSION] [INDEX] [NAME_LEN] 
[MCC_TYPE] [FIELD_11] [FIELD_12] [CHAR_LEN] [CHARACTERISTICS] 
[FIELD_15] [FIELD_16] [FORMAT_VERSION] [DATA_LEN] [PATCH_NAME] [7-BIT DATA...]
```

### 2.2 Header Fields

| Field | Example | Notes |
|-------|---------|-------|
| Magic | `22` | Constant |
| String | `serialization::archive` | Constant |
| Version1 | `10` | Constant |
| Field2 | `0` | Unknown |
| Field3 | `4` | Unknown |
| Version (patch struct) | `4` (FW2) or `3` (FW1) | Firmware version in patch |
| LengthOrIndex | (varies) | Unknown |
| NameLength | `5` (for "Acid1") | String length of patch name |
| MccType | `0-12` | Preset type (see Globals.java line 36) |
| Characteristic | `"Acid"` | One of 18 descriptors (Globals.java line 37) |
| CharLen | `4` (for "Acid") | String length of characteristic |
| Field15, Field16 | (varies) | Unknown |
| FormatVersion | `32` (FW2) or `16` (FW1) or `1` (empty) | Discriminates firmware |
| DataLength | `1600` (FW2) or `1536` (FW1 converted) | Expected decompressed length |

### 2.3 Patch Data Sections (FW2, 1600 bytes)

After decompression from 7-bit to 8-bit:

| Section | Byte Range | Length (bytes) | Contents |
|---------|-----------|----------------|----------|
| Panel Parameters | 0–1342 | 1343 | All knob/slider/button values |
| Sequencer Params | 269–287 | 19 | Swing, Gate, Tempo, Division, Mode, etc. |
| Matrix Values (FW2) | 0–447 | 448 | 16×28 modulation values (2 bytes each) |
| Matrix Used Bits (FW2) | 960–1015 | 56 | Bitmap: 16×28 = 448 bits = 56 bytes |
| Custom LFO 1 Data | 896–927 | 32 | 16 values × 2 bytes |
| Custom LFO 2 Data | 928–959 | 32 | 16 values × 2 bytes |
| Custom LFO Smooth | 1016–1019 | 4 | 2 bytes per LFO (16 smooth bits each) |
| Custom Destinations | 1340–1371 | 32 | 16 × 2-byte destination IDs |
| Metadata/Unused | 1372–1599 | 228 | (Reserved or unused in v0.5) |

**FW1 (1536 bytes):** Same structure but matrix only uses 16 columns (not 28), "used" flags in bytes 512–767 (2 bytes per button = 512 bytes total).

---

## 3. Parameter Map

Complete table of every `Param` enum entry. Byte offsets are **within the decompressed patch data**.

| Param Name | Offset | Length | DataType | Range | KnobType | Visible | Notes |
|---|---|---|---|---|---|---|---|
| **VCO1** |
| VCO1_FINE | 1128 | 2 | INT16 | -128…127 | ROT_CENTER | yes | VCO1 Fine Tuning |
| VCO1_SIN_SQR | 1202 | 2 | INT16 | 0…255 | ROTARY | yes | Sin/Sqr blend |
| VCO1_ULTRASAW | 1234 | 2 | INT16 | 0…255 | ROTARY | yes | Ultrasaw mode |
| VCO1_PULSE_WIDTH | 1232 | 2 | INT16 | 0…255 | ROTARY | yes | Pulse width |
| VCO1_METALIZER | 1158 | 2 | INT16 | 0…255 | ROTARY | yes | Metalizer |
| VCO1_COARSE | 1130 | 2 | INT16 | -48…48 semitones | ROT_LIGHT | yes | Coarse pitch |
| VCO1_SUB_LEVEL | 1240 | 2 | INT16 | 0…100% | ROTARY | yes | Sub oscillator |
| VCO1_SAW_LEVEL | 1236 | 2 | INT16 | 0…100% | ROTARY | yes | Saw level |
| VCO1_SQR_LEVEL | 1238 | 2 | INT16 | 0…100% | ROTARY | yes | Square level |
| VCO1_TRI_LEVEL | 1242 | 2 | INT16 | 0…100% | ROTARY | yes | Triangle level |
| **VCO2** |
| VCO2_FINE | 1132 | 2 | INT16 | -128…127 | ROT_CENTER | yes | Fine tuning |
| VCO2_SIN_SQR | 1204 | 2 | INT16 | 0…255 | ROTARY | yes | Sin/Sqr blend |
| VCO2_ULTRASAW | 1252 | 2 | INT16 | 0…255 | ROTARY | yes | Ultrasaw mode |
| VCO2_PULSE_WIDTH | 1250 | 2 | INT16 | 0…255 | ROTARY | yes | Pulse width |
| VCO2_METALIZER | 1160 | 2 | INT16 | 0…255 | ROTARY | yes | Metalizer |
| VCO2_COARSE | 1134 | 2 | INT16 | -48…48 semitones | ROT_LIGHT | yes | Coarse pitch |
| VCO2_SUB_LEVEL | 1246 | 2 | INT16 | 0…100% | ROTARY | yes | Sub oscillator |
| VCO2_SAW_LEVEL | 1254 | 2 | INT16 | 0…100% | ROTARY | yes | Saw level |
| VCO2_SQR_LEVEL | 1244 | 2 | INT16 | 0…100% | ROTARY | yes | Square level |
| VCO2_TRI_LEVEL | 1248 | 2 | INT16 | 0…100% | ROTARY | yes | Triangle level |
| **VCO3 / LFO3** |
| VCO3_COARSE | 1136 | 2 | INT16 | -48…48 semitones | ROT_LIGHT | yes | Coarse pitch |
| VCO3_LFO_DIV | 1080 | 1 | INT8 | 0–3 | LIGHT_HORI | yes | Divider: 0=16, 1=32, 2=64, 3=128 (ValueMaps.lfo3Div) |
| VCO3_LFO_WAVEFORM | 1258 | 1 | INT8 | 0–3 | LIGHT_HORI | yes | Waveform (sine/tri/sqr/saw, ValueMaps.vco3Wave) |
| VCO3_KBD_TRACK | 1138 | 1 | INT8 | 0–1 | BUTTON | yes | Keyboard tracking on/off (ValueMaps.offOn) |
| **Noise** |
| NOISE | 1256 | 1 | INT8 | 0–3 | LIGHT_HORI | yes | Noise type (white/pink/red/blue, ValueMaps.noiseType) |
| **Audio Mod** |
| AUDIOMOD_VCO1 | 1206 | 2 | INT16 | -127…127 | ROTARY | yes | VCO1 > VCO2 modulation |
| AUDIOMOD_VCO3_VCO2 | 1210 | 2 | INT16 | -127…127 | ROT_BIDIR | yes | VCO1 < VCO3 > VCO2 |
| AUDIOMOD_VCO3_VCF2 | 1208 | 2 | INT16 | -127…127 | ROT_BIDIR | yes | VCF1 < VCO3 > VCF2 |
| AUDIOMOD_NOISE_VCF1 | 1212 | 2 | INT16 | -127…127 | ROT_BIDIR | yes | VCO1 < Noise > VCF1 |
| AUDIOMOD_VCO3_VCO2_RIGHT | 1262 | 1 | INT8 | 0–1 | NONE | **no** | Hidden; internal state |
| AUDIOMOD_VCO3_VCO2_LEFT | 1270 | 1 | INT8 | 0–1 | NONE | **no** | Hidden; internal state |
| AUDIOMOD_VCO3_VCF_RIGHT | 1260 | 1 | INT8 | 0–1 | NONE | **no** | Hidden; internal state |
| AUDIOMOD_VCO3_VCF_LEFT | 1272 | 1 | INT8 | 0–1 | NONE | **no** | Hidden; internal state |
| AUDIOMOD_NOISE_VCF1_RIGHT | 1266 | 1 | INT8 | 0–1 | NONE | **no** | Hidden; internal state |
| AUDIOMOD_NOISE_VCF1_LEFT | 1274 | 1 | INT8 | 0–1 | NONE | **no** | Hidden; internal state |
| **VCO Sync** |
| VCO_SYNC | 1276 | 1 | INT8 | 0–1 | BUTTON | yes | VCO2 > VCO1 (on/off, ValueMaps.offOn) |
| **Mixer** |
| MIXER_VCO1 | 1094 | 2 | INT16 | 0…100% | ROTARY | yes | VCO1 Mix level |
| MIXER_VCO2 | 1096 | 2 | INT16 | 0…100% | ROTARY | yes | VCO2 Mix level |
| MIXER_VCO3 | 1098 | 2 | INT16 | 0…100% | ROTARY | yes | VCO3 Mix level |
| MIXER_NOISE | 1100 | 2 | INT16 | 0…100% | ROTARY | yes | Noise Mix level |
| MIXER_EXTERNAL | 1102 | 2 | INT16 | 0…100% | ROTARY | yes | Ext input Mix level |
| MIXER_VCO1_FILTER | 1104 | 1 | INT8 | 0–3 | MIX_FILTER | yes | VCO1 filter routing (none/steiner/ladder/both) |
| MIXER_VCO2_FILTER | 1106 | 1 | INT8 | 0–3 | MIX_FILTER | yes | VCO2 filter routing |
| MIXER_VCO3_FILTER | 1108 | 1 | INT8 | 0–3 | MIX_FILTER | yes | VCO3 filter routing |
| MIXER_NOISE_FILTER | 1110 | 1 | INT8 | 0–3 | MIX_FILTER | yes | Noise filter routing |
| MIXER_EXTERNAL_FILTER | 1112 | 1 | INT8 | 0–3 | MIX_FILTER | yes | Ext filter routing |
| **Steiner VCF (VCF1)** |
| STEINER_CUTOFF | 1126 | 2 | INT16 | 0…255 | ROTARY | yes | Cutoff frequency |
| STEINER_RESONANCE | 1198 | 2 | INT16 | 0…100% | ROTARY | yes | Resonance/Emphasis |
| STEINER_DRIVE | 1192 | 2 | INT16 | 0…100% | ROTARY | yes | Drive level |
| STEINER_BRUTEFACTOR | 1194 | 2 | INT16 | 0…100% | ROTARY | yes | Brute factor (distortion) |
| STEINER_ENV1AMT | 1200 | 2 | INT16 | -127…127 | ROT_CENTER | yes | Envelope 1 modulation |
| STEINER_OUT | 1230 | 2 | INT16 | 0…100% | ROTARY | yes | Steiner output level |
| STEINER_MODE | 1278 | 1 | INT8 | 0–3 | LIGHT_VERT | yes | Filter mode (LP/BP/HP/off, ValueMaps.steinerMode) |
| STEINER_SLOPE | 1196 | 1 | INT8 | 0–1 | LIGHT_VERT | yes | Filter slope (12dB=0, 24dB=1, ValueMaps.FilterSlope) |
| **Ladder VCF (VCF2)** |
| LADDER_CUTOFF | 1222 | 2 | INT16 | 0…255 | ROTARY | yes | Cutoff frequency |
| LADDER_RESONANCE | 1224 | 2 | INT16 | 0…100% | ROTARY | yes | Resonance/Q |
| LADDER_DRIVE | 1170 | 2 | INT16 | 0…100% | ROTARY | yes | Drive level |
| LADDER_BRUTEFACTOR | 1172 | 2 | INT16 | 0…100% | ROTARY | yes | Brute factor |
| LADDER_ENV1AMT | 1178 | 2 | INT16 | -127…127 | ROT_CENTER | yes | Envelope 1 modulation |
| LADDER_OUT | 1220 | 2 | INT16 | 0…100% | ROTARY | yes | Ladder output level |
| LADDER_MODE | 1174 | 1 | INT8 | 0–2 | LIGHT_VERT | yes | Filter mode (LP/BP/HP, ValueMaps.ladderMode) |
| LADDER_SLOPE | 1176 | 1 | INT8 | 0–1 | LIGHT_VERT | yes | Filter slope (12dB=0, 24dB=1) |
| **Filter Control** |
| FILTER_ROUTING | 1148 | 1 | INT8 | 0–1 | LIGHT_VERT | yes | Serial/Parallel (ValueMaps.FilterRouting) |
| FILTER_MASTER_CUTOFF | 1082 | 2 | INT16 | 0…255 | ROT_CENTER | yes | Master cutoff offset |
| **Envelope 1 (VCF)** |
| ENV1_VELO | 1162 | 2 | INT16 | 0…100% | SLIDER | yes | Velocity modulation |
| ENV1_ATTACK | 1032 | 2 | INT16 | 0…100% | SLIDER | yes | Attack time |
| ENV1_DECAY | 1034 | 2 | INT16 | 0…100% | SLIDER | yes | Decay time |
| ENV1_SUSTAIN | 1036 | 2 | INT16 | 0…100% | SLIDER | yes | Sustain level |
| ENV1_RELEASE | 1038 | 2 | INT16 | 0…100% | SLIDER | yes | Release time |
| **Envelope 2 (VCA)** |
| ENV2_VELO | 1164 | 2 | INT16 | 0…100% | SLIDER | yes | Velocity modulation |
| ENV2_ATTACK | 1040 | 2 | INT16 | 0…100% | SLIDER | yes | Attack time |
| ENV2_DECAY | 1042 | 2 | INT16 | 0…100% | SLIDER | yes | Decay time |
| ENV2_SUSTAIN | 1044 | 2 | INT16 | 0…100% | SLIDER | yes | Sustain level |
| ENV2_RELEASE | 1046 | 2 | INT16 | 0…100% | SLIDER | yes | Release time |
| **Envelope 3 (Free)** |
| ENV3_DELAY | 1056 | 2 | INT16 | 0…100% | SLIDER | yes | Delay before attack |
| ENV3_ATTACK | 1048 | 2 | INT16 | 0…100% | SLIDER | yes | Attack time |
| ENV3_DECAY | 1050 | 2 | INT16 | 0…100% | SLIDER | yes | Decay time |
| ENV3_SUSTAIN | 1052 | 2 | INT16 | 0…100% | SLIDER | yes | Sustain level |
| ENV3_RELEASE | 1054 | 2 | INT16 | 0…100% | SLIDER | yes | Release time |
| **LFO1** |
| LFO1_RATE | 1062 | 2 | INT16 | 0…100% (→ 0.01Hz–163Hz) | ROTARY | yes | Oscillation rate |
| LFO1_PHASE | 1068 | 2 | INT16 | -127…127 | ROT_CENTER | yes | Phase offset |
| LFO1_WAVE | 1064 | 1 | INT8 | 0–7 | LIGHT_HORI | yes | Waveform (sin/tri/sqr/revsaw/saw/s&h/rand/custom, ValueMaps.lfoWave) |
| LFO1_SEQSYNC | 1060 | 1 | INT8 | 0–1 | BUTTON | yes | Sequencer sync (on/off) |
| LFO1_RETRIG | 1066 | 1 | INT8 | 0–2 | LIGHT_HORI | yes | Retrigger mode (off/single/multi, ValueMaps.lfoRetrig) |
| **LFO2** |
| LFO2_RATE | 1072 | 2 | INT16 | 0…100% (→ 0.01Hz–163Hz) | ROTARY | yes | Oscillation rate |
| LFO2_DELAY | 1078 | 2 | INT16 | 0…100% | ROTARY | yes | Delay before start |
| LFO2_WAVE | 1074 | 1 | INT8 | 0–7 | LIGHT_HORI | yes | Waveform |
| LFO2_SEQSYNC | 1070 | 1 | INT8 | 0–1 | BUTTON | yes | Sequencer sync |
| LFO2_RETRIG | 1076 | 1 | INT8 | 0–2 | LIGHT_HORI | yes | Retrigger mode |
| **Effects** |
| EFFECT_DELAY | 1166 | 2 | INT16 | 0…100% | ROTARY | yes | Delay time |
| EFFECT_REGEN | 1214 | 2 | INT16 | 0…100% | ROTARY | yes | Regeneration / Feedback |
| EFFECT_TONE | 1216 | 2 | INT16 | 0…100% | ROTARY | yes | Tone / Rate |
| EFFECT_WIDTH | 1218 | 2 | INT16 | 0…100% | ROTARY | yes | Width / Depth |
| EFFECT_MIX | 1168 | 2 | INT16 | 0…100% (dry→wet) | ROTARY | yes | Dry/Wet mix |
| EFFECT_SYNC | 1058 | 1 | INT8 | 0–1 | BUTTON | yes | Tempo sync (on/off) |
| EFFECT_MODE | 1264 | 1 | INT8 | 0–4 | LIGHT_VERT | yes | Effect type (stereo delay/mono delay/chorus/flanger/reverb, ValueMaps.analogEffect) |
| **Glide** |
| GLIDE_AMOUNT | 1116 | 2 | INT16 | 0…100% | ROTARY | yes | Glide time |
| GLIDE_ON | 1118 | 1 | INT8 | 0–1 | BUTTON | yes | Glide on/off |
| **Pitch Wheel** |
| WHEEL_RANGE | 1322 | 2 | INT16 | 0…24 semitones | ROTARY | yes | Bender range |
| WHEEL_MOD | 1142 | 1 | INT8 | 0–3 | LIGHT_VERT | yes | Mod wheel target (matrix/cutoff/LFO1 vib/LFO1 amt, ValueMaps.modWheel) |
| **Voice Control** |
| VOICE_MODE | 1140 | 1 | INT8 | 0–2 | LIGHT_VERT | yes | Voice mode (monophonic/paraphonic/duo split, ValueMaps.vcoSync) |
| **Play Control** |
| PLAY_HOLD | 1120 | 1 | INT8 | 0–1 | BUTTON | yes | Key hold on/off |
| PLAY_PRIORITY | 1122 | 1 | INT8 | 0–2 | LIGHT_HORI | yes | Note priority (low/high/last, ValueMaps.notePriority) |
| PLAY_LEGATO | 1124 | 1 | INT8 | 0–2 | LIGHT_HORI | yes | Legato mode (on/off/glide, ValueMaps.Legato) |
| **Sequencer (stored in patch, but FW-dependent)** |
| SEQ_SEQUENCER_DIRECTION | 269 | 1 | INT8 | 0–3 | LIGHT_NONE | yes | Direction (fwd/rev/fwd-rev/rand) |
| SEQ_SEQUENCER_SWING | 270 | 1 | INT8 | 0…100% | ROTARY | yes | Swing amount |
| SEQ_SEQUENCER_GATE | 271 | 1 | INT8 | 0…100% | ROTARY | yes | Gate time |
| SEQ_SEQUENCER_DIVISION | 274 | 1 | INT8 | 0–3 | LIGHT_NONE | yes | Note division (1/4, 1/8, 1/16, 1/32) |
| SEQ_SEQUENCER_NOTEVAL | 275 | 1 | INT8 | 0–2 | LIGHT_NONE | yes | Note timing (straight/triplet/dotted) |
| SEQ_SEQUENCER_TEMPO | 283 | 2 | INT16 | 20…300 BPM | DISPLAY | yes | Tempo (stored as bpm × 100) |
| SEQ_SEQUENCER_BUTTON | 1316 | 1 | INT8 | ? | BUTTON | yes | Sequencer on/off |
| SEQ_ARPEGGIATOR_BUTTON | 1316 | 1 | INT8 | ? | BUTTON | yes | Arpeggiator on/off |
| **Macros** |
| MACRO_1 | 1180 | 2 | INT16 | 0…100% | MACRO | yes | User-assignable macro knob 1 |
| MACRO_2 | 1182 | 2 | INT16 | 0…100% | MACRO | yes | User-assignable macro knob 2 |
| MACRO_3 | 1184 | 2 | INT16 | 0…100% | MACRO | yes | User-assignable macro knob 3 |
| MACRO_4 | 1186 | 2 | INT16 | 0…100% | MACRO | yes | User-assignable macro knob 4 |

### 3.1 Data Type Interpretation

**INT8** (1 byte):
- Raw value from `bytes[offset]` (0–255 unsigned)

**INT16** (2 bytes):
- Stored as little-endian: `value = bytes[offset] + 256 × bytes[offset+1]`
- If value > 32767, convert to signed: `value -= 65536`
- **Scaled by `getScaledInt16()`**: divide by 256 → gives display value in range −0.5…0.5 or 0…1.0
- **Percent by `getPercent()`**: divide by 328 → gives 0…100%

**PCT** (2 bytes):
- Alias for INT16 with percent scaling

---

## 4. Modulation Matrix Encoding

The matrix is 16 sources (rows) × 28 destinations (columns, of which 16 are fixed + 12 custom = 28 total in FW2; only 16 in FW1).

Each cell stores:
- **Value** (modulation amount, 0…100%)
- **Used** (whether the routing is active, boolean)

### 4.1 FW2 (Firmware 2) Layout

**Matrix Values (bytes 0–447)**
- 16 rows × 28 columns = 448 values
- Each value: **2 bytes, little-endian INT16**
- Byte range: `0 + row*56 + col*2` to `0 + row*56 + col*2 + 1`
- Retrieval: `value = getPercent(bytes, row*56 + col*2)` → 0…100%

Example: Row 0 (Env1), Column 0 (VCO1 Pitch)
- Bytes: `0–1`
- To read: `bytes[0] + 256*bytes[1]`, divide by 328 → percentage

**Matrix Used Bits (bytes 960–1015)**
- 16 rows × 28 columns = 448 bits = 56 bytes
- Byte `960 + row*3 + (col / 8)`, bit `(col % 8)`
- Retrieval:
  ```
  byte_idx = 960 + row*3 + (col / 8);
  bit_pos = col % 8;
  is_active = (bytes[byte_idx] >> bit_pos) & 1;
  ```

To read cell [row, col]:
```
amount = getPercent(bytes, row*56 + col*2);        // 0–100%
byte_idx = 960 + row*3 + (col / 8);
bit_pos = col % 8;
is_used = (bytes[byte_idx] >> bit_pos) & 1;        // 0 or 1
```

### 4.2 FW1 (Firmware 1) Layout

**Matrix Values (bytes 0–447)**
- Same as FW2: 16 × 28 (but only first 16 columns used)
- Each value: 2 bytes, little-endian INT16

**Matrix Used Flags (bytes 512–767)**
- 16 rows × 16 columns = 256 flags
- Each flag: **2 bytes** (wasteful; only first byte used)
- Byte range: `512 + row*32 + col*2`
- Retrieval:
  ```
  is_active = bytes[512 + row*32 + col*2] & 1;
  ```

Columns 16–27 are **always zero** in FW1.

### 4.3 Fixed vs. Custom Destinations

**Fixed (12 destinations, indices 0–11):**
- 0: VCO1 Pitch
- 1: VCO1 Ultra (Ultrasaw)
- 2: VCO1 PW (Pulse Width)
- 3: VCO1 Metal (Metalizer)
- 4: VCO2 Pitch
- 5: VCO2 Ultra
- 6: VCO2 PW
- 7: VCO2 Metal
- 8: Steiner Cutoff
- 9: Ladder Cutoff
- 10: LFO1 Amount
- 11: VCA (Envelope 2)

**Custom (16 destinations, indices 12–27):**
- Stored as 16 × 2-byte IDs at bytes 1340–1371
- IDs map via `ValueMaps.customDestinations` (see below)

---

## 5. Sequencer Encoding

### 5.1 Sequence Data Structure

Sequence data is stored similarly to patch data: text with space-separated 7-bit values, decompressed to 1184 bytes.

Header: `22 serialization::archive 10 0 4 [LENGTH] 0 …` (LENGTH = 1184)

### 5.2 Per-Step Layout (bytes 13–269)

64 steps × 4 bytes each:

| Bytes | Field | Type | Notes |
|-------|-------|------|-------|
| +0 | Note | INT8 | 0–127 (MIDI note; 0 = off) |
| +1 | Bit Mask | INT8 | Flags (see below) |
| +2–3 | Mod Value | INT16 | Modulation value (via getPercent, 0…100%) |

**Bit Mask fields:**
- Bit 0: Note ON (1 = note active, 0 = rest/off)
- Bit 1: Accent (1 = accented)
- Bit 2: Tie (1 = tied to next step)
- Bit 3: Slide (1 = portamento to this note)
- Bit 4: Mod (1 = modulation data present)

### 5.3 Global Sequencer Parameters (bytes 0–12 + scattered)

| Offset | Length | Param | Type | Notes |
|--------|--------|-------|------|-------|
| 12 | 1 | Sequence Length | INT8 | Number of active steps (1–64) |
| 270 | 1 | Swing | INT8 | 0…100% |
| 271 | 1 | Gate | INT8 | 0…100% (gate time as % of step) |
| 273 | 1 | Seq/Arp Button | INT8 | 0–3 (off/on/off/on states) |
| 274 | 1 | Note Division | INT8 | 0–3 (1/4, 1/8, 1/16, 1/32) |
| 275 | 1 | Note Values | INT8 | 0–2 (straight, triplet, dotted) |
| 283 | 2 | Tempo | INT16 | BPM × 100 (e.g., 12000 = 120 BPM) |
| (varies) | 1 | Direction | INT8 | SEQ_SEQUENCER_DIRECTION (byte 269) |

---

## 6. Custom LFO Encoding

### 6.1 Layout

Two custom LFOs (LFO1 and LFO2 when waveform is set to "Custom"/7).

| Section | Byte Range | Bytes | Contents |
|---------|-----------|-------|----------|
| Custom LFO1 Values | 896–927 | 32 | 16 × 2-byte values |
| Custom LFO2 Values | 928–959 | 32 | 16 × 2-byte values |
| Custom LFO1 Smooth | 1016–1017 | 2 | 16 smooth bits |
| Custom LFO2 Smooth | 1018–1019 | 2 | 16 smooth bits |

### 6.2 Reading Custom LFO Data

For LFO `lfoNum` (0 or 1):

```java
int offset = 896 + lfoNum * 32;
int smooth_offset = 1016 + lfoNum * 2;

// Read smooth bits (16-bit mask, 1 bit per column)
int smooth1 = bytes[smooth_offset];
int smooth2 = bytes[smooth_offset + 1];
String smoothBits = int8ToBinString(smooth1) + int8ToBinString(smooth2);

// Read each of 16 columns
for (int col = 0; col < 16; col++) {
    int rawValue = bytes[offset + col*2] + 256 * bytes[offset + col*2 + 1];
    int scaledValue = rawValue / 256;  // −127…127
    boolean isSmooth = smoothBits.charAt(col) == '1';
    
    // Map scaled value to display
    int displayValue = customLfoValues.get(scaledValue);  // see ValueMaps
}
```

### 6.3 Value Map

Custom LFO values are discrete: {−127, −109, −91, −73, −54, −36, −18, 0, 18, 36, 54, 73, 91, 109, 127}

These 15 values map to column indices 0–14; value 16 (I/S, infinite/smooth) not stored in the map but indicated by smooth bit.

---

## 7. 7-Bit ↔ 8-Bit Codec

### 7.1 Algorithm (from Utils.ConvertStringOf7bitToIntOf8bit)

The patch and sequence data use a **7-bit packing scheme** to reduce file size:

**Encoding (8 bits → 7 bits + 1 control byte):**
- Pack 8 data bytes into 7 bytes + 1 control byte (8 bytes total)
- Control byte (first in sequence): Each bit represents the MSB (bit 7) of the corresponding data byte
- Next 7 bytes: The lower 7 bits of each data byte

**Java unpacking code:**

```java
public static int[] ConvertStringOf7bitToIntOf8bit(String[] data, int expectedNumOfBytes) {
    int numOf7bit = expectedNumOfBytes;
    int num8bitBytes = numOf7bit / 8;
    int num7bitBytes = numOf7bit / 8 * 7;
    int[] initBytes = new int[num7bitBytes];
    
    // Read the lower 7 bits (packed section)
    for (int i = 0; i < num7bitBytes; ++i) {
        int idx = 1 + i + i / 7;  // Skip control bytes
        initBytes[i] = Integer.parseInt(data[idx]);
    }
    
    // Apply MSBs from control bytes
    for (int i = 0; i < num8bitBytes; ++i) {
        int controlByte = Integer.parseInt(data[0 + i * 8]);
        for (int bit = 0; bit < 7; ++bit) {
            int bitMask = 1 << bit;
            if ((controlByte & (bitMask * 2)) != 0) {  // Check bit positions 1–7 of control byte
                initBytes[i * 7 + bit] += 128;
            }
        }
    }
    return initBytes;
}
```

**Decoding logic (text → binary):**
1. Text string is space-separated decimal integers
2. First integer of each group of 8: control byte (bitmap of MSBs)
3. Next 7 integers: lower 7 bits of data
4. Reconstruct each 8-bit value by placing MSB from control byte + 7 data bits

**Example:**
- Input (text): `"65 64 65 66 67 68 69 70 71"` (9 values)
  - Control: 65 (binary: 01000001 → MSBs for next 7 bytes)
  - Data: 64, 65, 66, 67, 68, 69, 70, 71
  - Result: `{64 (no MSB), 65|128, 66, 67, 128, 69, 70, 71}`

---

## 8. Value Maps (Enumerations)

### 8.1 Standard Boolean

| ID | Value |
|----|-------|
| 0 | Off |
| 1 | On |

### 8.2 LFO Waveforms

| ID | Full Name | Short | Shape |
|----|-----------|-------|-------|
| 0 | Sine | Sine | Sine wave |
| 1 | Triangular | Triangle | Triangle wave |
| 2 | Square | Square | Square wave |
| 3 | Reverse sawtooth | RevSaw | −50 to +50 |
| 4 | Sawtooth | Saw | +50 to −50 |
| 5 | Sample&hold | S&H | Random stepped |
| 6 | Random | Random | White noise |
| 7 | Custom | Cust | User-defined from bytes 896–959 |

### 8.3 VCO3 LFO Waveforms

| ID | Waveform |
|----|----------|
| 0 | Sine |
| 1 | Triangle |
| 2 | Square |
| 3 | Sawtooth |

### 8.4 VCO3 LFO Dividers (Clock Division)

| ID | Division |
|----|----------|
| 0 | /16 |
| 1 | /32 |
| 2 | /64 |
| 3 | /128 |

### 8.5 Mixer Filter Routing

| ID | Routing |
|----|---------|
| 0 | None (dry signal) |
| 1 | Steiner (VCF1) |
| 2 | Ladder (VCF2) |
| 3 | Both (serial: Steiner→Ladder) |

### 8.6 LFO Retrigger Modes

| ID | Mode |
|----|------|
| 0 | Off (free-running) |
| 1 | Single (retrigger once per note) |
| 2 | Multi (retrigger each gate) |

### 8.7 Filter Modes

| Filter Type | 0 | 1 | 2 | 3 (if applicable) |
|-------------|---|---|---|-------------------|
| **Steiner (VCF1)** | LP (Lowpass) | BP (Bandpass) | HP (Highpass) | Off (bypass) |
| **Ladder (VCF2)** | LP | BP | HP | (N/A) |

### 8.8 Filter Slope

| ID | Slope |
|----|-------|
| 0 | 12 dB/octave |
| 1 | 24 dB/octave |

### 8.9 Filter Routing

| ID | Routing |
|----|---------|
| 0 | Serial (Steiner → Ladder) |
| 1 | Parallel (mixed outputs) |

### 8.10 Noise Types

| ID | Type |
|----|------|
| 0 | White (flat spectrum) |
| 1 | Pink (−3 dB/octave) |
| 2 | Red (−6 dB/octave) |
| 3 | Blue (+3 dB/octave) |

### 8.11 Analog Effects

| ID | Effect |
|----|--------|
| 0 | Stereo Delay |
| 1 | Mono Delay |
| 2 | Chorus |
| 3 | Flanger |
| 4 | Reverb |

### 8.12 Voice Modes

| ID | Mode |
|----|------|
| 0 | MonoPh (Monophonic) |
| 1 | ParaPh (Paraphonic, 3 voices) |
| 2 | DuoSplit (Dual layer split) |

### 8.13 Note Priority

| ID | Priority |
|----|----------|
| 0 | Lowest |
| 1 | Highest |
| 2 | Latest |

### 8.14 Legato Mode

| ID | Mode |
|----|------|
| 0 | On (portamento between notes) |
| 1 | Off (new amplitude envelope per note) |
| 2 | Glide (always glide, no env retrigger) |

### 8.15 Mod Wheel Targets

| ID | Target |
|----|--------|
| 0 | Matrix (via row 7 = Mod Wheel) |
| 1 | Cutoff (master cutoff modulation) |
| 2 | LFO1 Vibrato (LFO1 pitch amount) |
| 3 | LFO1 Amount (LFO1 output scaling) |

### 8.16 Sequencer Direction

| ID | Direction |
|----|-----------|
| 0 | Forward |
| 1 | Reverse |
| 2 | Forward/Reverse (pendulum) |
| 3 | Random |

### 8.17 Sequencer Note Division

| ID | Division |
|----|----------|
| 0 | 1/4 (quarter note) |
| 1 | 1/8 (eighth note) |
| 2 | 1/16 (sixteenth) |
| 3 | 1/32 (thirty-second) |

### 8.18 Sequencer Note Values

| ID | Value |
|----|-------|
| 0 | Straight (sync to tempo) |
| 1 | Triplet (3 in time of 2) |
| 2 | Dotted (1.5×) |

### 8.19 Sequencer On/Off States

| ID | Sequencer | Arpeggiator |
|----|-----------|-------------|
| 0 | Off | Off |
| 1 | On | Off |
| 2 | Off | On |
| 3 | On | On |

### 8.20 Preset Categories (MCC Type)

| ID | Category |
|----|----------|
| 0 | Default |
| 1 | Bass |
| 2 | Brass |
| 3 | FM |
| 4 | Guitar |
| 5 | Keys |
| 6 | Lead |
| 7 | Organ |
| 8 | Pad |
| 9 | Percussive |
| 10 | SFX |
| 11 | Sequence |
| 12 | Strings |

### 8.21 Preset Characteristics

| ID | Characteristic |
|----|-----------------|
| 0 | Acid |
| 1 | Aggressive |
| 2 | Ambient |
| 3 | Bizarre |
| 4 | Bright |
| 5 | Complex |
| 6 | Dark |
| 7 | Digital |
| 8 | Ensemble |
| 9 | Funky |
| 10 | Hard |
| 11 | Long |
| 12 | Noise |
| 13 | Quiet |
| 14 | Short |
| 15 | Simple |
| 16 | Soft |
| 17 | Soundtrack |

### 8.22 Custom Destination IDs (Partial List)

Custom destinations can be routed to the following parameter IDs:

| ID | Destination |
|----|-------------|
| −1 | Not used (old format) |
| 516–519 | ENV1 ADSR |
| 520–523 | ENV2 ADSR |
| 524–528 | ENV3 ADSRD |
| 531 | LFO1 Rate |
| 534 | LFO1 Phase |
| 536 | LFO2 Rate |
| 539 | LFO2 Delay |
| 541 | Master Cutoff |
| 547–551 | Mixer levels (VCO1, VCO2, VCO3, Noise, Ext) |
| 558 | Glide Amount |
| 563 | VCF1 Cutoff |
| 564–568 | VCO Fine/Coarse tuning |
| 579–580 | VCO1/2 Metalizer |
| 581–582 | ENV Velocity |
| 583 | FX Delay Time |
| 584 | FX Dry/Wet |
| 585–589 | VCF2 Drive, Brute Factor, Env Amt |
| 590–593 | Macro 1–4 |
| 596–600 | VCF1 Drive, Brute Factor, Resonance, Env Amt |
| 601–606 | VCO1/2 Sin/Sqr, Audio Mod |
| 607–609 | FX Regen, Tone, Width |
| 610–627 | VCF2 outputs, VCO levels, Pulse Width, Ultrasaw, etc. |

---

## 9. Constants and Magic Numbers

### 9.1 Header Magic

| Constant | Value | Purpose |
|----------|-------|---------|
| `"22"` | Literal | Serialization archive ID |
| `"serialization::archive"` | Literal | Format identifier |
| `10` | Literal | Archive version |

### 9.2 Patch Structure Versions

| Param | FW1 | FW2 |
|-------|-----|-----|
| Version (internal) | 3 | 4 |
| Format Version (header) | 16 | 32 |
| Expected Data Length | 1536 | 1600 |
| Matrix Columns | 16 (+ 12 unused) | 28 |
| Matrix Used Bytes | 512–767 (2 bytes/flag) | 960–1015 (bit-packed) |

### 9.3 Data Length Constants

| Section | Length (bytes) |
|---------|---|
| Patch data (FW2) | 1600 |
| Patch data (FW1) | 1536 |
| Sequence data | 1184 |
| Empty patch | `"22 serialization::archive 10 0 4 4 1600 9 --Empty-- 1 0 0 18 000000000000000000 1 0 1 0"` |
| Empty sequence | `"22 serialization::archive 10 0 4 1 0"` |

### 9.4 Patch Update from FW1 to FW2

When loading FW1 patches (formatVersion = 16):
1. Set version to 3 internally
2. Append 64 zero bytes (PATCH_FIELDS_V3_TO_V4) to sequence data
3. Copy bytes from offset 1086 to 1340 (8 bytes = custom destinations initialization)

### 9.5 File Type Discriminators

| File Extension | Type |
|---|---|
| `.mbpz` | Single patch (1 patch + 1 sequence) |
| `.mbbz` | Single bank (16 patches + 16 sequences) |
| `.mbprojz` | Project (multiple banks, max 256 patches) |

---

## 10. Unknowns, Gaps, and TODOs

### 10.1 Explicitly Unknown Fields (from Patch.java)

- `headField1`, `headField2` – First two fields after "22" (always blank/empty)
- `lengthOrIndex` – 9th field in header; purpose unclear
- `headField11`, `headField12` – Fields after name; purpose unclear
- `headField15`, `headField16` – Fields before format version; purpose unclear
- Bytes 1372–1599 (228 bytes) – Marked as reserved/unused in v0.5; may be FW3 features

### 10.2 Ambiguous Areas

- **[AMBIGUOUS]** Control byte algorithm in `ConvertStringOf7bitToIntOf8bit`: The bit masking check `(controlByte & (bitMask * 2))` is unconventional. Bit positions are shifted by ×2, which may indicate a historical artifact or undocumented encoding variant.

- **[AMBIGUOUS]** `AUDIOMOD_VCO3_VCO2_RIGHT`, `AUDIOMOD_VCO3_VCO2_LEFT` and similar: Marked as invisible (`visible=false`) in the Param enum. Likely intermediate state used during parameter UI interaction; offset and purpose unclear.

- **[AMBIGUOUS]** `SEQ_SEQUENCER_BUTTON` and `SEQ_ARPEGGIATOR_BUTTON` both at offset 1316. Conflict or overlay? Possibly the same byte stores both states in the lower 2 bits.

- **[AMBIGUOUS]** Expected Data Length field: Why 1600 for FW2 vs. 1536 for FW1? No clear documentation of the extra 64 bytes' purpose.

### 10.3 Hidden/Undocumented Parameters

- 6 parameters with `visible=false` in Enums.Param (the AUDIOMOD_*_LEFT/RIGHT variants)
- Likely used as intermediate state during preset load or UI state management; not normally displayed or edited

### 10.4 Matrix Destination Resolution

- Custom destinations reference indices in the 0–448 range in ValueMaps.customDestinations
- Indices 0–447 map to matrix [row, col] pairs: `idx = row*28 + col` (or similar)
- The exact reverse-lookup formula is not explicitly present in the decompiled code; inferred from usage

### 10.5 Firmware Version Detection Gaps

- Code checks `formatVersion == 16` (FW1) or `formatVersion == 32` (FW2) or `formatVersion == 1` (empty)
- No explicit FW3+ handling; unknown if device ever shipped with FW3
- Migration from V3→V4 copies 8 bytes but no description of what those bytes represent

### 10.6 Effect Mode Ambiguity

- EFFECT_MODE stored as INT8 at offset 1264
- ValueMaps lists 5 effect types (0–4)
- No indication of what values 5–255 mean (if allowed at all)

### 10.7 Sequencer Parameter Overlap

- Tempo, Swing, Gate, Direction stored in both **patch** data (at documented offsets) and **sequence** data
- Possible redundancy or override mechanism not documented

---

## 11. Rust Implementation Guide

A Rust implementation of `Patch::parse(&[u8])` would:

1. **Read header** (text parsing, skip to first space-separated data block)
2. **Extract format fields** to determine FW version
3. **Decompress 7-bit data** using `ConvertStringOf7bitToIntOf8bit` algorithm
4. **Parse 1600 or 1536 bytes** into structured sections
5. **Populate matrices, parameters, sequences**:
   - Use offset table (Section 3)
   - For INT16, read little-endian 2-byte pairs, divide by 256 or 328
   - For INT8, read single byte
6. **Decode matrix**:
   - For each [row, col], read value from offset 0 + row*56 + col*2
   - Read "used" bit from offset 960 + row*3 + (col/8), bit (col%8)
7. **Decode sequencer** (if present):
   - Parse 64 steps, each 4 bytes: note, bitmask, mod_value (INT16)
8. **Decode custom LFOs**:
   - Read 16×2-byte values + smooth bits

See Globals.PATCH_INIT and Globals.PATCH_EMPTY for default/empty templates.

---

## Appendix: File Format History

- **v0.3** (firmware 1.x): 1536-byte patch format, 16-column matrix with 512-byte "used" flags
- **v0.4**: Transitional; support both FW1 and FW2
- **v0.5** (current): Full FW2 support, 1600-byte patches, 28-column matrix, bit-packed "used" flags, custom destinations

---

**Document Generated:** 2026-04-12
**Source:** MBPV v0.5 decompiled Java (CFR 0.152)
**Status:** Complete reverse-engineering of patch format; sequencer and custom LFO encoding finalized.
