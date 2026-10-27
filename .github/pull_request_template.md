<!--
Pull requests here change the engine. Drivers live in the SynthHub catalogue:
send one with the submit_driver tool, or attach it to a device request issue
(see CONTRIBUTING.md).
-->

## Summary

<!-- one or two sentences: what and why -->

## Checklist

- [ ] `cargo test --release` passes locally
- [ ] `cargo run --release -- lint <any new test driver>` has zero unexpected warnings
- [ ] No stray `transform` / `script` — or it's justified in the PR description
- [ ] If schema was extended: `docs/DEVICE_JSON_SCHEMA.md` and `CHANGELOG.md` updated
- [ ] First contribution: I will sign the [Contributor License Agreement](../CLA.md) when the bot asks (once)

## Notes for the reviewer

<!-- anything tricky, any design decision worth flagging -->
