# Changelog

All notable changes to `osc-bridge`. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/). Version numbers follow
[Semantic Versioning](https://semver.org/): `MAJOR.MINOR.PATCH`.

Until `1.0.0` the schema and runtime are still stabilising — expect
additive minor bumps every time a new device class surfaces something
the JSON can't yet express. Backwards-compat of existing device JSONs
is tracked explicitly under each release.

## [Unreleased]

### Changed

- **Arturia MatrixBrute device JSON** promoted from "working stub" to
  hardware-verified-partial (fw 2.0.3.1411). All 78 CC params renamed
  under explicit hierarchical paths (`/vco1/fine`, `/vcf_steiner/cutoff`,
  `/env1/attack`, `/mixer/vco1_vol`, …) with section tags — duplicates
  (`/fine`, `/adsr`, `/output`, `/course`, `/rate`) disambiguated by
  their synth section. Added **39 global options** as a `params` block
  using `0x42` / `0x43` (`/option/midi_channel_in`, `/option/local_control`,
  `/option/aftertouch_curve`, …). Added **2 SysEx commands** for preset
  metadata/body read (`/preset/metadata/get`, `/preset/body/dump`) and
  2 reply patterns for state broadcasts. Run `osc-bridge inspect
  devices/arturia/matrixbrute.json` to see the full surface.

## [0.9.0] — 2026-04-14

Reconfigurable controllers become first-class citizens, driven by the
Electra One MK2 as the hardware-verified reference. The Lua scripting
subsystem grows from a narrow escape hatch (per-field transforms) into
a full device-driver substrate — persistent state, native JSON, dynamic
routing, multi-block SysEx emission, outbound OSC — all proven end-to-end
against real hardware (firmware 4.1.4).

### Added
- **`src/routing.rs`** — `DynamicRoutes` with page-aware CC↔OSC lookup,
  mutable at runtime under `Arc<Mutex<_>>`.
- **Scripting extensions** (`src/scripting.rs`):
  - Persistent per-device state (`ob.state`).
  - Native JSON codec (`ob.json_decode` / `ob.json_encode`).
  - Execution profiles (`Default` 10 ms / 1 MiB vs `PresetIngest` 500 ms / 4 MiB).
  - Dynamic route registration (`ob.register_cc_route`, `clear_routes`,
    `set_current_page`, `get_current_page`, `list_routes`).
  - Multi-block SysEx emission (`ob.emit_sysex`).
  - Outbound OSC emission (`ob.emit_osc`) for introspection.
  - `log_warn` / `log_error`.
- **Device schema**: `custom_commands` (scripted OSC endpoints with no
  declarative frame), auto-loaded companion `<stem>.md` served via
  `/bridge/docs`.
- **Runtime**: `try_dynamic_cc_in` / `try_dynamic_osc_out` helpers consulted
  ahead of the declarative pipeline; `reply.script` wired through with
  `ctx.bindings`; `/bridge/docs` endpoint.
- **Electra One MK2 driver** (`devices/electra-one/`):
  - `/preset/upload` custom_command — parses an Electra preset, rebuilds
    CC routes, sets current page, emits the SysEx data block (`0x01 0x01`)
    to the device.
  - `/routes/list`, `/preset/current`, `/page/current` introspection.
  - Reply script on `/page/switched` keeping `current_page` in sync.
  - `electra-one.md` integration guide served over `/bridge/docs` —
    hardware-verified against firmware 4.1.4.
- **CLI**: `osc-send --from-file <path>` for loading large string args
  (preset JSON) without shell quoting hell.
- **Tests**: `routing` (6), `dynamic_routing` (7), `custom_commands` (3),
  `electra_one_driver` (12), plus expanded `scripting` (27). Total 100.

### Hardware-verified
- **Electra One MK2** (firmware 4.1.4, serial EO2-4312746f, hw rev 3.0) —
  upload round-trip, route registration, pot mapping, SysEx ACK/NACK flow,
  all introspection endpoints, and rendering of all control types (fader,
  list, pad, adsr, adr, dx7envelope). Bumped from 📘 vendor-doc to ✅.

### Known caveats (documented in the companion guide)
- Midir/Windows truncates incoming SysEx at 1024 bytes, affecting
  `/preset/get` on large presets (the device side is fine).
- `inputs: [{potId, valueId}]` mandatory on every Electra control.
- Colors must be hex 6-char (no palette names).
- Upload targets the currently-selected slot — pair with `arm_upload`.
- Envelope segments cycle via a touch-and-hold gesture, not multi-pot
  binding.

## [0.8.0] — 2026-04-13

Massive library expansion via Electra One community presets — and a generic
multi-source ingest pipeline.

### Added
- `sources.toml` — central declaration of every bulk-import source the
  project knows about. Currently: `pencilresearch` (git) and
  `electra-one-public` (Firestore).
- `scripts/sync_sources.py` — generic driver that dispatches each source to
  the right module under `scripts/sources/` (git, firestore, …).
- `scripts/sources/firestore.py` — Firebase login + Firestore listing/fetch
  with predicate filtering and incremental skip.
- `scripts/import_electra_preset.py` — converts an Electra One preset
  (schemaVersion 2/3 — Editor "tiles" layout) into an osc-bridge device
  JSON. Handles cc7 / cc14 / nrpn / sysex inline message types. Auto-merges
  into existing devices when vendor + name match strictly; otherwise creates
  a new entry. SysEx common-prefix factored into the device's
  `sysex.header`.
- `scripts/enrich_electra_authors.py` — second pass that resolves each
  imported preset's `userId` → display name and writes it into
  `_sources[].author` for credit.
- New status marker **🎛️ electra-preset** in `regen_supported_devices.py`
  to distinguish community Electra presets from manufacturer docs.

### Imported
- **+464 new devices**, **+95 enriched** existing pencilresearch devices,
  from the 636 public Electra One presets. 71 skipped (Lua-only utilities
  with no MIDI bindings).
- Library size: 331 → **795 devices** across **117 vendors**.
- Notable additions: every Sequential Prophet (incl. Prophet-5/10 Rev4,
  Prophet 6, OB-6, OB-X8), MatrixBrute, Hydrasynth, Access Virus A/TI,
  Waldorf Iridium / Blofeld, Korg Wavestate, Elektron Digitone / Syntakt /
  Octatrack, Dreadbox Nymphes (+SysEx layer), and ~450 more.

### Schema
- `_sources[]` entries gain an optional `author` field when the source is
  `type: "electra-preset"`. Each Electra import cites the preset URL, ID,
  revision, and the author's display name (credit only — no internal
  account identifier is stored).

