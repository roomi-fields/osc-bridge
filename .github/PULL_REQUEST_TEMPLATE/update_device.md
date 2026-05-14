<!--
Use this template when modifying an existing device JSON. Open the PR with
?template=update_device.md or paste this block manually.
-->

## Update `<Vendor> <Device>`

### Kind of update

<!-- Pick one or more -->

- [ ] Add SysEx layer on top of existing CC/NRPN surface
- [ ] Correct a wrong CC / NRPN number / range / orientation
- [ ] Extend coverage (parameters previously omitted now modelled)
- [ ] Rename OSC routes for consistency
- [ ] Schema extension required (link to prior PR)
- [ ] Documentation only (notes, section labels, `_usage` strings)

### Source for the change

<!--
Where does the new information come from?
- vendor SysEx spec PDF URL
- a USB sniff (attach pcap if possible)
- your own hardware testing log
- a community reverse-engineering repo
-->

### Coverage before / after

<!-- Paste output of `osc-bridge inspect` before and after, or just the entry count delta -->

Before: `<n commands, m cc_params, …>`
After:  `<n' commands, m' cc_params, …>`

### Breaking changes for OSC clients?

<!-- If an existing OSC path was renamed or its arg layout changed, say so
     explicitly here and bump the device's `revision` field in the JSON. -->

- [ ] No breaking OSC-surface change
- [ ] Breaking change, documented above

### Checklist

- [ ] `_source` field still accurate (or updated to cite both the original and the new material)
- [ ] `_limitations` field updated if limitations shrank or grew
- [ ] `cargo test --release` passes
- [ ] `osc-bridge lint devices/<vendor>/<slug>.json` — zero unexpected warnings
- [ ] `docs/SUPPORTED_DEVICES.md` section updated (coverage numbers, notes)
- [ ] `CHANGELOG.md` entry under the next minor / patch version
- [ ] If promoting 📄 → ✅ alongside this PR, include the hardware test log
