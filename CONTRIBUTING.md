# Contributing to osc-bridge

Thanks for wanting to help. This repository is the engine: the bridge, its
driver schema, the MCP server and the tools around them. The catalogue of
drivers lives on [SynthHub](https://synthhub.roomi-fields.com/), which hands
them out one at a time; drivers are contributed there, not here.

Ways to contribute:

1. **Fix a bug** in the engine or the runtime.
2. **Extend the schema** for a device class the declarative surface can't yet describe.
3. **Write a driver** for a device the catalogue lacks, or correct one, and send it to SynthHub (below).
4. **Test a driver on your hardware** and report it: that is how the catalogue gains credibility.

Code changes follow the usual workflow: fork → branch → commit → PR.

## General rules

- **One concern per PR.**
- **Run `cargo test --workspace --release` and `cargo clippy --workspace --all-targets -- -D warnings` before you submit.** PRs with failing tests won't be merged.
- **No `transform` / `script` unless absolutely needed.** The Lua escape hatch is documented in [`docs/scripting.md`](docs/scripting.md); `osc-bridge lint` warns on every use. Reach for it only when the declarative schema genuinely can't express the protocol.
- Commit style: imperative present, include a why. Co-author trailers welcome.

## Drivers

### Writing one

New to this? Start with [`docs/TUTORIAL_FIRST_DEVICE.md`](docs/TUTORIAL_FIRST_DEVICE.md)
— a 30-minute hands-on walkthrough that builds a working CC driver end to end.
The reference is [`docs/DEVICE_JSON_SCHEMA.md`](docs/DEVICE_JSON_SCHEMA.md).

- **Cite a source.** In decreasing order of preference: the vendor's programmer's reference / MIDI implementation; a canonical community table (e.g. [pencilresearch/midi](https://github.com/pencilresearch/midi)); your own reverse-engineering, with the captures. It goes in `_sources[]`.
- **One source, one file — no silent fusion.** A MIDI implementation changes between firmware versions, and community sources are often tied to a firmware they don't record. Fusing two sources into one JSON hides contradictions: two sources make two files. Pin the firmware in `_sources[].firmware` (`"unknown"` explicitly rather than omitting it).
- **File names:** `<vendor>/<device>.<source-tier>.fw-<version>.json` (e.g. `moog/subsequent-37.pencilresearch.fw-1.1.json`); its notes, if any, sit next to it with the same stem (`.md`). The catalogue id is that path without `.json`.
- **OSC names:** `/snake_case/section/param`, close to the vendor's terminology, without repeating the section (`/oscillator_1/octave`).
- **Check it:** `osc-bridge lint my-synth.json` (zero warnings unless you deliberately used `transform` / `script`), then `osc-bridge run --device my-synth.json` on the device.

### Sending it to SynthHub

- From an MCP client linked to SynthHub (`osc-bridge mcp`, or the hosted server): `submit_driver` with the file and its source. It is parsed, linted and tested, then added to the catalogue as an `agent-contributed` driver.
- Or open a [device request](https://github.com/roomi-fields/osc-bridge/issues/new?template=new_device_request.yml) here with the file attached.

### Correcting one, or confirming it on hardware

- `report_result` records how a driver behaved on your device, per firmware; `report_mapping` records which CC / NRPN a control really sends. When several users agree, the catalogue learns it, and `submit_fix` turns learned mappings into a corrected driver.
- A correction that needs more (a SysEx layer on top of a CC/NRPN table, a rename): send the corrected file with `submit_driver`, citing the source of the correction.

## Schema extensions

Schema extensions (new frame-token kinds, new arg types, new runtime behaviours) go through an open issue first. Post:

1. The device or class of devices that motivates the change.
2. A concrete example of what the JSON would look like.
3. Why the Lua escape hatch isn't enough.

Once aligned, the PR should add:

- The new Rust fields in `src/device.rs` (Option-wrapped, `#[serde(default)]`).
- Handling in `src/frame.rs` and / or `src/runtime.rs`.
- A test driver exercising the new capability under `tests/fixtures/devices/` (prefer a minimal synthetic one, e.g. under `acme/`), and its test.
- Updated docs in `docs/DEVICE_JSON_SCHEMA.md` and `CHANGELOG.md` under the next minor version.

## Local-only files — the `.local/` zone

`.local/` at the repo root is git-ignored. Use it for machine-local files
that aren't part of the project: scratch space, local config overrides,
experiment caches.

## Bug reports

If you observe wrong bytes going out or a JSON that refuses to load:

1. Run `osc-bridge inspect <driver>` (a file, or its catalogue id) and paste the output.
2. Attach a minimal OSC message that triggers the bug and the expected vs observed MIDI bytes.
3. Open an issue. Don't skip the "expected vs observed" part — half the bug reports we can action have it, the other half we can't.

## License and contributor agreement

You keep the copyright on your contributions. They are published here under the licence of the part they go into (AGPL-3.0-or-later for the code).

Before your first pull request is merged, you sign the [Contributor License Agreement](CLA.md), once: a bot comments on the pull request with the sentence to post, and records your signature. In short, the agreement lets the maintainer also use your contribution in other programs and hosted services built on osc-bridge, under any licence; [CLA.md](CLA.md) explains it in plain words.
