# Contributing to osc-bridge

Thanks for wanting to help. There are five concrete ways to contribute:

1. **Add a new device JSON** (doc-derived, untested).
2. **Update an existing device JSON** — extend its SysEx layer after sniffing, correct a wrong CC number, rename routes, extend coverage. Especially important after a bulk import (e.g. pencilresearch) where the device starts as CC/NRPN only and grows SysEx over time.
3. **Promote a 📄 device to ✅** (you own the hardware and tested it).
4. **Fix a bug** in the engine, runtime, or an existing device JSON.
5. **Extend the schema** for a device class the declarative surface can't yet describe.

All five follow the same broad workflow: fork → branch → commit → PR. Each has its own PR template under `.github/PULL_REQUEST_TEMPLATE/` — GitHub will surface the right one when you use `?template=<name>.md` in the PR URL, or you can paste the template block manually.

## General rules

- **One concern per PR.** Don't bundle a new device with a schema change. If the device needs the schema change, open two PRs: schema first, device second.
- **Run `cargo test --release` and `cargo run --release -- lint <your-device.json>` before you submit.** PRs with failing tests won't be merged.
- **Be honest about coverage.** A device you haven't tested on real hardware is 📄, not ✅. This matters — users trust the table.
- **No `transform` / `script` unless absolutely needed.** The Lua escape hatch is documented in [`docs/scripting.md`](docs/scripting.md); `osc-bridge lint` warns on every use. Reach for it only when the declarative schema genuinely can't express the protocol.
- Commit style: imperative present, include a why. Co-author trailers welcome.

## One source, one file — no silent fusion

A MIDI implementation changes between firmware versions, and community sources
(Electra presets, pencilresearch CSVs) are often tied to a specific firmware
that the source itself doesn't always record. **Fusing two sources into one
JSON hides contradictions** — we tried it on Tempera and ended up with
duplicate CCs, decalé emitter ranges, and 193 unsectioned entries.

The rule going forward:

1. **One `_sources[]` entry per file.** If you have two sources for the same
   device, ship **two files**. Do not merge their CC tables, do not union their
   commands.
2. **Pin firmware when you can.** `_sources[].firmware` is required on every
   `vendor-doc` driver and strongly recommended everywhere else. Use
   `"unknown"` explicitly rather than omitting the field.
3. **File naming convention:** `<device>.<source-tier>.fw-<version>.json`.
   Examples:
   - `tempera.vendor.fw-2.2.json` — vendor doc, firmware 2.2
   - `tempera.electra-community.fw-unknown.json` — Electra community preset, firmware unknown
   - `subsequent-37.pencilresearch.fw-1.1.json` — pencilresearch CSV, firmware 1.1
   Source-tier tokens: `vendor`, `electra-busa` / `electra-community` / `electra-<author>`, `pencilresearch`, `hardware-verified`.

   **Companion docs (`.md`) follow the same slug.** A device JSON may ship with a
   prose companion (protocol deep-dive, SysEx notes, capture logs) next to it:
   `push-3.json` ↔ `push-3.md`. When you rename the JSON, rename the `.md` with
   the exact same stem — e.g. `push-3.vendor.fw-1.1.json` pairs with
   `push-3.vendor.fw-1.1.md`. If several variants share the same protocol notes,
   keep a single shared `.md` named after the most authoritative variant and
   cross-link the others from their `_limitations`.
4. **Catalogue groups variants automatically.** `scripts/regen_supported_devices.py`
   groups files that share `device.name` + `device.vendor` into one catalogue
   entry listing every variant with its firmware and tier.
5. **Prefer the user's firmware at runtime.** The bridge takes a `--device`
   path, so users pick the variant that matches their hardware. If you have a
   vendor-doc variant that clearly supersedes a community one (newer firmware,
   authoritative), note it in the community variant's `_limitations` so users
   know which to prefer.

## Adding a new device JSON

New to this? Start with [`docs/TUTORIAL_FIRST_DEVICE.md`](docs/TUTORIAL_FIRST_DEVICE.md)
— a 30-minute hands-on walkthrough that builds a working CC driver end to end.
The steps below are the reference checklist.

### Step 0 — is there a source?

We only merge device JSONs that have a **verifiable source**. In decreasing order of preference:

1. A vendor's published programmer's reference / MIDI implementation guide (Novation, Arturia, Electra One, Elektron, Sequential, Moog, Roland, Yamaha, Korg…).
2. A canonical community CSV, e.g. [pencilresearch/midi](https://github.com/pencilresearch/midi).
3. Your own hardware reverse-engineering (USBPcap + tshark on the vendor editor). Attach the pcap or at least the SysEx captures in the PR description so a reviewer can independently verify.

Pull requests without a cited source get a "source?" comment and stay open until one is provided.

### Step 1 — file & folder

- Path: `devices/<vendor>/<device-slug>.json` (e.g. `devices/moog/subsequent-37.json`).
- `vendor` is a kebab-case folder (`moog`, `arturia`, `expressive-e`, `tasty-chips`…).
- Use the existing JSON schema. Reference: [`docs/DEVICE_JSON_SCHEMA.md`](docs/DEVICE_JSON_SCHEMA.md).

### Step 2 — required fields

```jsonc
{
  "_source": "<one line: vendor doc URL / version / state>. NOT verified on hardware.",
  "device": {
    "name": "Your Device",
    "vendor": "Vendor",
    "revision": "firmware X.Y",
    "osc_prefix": "/yourdevice",
    "manufacturer_id": [...],
    "device_id": [...]
  },
  "sysex": { "header": [...], "footer": [247] },
  "commands":  [ /* … */ ],
  "params":    { /* or */ } ,
  "cc_params": { /* or */ },
  "midi_in":   { /* standard note/cc/pitchbend mapping */ },
  "replies":   [ /* … */ ]
}
```

The `_source` field is not used by the runtime but IS checked by reviewers. It's the single most important line in your JSON — don't skip it.

### Step 3 — OSC naming rules

- Use `/snake_case/sub_section/param` style.
- Don't duplicate the section in the param name (`/oscillator_1/octave`, **not** `/oscillator_1/oscillator_1_octave`).
- Keep names close to the vendor's terminology. If the vendor calls it *Fatness LFO*, emit `fatness_lfo`, not `fat_modulation`.

### Step 4 — tests

Every new device MUST ship with at least one test file, `tests/<device_slug>.rs`. Required tests:

1. Load succeeds: `Device::load(...).unwrap()`.
2. Sample a couple of commands and assert the exact bytes of the generated SysEx frame.
3. Loop over `device.commands` with synthetic bindings and verify every frame starts with `0xF0` and ends with `0xF7`.

Copy `tests/electra_one.rs` or `tests/launch_control_xl_3.rs` as a template.

### Step 5 — lint

```bash
cargo run --release -- lint devices/<vendor>/<slug>.json
```

Zero warnings expected unless you deliberately used `transform` / `script`.

### Step 6 — update the docs

- Add a one-line row in `README.md`'s compact table (vendor, device, status marker, entry count).
- Add a full section in `docs/SUPPORTED_DEVICES.md` (source URL, file path, coverage breakdown, notes).
- If your device needed a schema extension, add it to `docs/DEVICE_JSON_SCHEMA.md` in the same PR.

### Step 7 — PR

Title: `Add <Vendor> <Device> (doc-derived)` — or `(hardware-verified)` if you tested it.

In the description:

- Link to the source (doc URL or pcap artefact).
- Entry count (commands / params / replies).
- `cargo test --release` output.
- Any schema extensions, even tiny ones.
- Anything you deliberately didn't model and why (e.g. unsupported SysEx opcodes).

## Updating an existing device

Most updates fall into one of these buckets:

- **Adding SysEx on top of an imported CC/NRPN skeleton.** Bulk-imported devices (via `scripts/import_pencilresearch.py`) start as pure CC/NRPN. Once you sniff the vendor editor or read the official SysEx spec, you can extend the same JSON with a `sysex.header`/`footer` and a `commands` block. Keep the existing `cc_params` entries intact unless the vendor's SysEx actively conflicts with them.
- **Fixing a wrong CC / NRPN / range / orientation.** Happens especially when a doc says one thing and the device behaves differently. Always cite the source of the correction (hardware test log or alternative doc).
- **Renaming OSC routes for consistency.** Breaking change for any client already bound to the old path. Bump the device's `revision` field and flag the change loudly in the PR.
- **Extending coverage.** Parameters omitted by the original source but legitimate. Document the new source.

Use the `update_device.md` PR template. Required bits:

- Kind of change (SysEx layer / CC correction / rename / extension / schema / docs).
- **Source of the change** — same seriousness as `_source` on a new device. We don't merge corrections without a citable reason.
- Coverage delta (entry counts before/after).
- Explicit flag if any OSC route changed name or arg layout.

## Promoting 📄 → ✅ (hardware verification)

1. Plug the device in and run the bridge.
2. Exercise **every** declared command, param, and reply.
3. Update any bytes / addresses / OSC names that were wrong in the doc-derived version.
4. Record the test session (a text log is enough — list the OSC messages you sent and the MIDI you observed).
5. Open a PR titled `Promote <Device> to hardware-verified`, attach the log in the description, flip the marker in both `README.md` and `docs/SUPPORTED_DEVICES.md`.

These PRs are first-class — they're how the catalogue gains credibility.

## Schema extensions

Schema extensions (new frame-token kinds, new arg types, new runtime behaviours) go through an open issue first. Post:

1. The device or class of devices that motivates the change.
2. A concrete example of what the JSON would look like.
3. Why the Lua escape hatch isn't enough.

Once aligned, the PR should add:

- The new Rust fields in `src/device.rs` (Option-wrapped, `#[serde(default)]`, retro-compat).
- Handling in `src/frame.rs` and / or `src/runtime.rs`.
- At least one device exercising the new capability (can be a minimal example under `devices/examples/`).
- Updated docs in `docs/DEVICE_JSON_SCHEMA.md` and `CHANGELOG.md` under the next minor version.

## Local-only files — the `.local/` zone

`.local/` at the repo root is git-ignored. Use it for machine-local files
that aren't part of the project: scratch space, local config overrides,
experiment caches.

One supported hook: `scripts/sync_sources.py` reads an optional
`.local/sources.toml` and merges its `[[source]]` blocks with the committed
`sources.toml`. This lets you point the bulk-import tooling at a
local-only source declaration without editing the tracked config. Absent
file = normal case, nothing happens.

A note on `_sources[]` provenance: author *names* are kept as public credit;
internal account identifiers are not stored — the importer drops them.

## Bug reports

If you observe wrong bytes going out or a JSON that refuses to load:

1. Run `cargo run --release -- inspect <file.json>` and paste the output.
2. Attach a minimal OSC message that triggers the bug and the expected vs observed MIDI bytes.
3. Open an issue. Don't skip the "expected vs observed" part — half the bug reports we can action have it, the other half we can't.

## License

Contributions are accepted under GPL-3.0-or-later, same as the rest of the project. You retain copyright on your contributions.