### Backwards compat
All previously-shipped devices continue to load. Existing pencilresearch
JSONs that gained an Electra layer keep their original `_sources[]` entry
(pencilresearch reference now appears alongside the Electra one).

## [0.7.0] — 2026-04-13

Mass device library: 331 devices across 102 vendors.

### Added
- Bulk import of [pencilresearch/midi](https://github.com/pencilresearch/midi) —
  326 new devices generated from the canonical community CSV collection
  (commit `deb41f2`). Moog (13), Roland (37), Elektron (16), DSI (9),
  Sequential (7), Yamaha (7), Waldorf (8), Novation (8), Behringer (6),
  Dreadbox (3), and ~80 more vendors. 7 empty-skeleton CSVs skipped.
- `scripts/import_pencilresearch.py` — single-CSV, bulk, and `--update`
  modes. Records the upstream commit SHA in `_sources[0].commit` so
  re-imports can diff against a precise baseline. Preserves custom
  extensions (SysEx layer, replies, custom commands) across re-imports.
- `scripts/regen_supported_devices.py` — rebuilds
  `docs/SUPPORTED_DEVICES.md` from every `devices/**/*.json`, grouped
  by vendor alphabetically. Single source of truth = the JSONs.
- Schema: `_source` (string) deprecated in favour of `_sources` (array
  of `{type, url, commit?, imported_at}`). Multiple sources can coexist
  (e.g. pencilresearch CC/NRPN baseline + vendor-doc SysEx layer). The
  legacy `_source` field is still read for backwards-compat.
- CI step: regenerates `SUPPORTED_DEVICES.md` and fails on diff —
  contributors must keep the catalogue in sync.
- GitHub topics added for discoverability:
  osc, midi, sysex, hardware-synth, moog, arturia, novation, sequential,
  korg, roland, yamaha, elektron, dave-smith-instruments, waldorf,
  dreadbox, erica-synths, polyend.
- README compacted: a 3-row summary instead of an unbounded table.
  Full catalogue lives in `docs/SUPPORTED_DEVICES.md`.

### Known limitations
Pencilresearch-imported devices are **CC/NRPN only**. SysEx (preset
management, display control, vendor-proprietary opcodes) is NOT
covered for those 326 devices — the `_limitations` field spells it out.
Extending a device with SysEx on top of the imported base is a
first-class contribution path; see `CONTRIBUTING.md` and the
`update_device.md` PR template.

## [0.6.0] — 2026-04-13

Kanopi integration: N/N bridge — performance MIDI + multi-client + multi-device.

### Added
- `midi_out` optional section on every device. When present, activates
  standard OSC performance routes automatically: `/note/on`, `/note/off`,
  `/pitchbend`, `/aftertouch`, `/poly_aftertouch`, `/cc/{num}`,
  `/program_change`. Supports `default_channel` and `note_offset`
  (drum-machine mappings).
- `/bridge/status` RPC endpoint — emits one `/bridge/status/device
  <slug> ok` OSC message per active device to every configured client.
  Lets Kanopi verify the setup at boot.
- `--osc-client` is now repeatable. Every listed client receives a copy
  of each outbound event. `RuntimeOptions.osc_client: Option<...>` →
  `RuntimeOptions.osc_clients: Vec<SocketAddr>`.
- New subcommand: `osc-bridge orchestrate --config bridge.toml`. Drives
  N devices in one process, dispatched by `osc_prefix`. Each entry
  can override the device's default `osc_prefix` (useful when two
  instances of the same model are wired up — e.g.
  `/matrixbrute-1` / `/matrixbrute-2`). Rate-limited per device, MIDI-in
  events emitted with the right prefix.
