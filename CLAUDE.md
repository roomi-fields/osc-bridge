# osc-bridge

osc-bridge lets agents and software drive music devices: it translates between OSC and MIDI (1 and 2) or OSC devices, recognises the device plugged in, recalls and takes its sounds, serves an MCP server to agents (locally or over HTTP), and opens a relay door so a hosted service reaches a person's devices. It reads devices described in the Open Synth Descriptors format (OSD) through the `osd` reader, and stays linked to the SyntHub service for its catalogue and shared knowledge. It is a Cargo workspace: the application at the root, its libraries under `crates/`.

**Owner**: Romain Peyrichou. He validates this charter, `docs/ARCHITECTURE.md`, each `CADRE.md` and `INTERFACE.md`, and each ticket labelled `a-valider`; the supervisor validates the other documents. So the owner decides what the engine is.

The framework's rules: @node_modules/@roomi-fields/grillhouse/charte.md

## What decides

The metaframe (`node_modules/@roomi-fields/grillhouse/METACADRE.md`), then the project's frame (`docs/ARCHITECTURE.md`, and the `CADRE.md` and `INTERFACE.md` of each crate), then the OSD specification for what a device descriptor means, then the code. So the intent decides, and the code follows.

## How we arbitrate

In this order:

1. **What does the mature reference do?** For an I/O-free protocol core with its transports beside it: `quinn-proto` and `quinn`; for the logic of a device's sounds: KnobKraft Orm; for describing devices: MIDI 2.0 Property Exchange, MIDNAM and OSCQuery; for agents: the MCP specification. What they do is the default answer; a departure carries its written reason.
2. **What already exists?** A standard, an off-the-shelf tool, a proven crate, a convention of the neighbouring projects is taken as is.
3. **Does the domain require it?** In this order of priority:
   - **safety**: a message that may overwrite a device's memory waits for the person's consent;
   - **compatibility**: a valid descriptor stays readable by later versions, and an installed copy keeps working with the service;
   - **live play**: the delay the core adds is measured, under 1 ms per message excluding MIDI;
   - **credit**: every driver, sound and export carries its sources and their licence.
   A choice that weakens them is measured and stated.
4. **All the code is Rust.** So agents work in the language where they are most effective.

## What goes up to the supervisor

A question no step of the circuit settles goes up to the supervisor, with its evidence. Goes up to the owner, who alone decides it: a published interface (the MCP tools, the relay protocol, the service API types, the CLI), a licence, a release, anything sent outside the repository.

## Commands

- `cargo build --release` builds the application; `cargo test --workspace -q` and `cargo clippy --workspace --all-targets -- -D warnings` cover every member of the workspace. The version, edition, licence, lints and shared dependencies live in the root `Cargo.toml` (`[workspace.package]`, `[workspace.lints]`, `[workspace.dependencies]`); a crate joins the workspace by being listed in its `members`.
- The tests make their temporary folders and files with `tempfile` (`common::temp` in `tests/`).
- `python3 scripts/sync_version.py --check` keeps the published version mirrors equal (until the tools are Rust).
- `npm test -- <paths>` and `npm run verifier` run the tests and the guards through the framework (nextest, clippy, cargo-deny); the dependencies' licences allowed are in `deny.toml`.
- Ticket prefix: `OB-`.
- Push policy: each commit goes to main and is pushed; ask before rewriting history, deleting a branch, changing a CI that publishes a release, or acting outside the repository. Commits and texts sent to other projects carry no mention of AI.
