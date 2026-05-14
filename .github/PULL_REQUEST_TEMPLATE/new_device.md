<!--
Use this template for PRs that add a new device JSON.
GitHub will NOT pick it up automatically — open the PR with
?template=new_device.md in the URL, or paste this block manually.
-->

## New device: `<Vendor> <Device>`

**Status:** 📄 doc-derived / ✅ hardware-verified (delete one)

### Source

<!--
Cite the source. A URL is best; a commit of an external CSV or a pcap
attached to the PR also works. PRs without a cited source won't be merged.
-->

- Source URL / file:
- Firmware / document revision:

### Coverage

- OSC prefix: `/<prefix>`
- SysEx commands: <n>
- Params (SysEx table): <n>
- CC params: <n>
- Replies: <n>
- MIDI-in mapping: standard / customised (delete one)

### What I deliberately didn't model

<!-- list SysEx opcodes, params, or behaviours that are documented but
     not in this PR, and why (future scope, too exotic, unclear, …) -->

### Checklist

- [ ] `devices/<vendor>/<slug>.json` follows `docs/DEVICE_JSON_SCHEMA.md`
- [ ] `_source` field present and accurate
- [ ] `tests/<slug>.rs` with at least: load-success, bytes-of-one-command, all-commands-build
- [ ] `cargo test --release` passes (paste tail of output below)
- [ ] `cargo run --release -- lint devices/<vendor>/<slug>.json` — zero warnings (or warnings justified)
- [ ] Row added to `README.md` "Supported devices" table
- [ ] Section added to `docs/SUPPORTED_DEVICES.md`
- [ ] No schema extension **OR** schema extension split into a prior PR

### Test output

```
<paste cargo test tail>
```

### Notes

<!-- anything reviewer should know: naming choices, non-obvious bindings, etc. -->