- `docs/CR-kanopi-gaps.md` — design rationale.
- `examples/bridge.toml` — sample orchestrator config.
- Added `toml = "0.8"` dependency.

### Backwards compat
All existing device JSONs load unchanged. Zero regression on prior tests
(24 → 42 passing). CLI breaking change on `--osc-client`: the flag is
now a `Vec`, so passing it once still works as before; passing it
multiple times is the new feature.

## [0.5.0] — 2026-04-13

Architecture: Lua scripting escape hatch.

### Added
- `mlua` (Lua 5.4 vendored) as an optional runtime dependency.
- `src/scripting.rs` — sandboxed `ScriptEngine` with 1 MiB memory cap,
  10 ms deadline via instruction hook, `ob.*` helper library
  (`u7_clamp`, `u14_lsb/msb`, `checksum_xor/sum`, `log`). Loaders,
  `os`, `io`, `debug`, `package`, `require` are stripped.
- Schema: optional `transform` / `transform_reverse` on `ParamEntry`
  and `CcParamEntry`; optional `script` on `Command` and
  `ReplyPattern` (the latter reserved).
- Runtime wiring: `transform` applied pre-clamp on CC + SysEx param
  paths; `script` run post-build on commands. Engine is lazy-init —
  devices without scripts pay zero overhead.
- `osc-bridge lint <device.json>` — validates schema, warns on every
  use of `transform` / `script` per the design CR.
- `devices/examples/scripted-example.json` with 3 integration tests.
- `docs/scripting.md` user guide.

### Backwards compat
All existing device JSONs load unchanged. 24 pre-existing tests green.

## [0.4.0] — 2026-04-13

Novation Launch Control XL 3 (doc-derived).

### Added
- `devices/novation/launch-control-xl-3.json` — 6 SysEx commands
  (DAW mode on/off, RGB LED per control, display configure/text/bitmap)
  + 69 CC entries (12 feature controls + 57 palette-index LED setters)
  + bitmap-ack reply. CC numbers cross-checked against Novation's
  published surface diagram PNG.
- `tests/launch_control_xl_3.rs` — 8 integration tests verifying
  SysEx bytes against the reference guide.
- README: Novation row added with source + entry count.

## [0.3.0] — 2026-04-13

Electra One MK2 (doc-derived).

### Added
- `devices/electra-one/electra-one.json` — 31 SysEx commands
  (query preset/runtime/config, preset/page/control-set switch,
  display text, Lua eval via `/lua`, reboot, destructive ops) +
  12 inbound events decoded via `replies` (ACK/NACK, switch events,
  pot touch, list-change notifications).
- `tests/electra_one.rs` — 9 integration tests.
- README: Electra One row + updated "Source" column distinguishing
  hardware-verified from doc-derived.

## [0.2.1] — 2026-04-13

Honesty pass: exhaustive doc-derived maps, explicit source tracking.

### Changed
- Polyend Synth: 110 → 195 CC entries (previously 85 silently
  skipped). Parsed via fixed-column layout from the official PDF.
  All 8 engines fully covered.
- Moog Subsequent 37: cleaner OSC naming (dropped redundant
  section prefix from param names).
- README: added explicit `Source` column (hardware-verified
  vs doc-derived) and per-device entry counts.

## [0.2.0] — 2026-04-13

Schema extension: 14-bit CC pairs + NRPN addressing.

### Added
- Moog Subsequent 37 (131 params: 89 CC, 98 NRPN, 31 as 14-bit CC
  pairs).
- `CcParamEntry` gains optional `cc_lsb`, `nrpn_msb`, `nrpn_lsb`
  fields. Runtime emits the correct MIDI sequence per entry:
  single CC, 14-bit CC pair, or full NRPN (CC 99/98/6 + CC 38
  when 14-bit).

### Backwards compat
Existing devices with single-CC entries continue to work unchanged;
the new fields are all `Option<u8>` with `#[serde(default)]`.

## [0.1.1] — 2026-04-13

### Added
- Polyend Synth device (110 CC-mapped params across 8 engines,
  program-change command). First CC-only device — `cc_params`
  section introduced in the schema.

## [0.1.0] — 2026-04-12

Initial public release.

### Added
- Skeleton declarative bridge: OSC ↔ MIDI/SysEx driven by
  `devices/<vendor>/<name>.json`.
- Hardware-verified Arturia MiniLab 3 (264 SysEx params + 6 commands
  + MIDI-in feedback, Ghidra static + empirical fuzz).
- CLI: `list`, `inspect`, `run`, `osc-send`, `osc-listen`.
- GPL-3.0 licence (inherits from the `sysex-controls` reverse
  engineering that seeded the MiniLab 3 parameter table).
