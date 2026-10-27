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
`fader`, `vfader`, `list`, `pad`, `adsr`, `adr`, `dx7envelope`, **`custom`**.

- `fader` is the default vertical bar.
- `list` requires an `overlays` entry (id-referenced from `values[].overlayId`)
  mapping numeric ranges to string labels (e.g. `{0→"SQR", 43→"TRI", 85→"SAW"}`).
- `pad` is a button; set `"mode": "toggle"` and the `message` block must include
  `onValue` / `offValue`.
- `adsr` / `adr` / `dx7envelope` are multi-point visual envelopes. See § 6 for
  the envelope-segment-editing gesture — it's not what you'd guess.
- **`custom`** is a Lua-painted tile (the canvas under your `setPaintCallback`).
  Required for any widget whose visual is implemented in Lua rather than as one
  of the firmware's built-in controls (step sequencers, custom meters, note-list
  editors, etc.). The `values[]` array still drives the parameter wiring, but
  the paint surface is entirely your code's responsibility.

**Custom-tile pot dispatch quirk (firmware 4.1.4)**: when `type:"custom"`, the
firmware dispatches pot events to the tile from **one pot only**, regardless of
how many entries the control's `inputs[]` array declares. Multi-pot custom
controls is forum thread [#4172](https://forum.electra.one/t/multi-pot-custom-control/4172)
— acknowledged as a requested feature, not yet shipped. Design custom widgets
around one-pot interaction (mode toggles, double-click variants, touchscreen
gestures) rather than multiple physical encoders per tile.

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

X/Y bounds are in pixel coordinates on the **1024×565 drawing area** (the
physical panel is 1024×575 but the firmware's slot-bounds math caps at 565 —
verified against `app.electra.one`'s `displayHeight` constant at offset
~129500 in `0c61af0.js`). Typical column layout for 6 columns:
`x = 14, 181, 348, 515, 682, 849` with width 158. Typical two-row layout:
`y = 40` (top) and `y = 320` (bottom). Full slot-bounds math for any slotId
is in §11 (Layout math reference).

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
| `/electra1/preset_list/changed` | Presets list modified on device (`7E 05`) |
| `/electra1/snapshot_list/changed` | Snapshot library modified (`7E 03`) |
| `/electra1/snapshot_bank/switched <bank>` | User changed snapshot bank (`7E 04`) |
| `/electra1/control_set/switched <set>` | User pressed SECTION button or tapped a tile of another set (`7E 07`) |
| `/electra1/capture_list/changed` | MIDI capture library modified (`7E 31`) |
| `/electra1/usb_host/changed` | USB Host port: a controller (re)connected on the device's USB-Host jack |
| `/electra1/log <text>` | `print()` output and runtime Lua errors from device (`7F 00`) — *not yet emitted by osc-bridge; the SysEx arrives but the driver doesn't surface it as OSC. Easy add: one entry in `replies[]`* |

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

## 7b. Schema gotcha — `tiles` vs `controls`

If you're generating preset JSON from a tool that uses the web-editor's
internal "tiles" schema (the format `app.electra.one` saves into Firestore and
that the [electraone-widgets](https://github.com/roomi-fields/electraone-widgets) repo
uses), you must convert it before uploading via SysEx — the firmware does not
parse it.

Symptom: `/preset/upload` ACKs at the transport level, the device fires
`7E 05` preset-list-change and `7E 02` preset-switch events as if everything
worked, but the screen shows "no name - page 1" or stays empty, and
`/preset/get` afterwards returns 0 bytes.

| Schema | Top-level keys | Where it lives |
|---|---|---|
| **`tiles` (web-editor internal)** | `schemaVersion`, `id`, `name`, `targetDevice`, `lua`, `devices`, `tiles[]`, `pages`, `categories`, `firstPageId` | `app.electra.one` Firestore + the electraone-widgets repo + `osc-bridge` `.cache/electra-import/` |
| **`controls` (device firmware)** | `version`, `name`, `projectId`, `pages`, `devices`, `overlays`, `groups`, `controls[]` | The official [Preset format spec](https://docs.electra.one/developers/presetformat.html) — what `/preset/upload` must send. |

The web editor converts tiles → controls in JS (`projectToPreset` function,
offset ~127000–131000 in bundle `0c61af0.js`) before calling its WebMIDI
`output.send()`. If your osc-bridge client is generating presets from
scratch following the `controls` schema described in §2, you're fine. If you're
pulling JSON from Firestore or from the electraone-widgets repo, run it
through the converter first.

A Python port of `projectToPreset` (byte-identical to the web editor, verified
against a live MIDI sniff) lives in the
[electra-one-mcp plugin](https://github.com/roomi-fields/electra-one-mcp) at
`server/preset_converter.py`. Translate to Rust as needed.

The same plugin also ports `presetToProject` (the reverse direction) so you
can pull a preset off the device and write it back in tiles form for
`git diff`-style workflows.

---

## 7c. File Transfer API (for very large files / multi-file deploys)

In addition to the simple uploads in §2 (`01 01 Upload Preset`, `01 0C Upload
Lua`, `01 0F` devices, `01 12` data, `01 11` performance), the device exposes
a transactional file-transfer API for large or multi-file deploys:

| Step | SysEx | Purpose |
|---|---|---|
| 1 | `F0 00 21 45 01 2D F7` | Open cache (begin transaction) |
| 2 | `F0 00 21 45 01 2E <fileId> <s0> <s1> <s2> <s3> F7` | Register a file + its size (4×7-bit LE) |
| 3 | `F0 00 21 45 01 2F <fileId> <data> F7` | Send chunk (one SysEx per chunk; data must stay 7-bit) |
| 4 | `F0 00 21 45 04 2D <commit-json> F7` | Commit + verify MD5 + move to final location |

`type:` values supported: `firmware`, `bootloader`, `preset`, `lua`,
`luaModule`, `ui`, `config`, `deviceList`, `datafile`, `performance`.
`location:` values: `slots`, `updates`, `assets`, `modules`, `presets`, `root`.

**Known limitation on firmware 4.1.4**: commit silently rolls back when
`type:"preset"` is in the commit JSON. The web editor never uses FT for
presets (only for `firmware`, `luaModule`, multi-file `slots` deploys with
Lua + Devices + Performance + Persisted at once). Stick with the simple
`01 01` + `01 0C` path for preset+lua and reserve FT for `luaModule`
uploads (the only way to write to `/ctrlv2/lua/<namespace>/<file>.lua`
without a host filesystem mount).

Reference: [filetransfer.html](https://docs.electra.one/developers/filetransfer.html) +
[forum #592](https://forum.electra.one/t/command-line-preset-file-upload-tool/592)
(staff posts #9, #16 on chunk sizes and reliability).

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
6. If you have a `tiles`-schema JSON (web editor's internal format), convert
   to `controls` schema BEFORE upload (see §7b). The firmware silently
   rejects tiles JSON.

---

## 11. Layout math reference (MK2)

Extracted from `app.electra.one`'s bundle `0c61af0.js` (offsets ~129500
defaults + ~159200 slot helpers + ~134272 reverse math). Verified against a
live MIDI sniff: `slotId=1, span=1, vspan=0` → `bounds=[181, 6, 158, 16]`,
`potId=8`, byte-identical to web editor output.

### Constants

```
mk2:  numPages=12, slotsPerPage=72, slotsPerRow=6, rowsOnPage=6
      slotsInSection=24, controlsInSection=12
      displayWidth=1024, displayHeight=565
      controlWidth=168, controlHeight=60
      groupWidth=168, groupHeight=30, groupSpanHeight=97
      maxVspan=6
      rowY=[0, 30, 90, 120, 180, 210, 270, 300, 360, 390, 450, 480]
```

The 6×12 grid alternates rows: even-y rows (y=0,2,4,…) are label/group rows
(h=30); odd-y rows are control rows (h=60). A "section" is one label + one
control row pair; the device has 6 sections per page.

### Forward (slotId → bounds + pot)

```python
def slot_to_bounds(slot, span=1, vspan=0):
    x = slot % 6
    y = (slot // 6) % 12
    if y % 2 == 0:                           # label/group row
        w = 146 * span + 21 * (span - 1) + 12
        h = (90 * vspan - 9) if vspan > 0 else 16
        px = 20 + 167 * x - 6
    else:                                    # control row
        w, h = 146, 56
        px = 20 + 167 * x
    py = 6 + 22 * ((y // 2) + (y % 2)) + 68 * (y // 2)
    return [px, py, w, h]

def slot_to_pot(slot):
    x = slot % 6
    return x + 1 if (slot // 6) % 4 == 1 else x + 7

def slot_to_page_id(slot):    return slot // 72 + 1
def slot_to_set(slot):        return ((slot // 6) % 12) // 4 + 1
```

### Reverse (bounds → slotId)

```python
def bounds_to_slot(bounds, page_id=1):
    col = bounds[0] // 170
    row_section = (bounds[1] - 6) // 90
    row_offset = 0 if (bounds[1] - 6) % 90 == 0 else 1
    return 72 * (page_id - 1) + 6 * (2 * row_section + row_offset) + col
```

For the Mini variant (4 cols × 6 rows, 24 slots/page, different constants) see
the bundle around offset 157783, or the matching `_MiniLayout` class in the
[electra-one-mcp plugin](https://github.com/roomi-fields/electra-one-mcp/blob/main/server/preset_converter.py).

---

## 12. Note on USB port routing

§8 recommends Port 1 (`Electra Controller`) as the osc-bridge target for both
SysEx config AND CC traffic. This holds for osc-bridge's mixed
config-plus-runtime workflow, but for SysEx-only admin work (preset upload,
file transfer, event subscription, executing Lua, querying state), the **CTRL
port** is the conventional target:

- **Port 1** — `Electra Controller` (no suffix). Carries CC/Note traffic to/from
  the active preset's MIDI device assignments. The bundle routes `app.electra.one`'s
  default MIDI here. Per-channel routing applies.
- **Port 2** — `MIDIOUT2 / MIDIIN2`. Same as Port 1 but for the second MIDI
  cable; pass-through to externally-connected gear via the device's MIDI
  passthrough.
- **Port 3 (CTRL)** — `MIDIOUT3 / MIDIIN3`. Admin / SysEx / events. The
  `app.electra.one` bundle's auto-selection regex prefers names matching
  `/Electra.*(CTRL|Port 3|MIDI 3)/i`. Every documented SysEx admin command in
  §2 + §6 + §7 works here. All `7E XX` unsolicited events come out of this
  port. CC sent here may light the device's USB LED but does not route to
  controls.

Practical guidance:
- **osc-bridge default**: Port 1 (the existing recommendation). Carries both
  CC and SysEx; the device's command interpreter accepts most SysEx admin
  on any port.
- **Pure SysEx admin / scripting clients** (preset push, Lua REPL, snapshot
  management, file transfer): Port 3 (CTRL) is the conventional target —
  guarantees no collision with active-preset CC traffic.

---

## 13. Cross-reference: complete SysEx catalog

For the full 62-command SysEx vocabulary (every host→device command + every
`7E XX` / `7F XX` event), see:

- `electra-one.json` in this folder — what osc-bridge wraps as OSC routes
- The [electra-one-mcp plugin](https://github.com/roomi-fields/electra-one-mcp)'s
  `docs/structured/sysex_commands.json` — same catalog with payload-shape
  metadata, queryable via its `get_sysex_command` MCP tool
- [Official protocol doc](https://docs.electra.one/developers/midiimplementation.html)
