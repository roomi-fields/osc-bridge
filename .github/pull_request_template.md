<!--
Default PR template. For a new device, copy the checklist from
.github/PULL_REQUEST_TEMPLATE/new_device.md and replace this block.
For a hardware-verification promotion, use promote_to_verified.md.
-->

## Summary

<!-- one or two sentences: what and why -->

## Checklist

- [ ] `cargo test --release` passes locally
- [ ] `cargo run --release -- lint <any new device JSON>` has zero unexpected warnings
- [ ] No stray `transform` / `script` — or it's justified in the PR description
- [ ] If schema was extended: `docs/DEVICE_JSON_SCHEMA.md` and `CHANGELOG.md` updated
- [ ] If a device was added / promoted: `README.md` table and `docs/SUPPORTED_DEVICES.md` updated

## Notes for the reviewer

<!-- anything tricky, any design decision worth flagging -->
