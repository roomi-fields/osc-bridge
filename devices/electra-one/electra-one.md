# Electra One MK2 — integration guide

Companion to `electra-one.json`. Loaded automatically by osc-bridge and served
via `/bridge/docs`. Targets both human integrators and LLM-driven clients.

OSC prefix for this device: **`/electra1`**
Device: Electra One MK2, firmware 4.x.

**Hardware-verified** against Electra One MK2 firmware 4.1.4 (serial EO2-4312746f,
hw revision 3.0) on 2026-04-14: upload round-trip, route registration, pot
mapping, SysEx ACK/NACK, introspection endpoints, and rendering of all control
types (fader, list, pad, adsr, adr, dx7envelope) all validated end-to-end.

---

## 1. Quick start — what Kanopi needs to do

1. **Upload a preset** → `/electra1/preset/upload <json_string>`
   - Reconfigures the device *and* rebuilds the bridge's semantic CC routes.
   - Single SysEx, no arm step. ~200 ms until the device is ready.
2. **Listen for semantic events** → `/electra1/page<N>/<control_name>` (float 0..1)
3. **Drive controls back** → send OSC on the same semantic addresses.
4. **Query state whenever** → see §4 introspection endpoints.
5. **Mutate a live preset without reuploading** → `/electra1/control/update`
   (name / color / visible) or `/electra1/lua` (arbitrary Lua, once a script is
   attached to the slot). See §7.

---

## 2. Preset JSON format (what you must generate)

Top-level:
```json
{
  "version": 2,
  "name": "MyPreset",
  "pages": [ { "id": 1, "name": "…" }, … up to 6 ],
  "devices": [ { "id": 1, "name": "…", "port": 1, "channel": 1 }, … ],
  "groups":   [ { "pageId": 1, "name": "…", "color": "white", "bounds": [x,y,w,h] }, … ],
  "controls": [
    {
      "id": 1, "pageId": 1, "name": "Cutoff", "type": "fader",
      "bounds": [x, y, w, h], "color": "orange",
      "values": [
        { "id": "value", "min": 0, "max": 127, "defaultValue": 64,
          "message": { "type": "cc7", "parameterNumber": 74, "deviceId": 1 }
        }
      ]
    }
  ]
}
```

### Control types (all hardware-validated)
`fader`, `vfader`, `list`, `pad`, `adsr`, `adr`, `dx7envelope`.

- `fader` is the default vertical bar.
- `list` requires an `overlays` entry (id-referenced from `values[].overlayId`)
  mapping numeric ranges to string labels (e.g. `{0→"SQR", 43→"TRI", 85→"SAW"}`).
- `pad` is a button; set `"mode": "toggle"` and the `message` block must include
  `onValue` / `offValue`.
- `adsr` / `adr` / `dx7envelope` are multi-point visual envelopes. See § 6 for
  the envelope-segment-editing gesture — it's not what you'd guess.

### Required field: `inputs`
**Every control must declare `"inputs": [{"potId": N, "valueId": "value"}]`.**
Hardware-verified: without this, the firmware falls back to a near-random pot
assignment (observed: a single pot driving multiple on-screen controls in lock-step).
`potId` is 1..12 matching KNOB1 (top-left) .. KNOB12 (bottom-right).

### Colours must be hex 6-char (no `#`)
Example: `"F49500"` (orange), `"529DEC"` (blue), `"03A598"` (teal), `"C44795"`
(pink), `"FFD232"` (yellow), `"F45C51"` (red), `"FFFFFF"` (white).
Named palette values like `"orange"` or `"blue"` are **silently rejected** —
the device accepts the upload (ACK) but fails to render properly and may
freeze.

### Message types
`cc7`, `cc14`, `nrpn`, `program`, `sysex`. **Only `cc7` currently gets a
semantic OSC route in the bridge** — the others render and emit MIDI but fall
back to `/electra1/cc/{num}` generic routing. Extending this is a one-line
change in the driver's upload script.

