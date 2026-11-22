# osc-bridge as published — interface

The repository publishes a program, an installer, a directory entry and crates. What crosses each crate is described in its own `INTERFACE.md` under `crates/<name>/docs/`; the command line, the MCP tools and the OSC addresses are in `crates/osc-bridge/docs/INTERFACE.md`.

## Overview

| Published thing | Where | Who takes it |
|---|---|---|
| the `osc-bridge` program, one file per platform | the GitHub release of the version: `osc-bridge-linux-x86_64`, `osc-bridge-windows-x86_64.exe`, `osc-bridge-macos-arm64`, `osc-bridge-macos-x86_64` | people, the npm installer |
| the installer | npm, `@roomi-fields/osc-bridge` (`npx @roomi-fields/osc-bridge`) | people, agents' MCP clients |
| the directory entry | the MCP registry (`server.json`), the Claude plugin (`.claude-plugin/plugin.json`), the bundle (`mcpb/manifest.json`) | MCP clients and directories |
| the crates `synth-core`, `synth-io`, `synthhub-protocol`, `osc-bridge` | the git tag `v<version>` of this repository | the SyntHub service and site |
| the changelog | `CHANGELOG.md`, one entry per version, breaking changes under their own heading | everyone above |

The version is written once, in `[workspace.package]` of the root `Cargo.toml`; `python3 scripts/sync_version.py` copies it to the mirrors, and `--check` refuses a difference.

## A release

| Step | Done by | Result |
|---|---|---|
| the owner asks for a version | the owner | the version and its changelog entry on main, the release published on GitHub |
| build | the CI (`release`) | the four programs attached to the release |
| publish | the CI (`npm-publish`), after every build | the npm package, then the registry entry |

A run started by hand republishes the npm package of an existing release, and does nothing when that version is already on npm.

## The faults

- `no prebuilt binary for platform` — the installer runs on a platform no release covers; it names the four covered and points to building from source.
- a version mirror that differs — `sync_version.py --check` fails and names the file.
- an old form of a published surface — an explicit error naming the current form and asking to install the current version (frame, R15).

## The guard

- The CI of each branch builds the program and runs the tests of every crate before main moves.
- `sync_version.py --check` holds the mirrors equal.
- `cargo-deny` holds the dependencies' licences and advisories (`deny.toml`).
- The snapshot of the MCP tools list (application interface) shows any change of a published surface before a release.

## Consumers

- People and agents on their machine (the program, the installer).
- MCP directories (the registry entry).
- The SyntHub service and site (the crates, by tag).

## Risks

- The installer's file names and the CI's are written in two places (`npm/scripts/postinstall.js`, `.github/workflows/release.yml`); a platform added in one and not the other fails only at install.
- No program is built for Linux ARM or Windows ARM; a person there builds from source.
