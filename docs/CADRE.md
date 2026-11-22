# osc-bridge as published — frame

## Role

- **R1** The repository publishes osc-bridge as one product at one version: the `osc-bridge` program for a person's machine, its installer on npm, its entry in the MCP directories, and the crates the SyntHub service and site build on.

## Receives

- **R2** The lots the circuit integrates on main, each judged green by the CI of its branch.
- **R3** The owner's word for a release: a version number and its line in the changelog.
- **R4** The `osd` reader and `osd-formats`, pinned to one commit of the OSD repository.

## Returns

- **R5** A GitHub release per version, carrying the program for Linux x86-64, Windows x86-64, macOS ARM and macOS x86-64.
- **R6** The npm package `@roomi-fields/osc-bridge`, which fetches the program of its own version at install, and the entry of the MCP registry that names it.
- **R7** A git tag per version, from which another repository takes `synth-core`, `synth-io`, `synthhub-protocol` and the application as crates.

## Knows

- **R8** The four crates of the workspace and the order in which they depend on each other; the version, written once in the root `Cargo.toml`; the published surfaces of the application (command line, MCP tools, OSC addresses).

## Does not know

- **R9** The catalogue of devices and sounds, which lives in the SyntHub service; the site and its pages; the OSD format, which lives in its own repository.

## Refuses

- **R10** A release the owner has not asked for.
- **R11** A release whose version differs between the root `Cargo.toml` and its mirrors (npm, the plugin, the registry entry, the bundle).
- **R12** Publishing the npm package before every program of the release is attached.
- **R13** A dependency whose licence the list of allowed licences does not hold.

## Invariants

- **R14** One version for the whole workspace: every crate, the program and the npm package carry it.
- **R15** A published surface has one form. When it changes, the old form is removed in the same release, the changelog says it under a breaking heading, and whoever still uses the old form gets an explicit error that asks them to install the current version.
- **R16** Every release carries its sources and their licence: the code under AGPL-3.0-or-later, each driver and sound under the licence of its source.
- **R17** A release contains no driver: the program takes its drivers from the service, or from a folder the person gives.

## Cost

- **R18** From the owner's word to the npm package available: one run of the CI, under 15 minutes (5 to 8 measured on the last three), with no step done by hand.