### Colors (palette)
`white`, `red`, `orange`, `blue`, `green`, `pink`, `purple`, `yellow`.

### Grid and control sets (MK2 hardware-verified)
Each **page** has up to **3 control sets**, switched by the 3 physical buttons
on the **left side** of the device (labeled SECTION 1 / 2 / 3) or by tapping
a control on the touchscreen (taps auto-activate that control's set).

Each **control set** can contain up to **12 controls**, one per physical pot
KNOB1..KNOB12. `controlSetId` on a control (1, 2, or 3) places it in a specific
set. Only one set is visually active at a time.

X/Y bounds are in pixel coordinates on the 1024×575 screen. Typical column
layout for 6 columns: `x = 14, 181, 348, 515, 682, 849` with width 158. Typical
two-row layout: `y = 40` (top) and `y = 320` (bottom).

### Upload targets the currently-selected slot
`/preset/upload` writes to whatever slot was last selected on the device
(either via a `/preset/switch` beforehand, or via the UI). **Always pair the
upload with `/preset/arm_upload <bank> <slot>` first** if you need to target
a specific slot — otherwise you will overwrite whatever preset happens to be
loaded, which can destroy user work.

---

## 3. Semantic address derivation (for reference — prefer introspection)

The bridge normalizes control names to addresses like:
```
/electra1/page<N>/<slug>
```
- `<N>` = **index** of the page in `pages[]` (1-based), *not* the page `id`.
- `<slug>` = `name:lower():gsub('[^%w]+', '_'):gsub('^_+',''):gsub('_+$','')`
  - `"Filter Cutoff"` → `filter_cutoff`
  - `"Osc 1 & FX"`    → `osc_1_fx`

**Recommended**: don't derive these yourself. Call `/electra1/routes/list`
after every upload and use the returned addresses verbatim.

Values on the wire are always **normalized float 0..1**, regardless of the
preset's declared `min`/`max`. Clients that need the scaled value compute
`min + norm * (max - min)` themselves.

Channel in the route = `device.channel - 1` (Electra is 1-indexed, MIDI 0-indexed).

---

## 4. Introspection endpoints (discovery)

| Send | Receive |
|---|---|
| `/electra1/routes/list` | N× `/electra1/routes/entry <addr> <ch> <cc> <page> <min> <max>` + `/electra1/routes/done` |
| `/electra1/preset/current` | `/electra1/preset/current <name>` |
| `/electra1/page/current` | `/electra1/page/current <n>` (or -1 if unset) |
| `/bridge/status` | `/bridge/status/device <slug> <status>` per device |
| `/bridge/docs` | `/bridge/docs/device <slug> <markdown>` (this file) |

After an upload, always poll `/routes/list` to learn the authoritative
address table — the slugification rule is a current implementation detail
that may evolve.

---

## 5. Spontaneous events (subscribe by default)

The bridge forwards device-initiated events as they arrive:

| OSC address | Meaning |
|---|---|
| `/electra1/ack <tx_lsb> <tx_msb>` | SysEx command acknowledged |
| `/electra1/nack <tx_lsb> <tx_msb>` | SysEx command rejected |
| `/electra1/page/switched <page>` | User changed page on the device. Bridge auto-updates `current_page`. |
| `/electra1/preset/switched <bank> <slot>` | Loaded preset slot changed |
| `/electra1/preset_bank/switched <bank>` | Bank changed |
| `/electra1/pot/touch <pot_id> <control_lsb> <control_msb> <touched>` | Touch-sensitive pot began/ended contact |
| `/electra1/preset_list/changed` | Presets list modified on device |
| `/electra1/snapshot_list/changed` | Snapshot library modified |

---

## 6. Workflow recipes

### Upload to a specific bank/slot
```
/electra1/preset/arm_upload <bank> <slot>   # "Set Preset Slot" (misnamed for history)
/electra1/preset/upload <json>              # writes JSON into the selected slot
/electra1/preset/switch <bank> <slot>       # optional: activate it for display
```
Wait for `/electra1/ack` before assuming success.

### Editing envelope segments (MK2 UI gesture)
`adsr` / `adr` / `dx7envelope` controls show **one pot binding at a time** on the
hardware, even when `inputs` declares multiple `{potId, valueId}` entries. To
cycle through segments:

- **Touch-and-hold the envelope's pot, then tap the envelope on the touchscreen.**
  The pot's assignment cycles to the next value (Attack → Decay → Sustain → Release → Attack…).
- **Or long-press the envelope control on screen** to open a detail window
  where all values are simultaneously editable.

If your client wants independent pot control over each segment, use separate
`fader` controls per segment instead of a visual envelope widget.

### Switch to an existing stored preset
```
/electra1/preset/switch <bank> <slot>
```

### Send raw SysEx (power users)
```
/electra1/raw/syx "F0 00 21 45 … F7"
```

---

## 7. Live mutation — changing a preset without reuploading

Two SysEx paths let a client mutate an already-loaded preset in-place. Both
are hardware-verified on MK2 firmware 4.1.4 (2026-04-18).

### 7.1 `/control/update` — declarative field mutation (opcode 0x14 0x07)

Signature: `/electra1/control/update <control_id:u14> <json_patch:string>`

The JSON patch is a subset of the control's fields; only the listed keys change.
Reverse-engineered from `jorisroling/bitwig-electra-one` (`Electra One.control.js`).

| Patch field | Effect | Status |
|---|---|---|
| `name` | Renames the on-screen label | ✅ works |
| `color` (hex 6-char string, no `#`) | Changes the control's color | ✅ works |
| `visible` (bool) | Shows / hides the control | ✅ works |
| `bounds` ([x,y,w,h]) | *Intended* to move/resize | ⚠️ ACK but **silently ignored** by firmware |
| `values[].defaultValue` | *Intended* to change default | ⚠️ ACK but **silently ignored** |
| invalid `control_id` | — | ⚠️ firmware ACKs instead of NACKing (robustness gap) |
| malformed JSON body | — | ✅ correctly NACKed |

Mutations persist across preset switch-away-and-back; they are stored slot-side.

### 7.2 `/lua` + `/lua_script/upload` — imperative runtime mutation

`/electra1/lua <lua_source:string>` (opcode 0x08 0x0D) executes Lua in the
context of the currently-loaded preset's Lua runtime.

**Precondition** (the trap): a preset uploaded via `/preset/upload` alone
has **no Lua runtime attached**. Calling `/lua` on it is a silent no-op
(globals `controls`, `info`, etc. are nil, errors go only to the logger).
You must first upload a Lua script to the same slot:

```
/electra1/preset/arm_upload <bank> <slot>
/electra1/lua_script/upload <lua_source>        # opcode 0x01 0x0C
/electra1/preset/switch <bank> <slot>           # force a fresh load so the runtime starts
```

After that, `/lua` starts ACKing and its side effects become visible.

**API surface validated on hardware** (2026-04-18, MK2 fw 4.1.4):

| Lua expression | Effect | Status |
|---|---|---|
| `controls.get(id):setName('NewLabel')` | Renames label | ✅ works |
| `controls.get(id):setColor(0xRRGGBB)` | Changes color — **must be an integer literal**, not a string | ✅ works |
| `controls.get(id):setColor('white')` / `'orange'` | String argument | ❌ silent no-op |
| `controls.get(id):setVisible(bool)` | (extrapolated from /control/update; not individually re-tested under /lua) | likely ✅ |
| `info.setText(s)`, `window.setInfoText(s)`, `preset.setInfoText(s)` | Change header text | ❌ no visible effect on any variant tried |
| `controls.add(...)` / `controls.create(...)` | Create a new control at runtime | ❌ no such API — firmware cannot instantiate new control objects post-upload |
| function definitions + later `/lua "myfn()"` | Persisted across /lua calls within the same runtime | ✅ works |
| `preset.onReady = function() ... end` | Callback fires on preset (re)load | ⚠️ unverified — `/preset/reload` NACKed in testing; other reload paths untried |

Mutations made via `/lua` **persist** across preset switch-away-and-back,
same as `/control/update`.

### 7.3 Strategic note for "live-dialogue" clients (Kanopi pattern)

To give a client full live-mutation power without preset reuploads:

1. At session start, upload the preset JSON **and** an accompanying Lua
   script that exposes the mutation functions you'll want to call (thin
   wrappers around `controls.get():setX()`).
