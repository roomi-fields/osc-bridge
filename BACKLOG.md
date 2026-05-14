# Backlog

Short-list of follow-ups. Move to an issue when picked up.

## Data hygiene
- [ ] Fix `infer_vendor()` in `scripts/import_electra_preset.py` — still misses `matrixbrute`, `equator2`, `hydrasynthv1`, etc. (had to be rebucketed manually). Broaden `KNOWN_VENDORS` + substring/token match with accent-stripping.
- [ ] Identify the 23 remaining `devices/electra-community/` entries (bacara, modnetic, eikon, rs_6, viper, vector_synth, ser_2020, destiny_model_q2, dl_8000r_v_1_1, division_department, the_runner, urbanspaceman_gfx, vidos, x_space, dekrispator, evadeluxe, exst1, flash_synth, gemini, polyexciter_13, revision_15_of_king_of_fm, vkeys_editor_mk2_25, biolux_fx_rack).
- [ ] Deduplicate the 15 `_sources[]` entries accumulated on `devices/arturia/matrixbrute.json` — the importer adds one per merge; cap / coalesce.
- [ ] Re-run the bulk Electra extraction with a higher `msgs >= N` threshold to see if 50 is the right floor (some 50-msg presets may be low-value WIP).

## Coverage
- [ ] Hardware-verification pass on MatrixBrute (current status: 🚧 working stub; library has JSON but untested on device).
- [ ] Layer SysEx on top of pencilresearch-imported devices that have known preset-management opcodes (Moog Sub 37, Prophet family, etc.).
- [ ] Mine `devices/electra-community/` remainders + the bulk Electra cache's rejected presets for SysEx-only coverage the catalogue lacks.

## Pipeline / infra
- [ ] CI job that runs `scripts/build_device_index.py` + `scripts/regen_supported_devices.py` and fails on diff (parity with the existing check).
- [x] `scripts/sync_sources.py` reads an optional `.local/sources.toml` for local source-declaration overrides.
- [ ] Strip/normalise author UIDs from `_sources[]` before publishing (privacy — some authors may not want their UID exposed even if they shared publicly).

## Runtime / Rust
- [ ] Integration tests for the Lua `transform` / `script` escape hatch on a real device (currently only the synthetic `scripted-example`).
- [ ] Benchmark: per-device rate-limiter allocation cost under N=32 orchestrated devices.
- [ ] `osc-bridge lint` to also warn on `_sources[]` entries without a `type` or `url`.

## Docs
- [ ] Write the "How I wrote my first device JSON in 30 min" tutorial — referenced by README but doesn't exist yet.
- [ ] Surface on the Pages site: author credit per device (data is there in `devices.json`, not displayed).

## Security
- [ ] Re-check GitHub's secret-scanning status weekly until the orphan blob is GC'd (direct-SHA URL still returns the key as of 2026-04-13).
