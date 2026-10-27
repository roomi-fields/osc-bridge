# Backlog

Short-list of follow-ups. Move to an issue when picked up.

The catalogue's own follow-ups (imports, data hygiene, coverage) are kept
with the catalogue, on SynthHub.

## Runtime / Rust
- [ ] Integration tests for the Lua `transform` / `script` escape hatch on a real device (currently only the synthetic `scripted-example`).
- [ ] Benchmark: per-device rate-limiter allocation cost under N=32 orchestrated devices.
- [ ] `osc-bridge lint` to also warn on `_sources[]` entries without a `type` or `url`.

## Docs
- [x] "Your first device JSON in 30 minutes" tutorial — [`docs/TUTORIAL_FIRST_DEVICE.md`](docs/TUTORIAL_FIRST_DEVICE.md), linked from README and CONTRIBUTING.

## Security
- [x] Firebase web API key blob: resolved by the 2026-05-14 history squash —
  no ref on the remote reaches the old commit anymore. The key was
  public-by-design (it identifies the Firebase project, not a user); GitHub
  may keep the orphaned object until internal GC, but it's unreachable via
  normal git operations and carries no real exposure.