2. For subsequent live changes, send `/lua "yourFn(args)"` — zero upload
   cost, <10 ms round-trip, persistent across slot switches.
3. **You cannot create new controls live.** If the session's control set
   changes, you must re-upload the preset (the 200 ms reconfig hit).

The thinnest possible mutation script — enough to unlock Kanopi-style
live dialogue — is ~1 KB of Lua.

---

## 8. Hardware constraints

- **Max preset size**: ~100 KB of JSON. Larger layouts must be split.
- **Reconfiguration latency**: ~200 ms between `/preset/upload` and the device
  being ready to report touches on the new layout. Not suitable for real-time
  morphing; fine for scene/section changes.
- **Storage**: 6 banks × 12 slots = 72 persistent preset slots in flash.
- **USB ports**: the MK2 exposes **3 virtual MIDI ports**. Port 1 ("Electra
  Controller") is the main one for **both** SysEx config AND control CC traffic
  — use this for osc-bridge. Port 2 is a MIDI passthrough for connected gear.
  Port 3 ("MIDIIN3/MIDIOUT3" on Windows, "CTRL" in Electra docs) is the
  Management Port and accepts SysEx only — CC messages sent here arrive at the
  device (USB LED blinks) but don't update on-screen values. **Bottom line for
  osc-bridge: use Port 1 (`--out-port <main_idx> --in-port <main_idx>`).**
- **Touch-sensitive pots** (12): report `/pot/touch` for begin/end contact
  events. Useful for "pick up" behavior to avoid value jumps when a knob is
  first touched.

---

## 9. Known limitations of the current driver

- Only `cc7` messages produce semantic routes. Extend the `/preset/upload`
  script in `electra-one.json` to handle `cc14` / `nrpn` / `program` / `sysex`.
- `/preset/arm_upload` is historically named after an operation it is not
  (it is actually Electra's "Set Preset Slot"). Renaming would break existing
  users; keep the name unless a major version bump is planned.
- Ack/Nack replies are surfaced as OSC events, but the bridge does not yet
  correlate them to the original upload request — client-side tracking of
  `tx_lsb`/`tx_msb` is currently the caller's responsibility.
- **Upload does not auto-switch the UI to preset view.** Firmware 4.1.4
  verified behaviour: after `/preset/upload`, the preset is loaded into the
  selected slot (confirmed via `/preset_list/get`) AND becomes the active
  preset for MIDI routing (`/preset/switched` event fires), BUT if the device
  was on the Menu / Home screen at upload time it stays there. The user must
  press the device's Home/Preset physical button to exit the menu and see the
  preset's controls. No documented SysEx command forces a UI screen change.
- **Midir / Windows truncates SysEx replies at 1024 bytes.** `/preset/get`
  on a preset larger than ~1KB returns a partial SysEx without `F7`
  terminator. The device's side is fine — it's the host-side MIDI driver
  buffer. Workarounds: fetch presets over USB mass-storage if the device
  supports it, or use Electra Console to export. (A possible osc-bridge fix:
  accumulate partial SysEx across multiple midir callbacks.)

---

## 10. For LLM-driven integrators

When writing new client code against this device:
1. Read this file first (`/bridge/docs` returns it).
2. Always call `/electra1/routes/list` after upload — do not derive addresses.
3. Assume value range is **always** normalized 0..1 on the OSC wire.
4. Treat `/electra1/ack` and `/electra1/nack` as upload outcome signals.
5. When generating a preset JSON, prefer existing template-mutation over
   building from scratch — the format has many cross-references (pageId,
   deviceId) that are easy to get subtly wrong.
