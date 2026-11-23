# The birth of the repository `synthub` — statement and plan

Statement of 2026-11-23 for ticket OB-ud1.29 (exploration: no repository is created, nothing is moved, renamed or
published). It reads the copy at commit `046fb005`. It carries out `RENAME-SYNTHUB.md` § 4 (row R2) and `FRONTIER.md`
§ 9, whose decisions it does not reopen: the program is SyntHub, command `synthub`, in `roomi-fields/synthub`,
depending on osc-bridge as a library; AGPL-3.0-or-later with the same CLA; first version 0.25.0; nothing is published
before the breaking version.

Instruments: file:line are read in the copy; the lists of items were made by reading every `crate::`/`super::` path and
every `use` of the module outside comments and outside `mod tests`, then the declaration of each item in the bridge.
That instrument sees paths; it does not see a method or a field reached through a value. Fields and methods were read
for the types the module builds or reads (`Probe`, `Probed`, `Identity`, `Capture`, `McpOptions`, `Known`): all are
`pub` but those of `Known`. The proof that nothing is missing is the build itself (§ 8, step 1 and step 2).

## 1. What leaves for the new repository

### 1.1 The module `crates/osc-bridge/src/synthub/`

| File | Lines | Public items | What it holds |
|---|---|---|---|
| `src/synthub.rs` | 12 | declares the 8 submodules (`:5-12`) | the entry of the module |
| `synthub/online.rs` | 790 | 19 | service client, account, settings, cache folder, `SERVER` |
| `synthub/online/tests.rs` | 131 | — | tests of `online`, declared `online.rs:790` |
| `synthub/learn.rs` | 696 | 21 | listening to an unknown device, fingerprint, share |
| `synthub/learn/tests.rs` | 418 | — | declared `learn.rs:696` |
| `synthub/mcp.rs` | 858 | 7 | the server of the linked tools: envelops the local server, forwards 18 tools |
| `synthub/mcp/tests.rs` | 263 | — | declared `mcp.rs:858` |
| `synthub/connect.rs` | 396 | 4 | the relay door and the test synth (`mod tests` inline, `:369`) |
| `synthub/relay_protocol.rs` | 133 | 8 | the relay frames written by hand (`mod tests` inline, `:121`) |
| `synthub/take.rs` | 257 | 6 | taking a sound through the service's librarian (`mod tests` inline, `:171`) |
| `synthub/catalogue.rs` | 93 | 6 | `locate` of an id or slug, `tier_rank`, fetch |
| `synthub/catalogue/tests.rs` | 36 | — | declared `catalogue.rs:93` |
| `synthub/command.rs` | 42 | 2 | `privacy` and `mcp` commands |

Total: 3 265 lines in the 8 files plus `synthub.rs` and 848 in the four test files. Counts of public items: `grep -c '^pub '`
per file (total 73).

### 1.2 What the binary of the bridge calls (it keeps the call, loses it at R0)

`src/main.rs` reaches the link by full path only (frontier guard): `connect::DEFAULT_RELAY` `:70`, `catalogue::locate` at
`:273 :295 :296 :312 :324 :333 :412 :445`, `connect::run` `:280`, `command::privacy` `:281`, `learn::print_learning` `:283`,
`mcp::print_detection` `:288`, `command::mcp` `:325`, `catalogue::tier_rank` `:285 :289`. R0 removes these; they are not
copied: the program writes its own `main`. See O3 (§ 4): the bodies of six commands live in `main.rs` and not in the
library.

### 1.3 Tests

The tests of the bridge are in `crates/osc-bridge/tests/it/` (one executable). 24 files name `osc_bridge::synthub::`
(`grep -c 'synthub::'`); with `tests/common/mod.rs:184-185 :286 :291` (`online_in`, `mcp_session_in`) they reach the link
through helpers too. Three kinds, which R0 and the birth treat differently:

| Kind | Files (lines naming the link) | Goes |
|---|---|---|
| The subject is the link | `online_account.rs` 5, `fingerprint_sharing.rs` 7, `learn_service_matches.rs` 10, `first_driver_path.rs` 14, `link_envelope.rs` 6, `relay_frames.rs` 1 (+ `fixtures/relay_frames.json`), `driver_freshness.rs` 1, `driver_download.rs` (helper), `newer_version_notice.rs` (helper), `detect_probes_wire.rs` 1, `isolated_config.rs` 1, `install_export_fetch.rs` 2, `account_isolation_guard.rs` 3 (a guard on linked sessions, `:155`), `learn_mode.rs` 2 | to synthub with the module |
| The subject is the bridge, the link is a stand-in | `detection_units.rs:13`, `osd_driver.rs:376`, `install_export.rs:417`, `presets_with_device.rs:129`, `roles.rs:155`, `export_formats.rs:92 :139`, `published_tool_lists.rs:25 :97`, `detect_consent.rs:12 :91 :157`, `detection_proofs.rs` (`:8 :11 :95`), `detection_capture.rs` 9, `tier_order.rs:2 :6`, `common/mod.rs:286` | stay; R0 replaces `catalogue::tier_rank` by a neutral order of the test (the value the tests give as `tier_order`) |
| The guard of the frontier | `synthub_frontier_guard.rs` (324 lines) | stays, reduced (§ 5) |

The split of a mixed file (`detection_proofs.rs` uses `learn_report` at `:8`, which is the link's) is R0's work, file by
file; this statement gives the groups, not each assertion (not measured test by test).

Fixtures: `tests/fixtures/` is read by both kinds; the program's tests that need cards copy those they read
(`fixtures/devices`, `captures`, `relay_frames.json`): a copy, not a sharing, since a repository cannot read another's
test folder.

### 1.4 Package, manifests and workflows of the program

| Piece | Where | At the birth | At R0 in the bridge |
|---|---|---|---|
| npm shim and installer | `npm/` (`package.json:2` name, `:4` `mcpName`; `bin/osc-bridge.js`; `scripts/postinstall.js:15`) | copied, renamed (R5) | removed (npm is not the bridge's: FRONTIER § 9 Q5) |
| MCP registry entry | `server.json:3` | copied (R5) | removed |
| bundle, plugin, listings | `mcpb/manifest.json:3`, `.claude-plugin/plugin.json:3 :33`, `smithery.yaml:16`, `glama.json` | copied (R5) | removed |
| version mirrors | `scripts/sync_version.py` (`MIRRORS`, `:44`), `.github/workflows/version-mirrors.yml` | copied, their list follows the program | the list keeps what remains (`Cargo.toml` only) |
| release | `.github/workflows/release.yml` (build `:45`, asset name `:50-52`, `npm-publish` `:60`, `:82`) | copied: binaries and npm (R5) | keeps the binaries leg, loses `npm-publish` and the registry step |
| CLA | `CLA.md`, `.github/workflows/cla.yml:32 :37 :40` | copied, address of the new repository | kept (the bridge keeps its contributors) |
| the CI of the frame | `ci.yml`, `machine-vierge.yml`, `nuit.yml`, `platforms.yml`, `package.json` `grillhouse.flux` | the new repository installs the frame by its own `package.json`, not a copy | unchanged |
| texts | `PRIVACY.md`, `docs/MCP.md`, `docs/MCP_DIRECTORIES.md`, `CHANGELOG.md` (its first entry), `README.md` | the program's part is written there (R6) | R0 removes the SyntHub parts |

Nothing of this list is moved by this ticket; it is the statement R4–R6 and the birth tickets start from.

### 1.5 The three crates that hold only documents

Measured: `crates/synthhub-protocol/docs/` (`CADRE.md` 34 lines, `INTERFACE.md` 35), `crates/synth-core/docs/` (40, 41),
`crates/synth-io/docs/` (36, 37); no `Cargo.toml` among them (`FRONTIER.md:14-16` says it, and `Cargo.toml:5` lists
`crates/osc-bridge` alone).

- `synthhub-protocol`: what leaves is its two documents, taken into the repository `synthub` as `synthub-protocol` (D4 of
  `RENAME-SYNTHUB.md`; `CADRE.md:5` R1 makes it the types between the service and its clients). They are written there at the
  second time of the birth (§ 3), renamed by R3 (the wire).
- `synth-core` (`CADRE.md:5` R1 session, recognition, programs, scripts) and `synth-io` (`CADRE.md:5` R1 ports, sockets,
  OSCQuery) describe the translation: they **stay in the bridge**. They hold no code; the library of the bridge is today
  the one crate `osc-bridge` whose modules `recognition`, `librarian`, `runtime`, `scripting`, `ws`, `oscquery`… are what
  these two documents will frame. Their `Consumers` lines name the service and the site next to `osc-bridge`
  (`synth-core/docs/INTERFACE.md:36-38`): the service reads the bridge's library, as D4 of the frontier says.
- The library of the repository `synthub` (D4: the contract and the common logic) receives the contract
  `synthub-protocol` and the taking of a sound (`src/synthub/take.rs`, 257 lines, `FRONTIER.md:107` marks it "a library of
  the repository `synthub`"). The service reads from it only these two: never the program's commands.

## 2. What the program reads of the bridge

Instrument: the introduction. Of the items the module reads outside itself, 16 are not public (§ 2.2).

### 2.1 Public today (what the library already publishes for the program)

`recognition`: `IDENTITY_REQUEST` (`recognition.rs:19`), `parse_identity_reply` (`:49`), `Probe` (`:131`),
`maker_request_fault` (`:142`), `TierOrder` (`:107`), `rank` (`:452`), `Ranked::is_strong` (`:440`); `librarian`:
`parse_hex` (`:38`), `split_sysex` (`:19`), `read_syx` (`:184`), `write_syx` (`:177`), `send_messages` (`:116`);
`midi_port`: `Midi` (`:14`), `Opener` (`:39`), `guarded_open` (`:44`), `open_hardware` (`:58`); `capture`: `Capture`
(`:77`; `read` `:117`; `FORMAT_VERSION` `:17`, read by the tests); `detect`: `Probed` (`:90`), `ClientKind` (`:106`),
`client_kind` (`:125`), `observe` (`:240`), `usb_cards` (`:59`), `usb_of_port` (`:78`), `label` (`:147`); `mcp`:
`McpOptions` (`:51`), `DeviceIndex` (`:89`; `build` `:99`, `find_path` `:146`), `run` (`:169`), `run_session` (`:193`),
`rpc_reply` (`:224`), `local_tool_schemas` (`:283`), `hardware_candidates` (`:995`), `hex_bytes` (`:1050`),
`detect_capture` (`:1137`), `slugify` (`:1509`), `vendor_dir` (`:1520`), `content_structured` (`:1828`),
and, read by the tests of the module: `catalogue_tool` (`:983`), `detect_probes` (`:1012`), `probes_json` (`:1035`),
`match_port` (`:1073`). `osd` types come through the `osd` crate, which the program also depends on.

### 2.2 Not public yet: "à publier" by ticket B1 (§ 6)

All are `pub(crate)`: the module reads them as a sub-module of the crate; outside the crate they do not exist.

| Item | Declared | Read by the module at | Published by |
|---|---|---|---|
| `detect::client_number` | `detect.rs:308` | `learn.rs:33` | B1 |
| `detect::ClientKey` | `detect.rs:317` | `learn.rs:13 :82` | B1 |
| `detect::client_key` | `detect.rs:319` | `learn.rs:13 :79` | B1 |
| `mcp::build_index` | `mcp.rs:159` | `synthub/mcp.rs:135` | B1 |
| `mcp::tool_names` | `mcp.rs:863` | `synthub/mcp.rs:175` | B1 |
| `mcp::Known` and its four fields | `mcp.rs:875-882` | `synthub/mcp.rs:17 :247`, `mcp/tests.rs:239` | B1 |
| `mcp::call_local` | `mcp.rs:884` | `synthub/mcp.rs:247`, `mcp/tests.rs:238` | B1 |
| `mcp::device_arg` | `mcp.rs:973` | `synthub/mcp.rs:17 :199` | B1 |
| `mcp::proofs_json` | `mcp.rs:1058` | `learn.rs:14 :552` | B1 |
| `mcp::detect_tool` | `mcp.rs:1098` | `synthub/mcp.rs:241` | B1 |
| `mcp::detect_hardware` | `mcp.rs:1111` | `synthub/mcp.rs:356` | B1 |
| `mcp::software_cards_only` | `mcp.rs:1191` | `learn.rs:14 :557` | B1 |
| `mcp::detect_port` | `mcp.rs:1298` | `learn.rs:611`, `synthub/mcp.rs:263` | B1 |
| `mcp::port_answer` | `mcp.rs:1310` | `learn.rs:614` | B1 |
| `mcp::unlisted_text` | `mcp.rs:1330` | `learn.rs:14 :475` | B1 |
| `mcp::format_detection` | `mcp.rs:1383` | `synthub/mcp.rs:361` | B1 |

Rows: 16 items (`Known` and its fields are one row). `ProbesOf` and `AnswersOf` (`mcp.rs:1092 :1095`) are
private aliases in the signatures of `detect_tool` and `detect_hardware`; an alias expands to public types, so they need no
publication (rustc accepts a private alias in a public signature; not played here).
These publications precede the birth: the program does not build without them (§ 8, step 1). R0 only removes.

### 2.3 What the program cannot get from the library today

The bodies of `run`, `inspect`, `backup`, `restore`, `export`, `orchestrate` are in the binary: `main.rs:273 :312 :333
:412 :445` call `catalogue::locate` then the library. The program that keeps these commands by id or slug (`FRONTIER.md`
§ 3, § 9 Q1: the program "passes a path to the library") either rewrites their bodies or the bridge moves them into the
library. O3 (§ 4).

## 3. The skeleton of the new repository, and its four times

| Time | What | Who |
|---|---|---|
| 1 Grill | structure: role, frontier, refusals, interface, language | done: `FRONTIER.md` § 9, `RENAME-SYNTHUB.md` § 3, this statement; the owner decided, the supervisor leaves the working decisions below |
| 2 Writing | the row of `synthub` in the table of components of the new `docs/ARCHITECTURE.md`, its `CADRE.md`, `INTERFACE.md`; same for the library and for `synthub-protocol` (moved documents) | the supervisor, with `redacteur`; read by the owner |
| 3 Creation | the repository (private until the breaking version), the frame installed, the Cargo workspace; `npx grillhouse paquet <path>` for each crate | the owner creates the repository (outside this repository); the supervisor runs the creation |
| 4 Code | the copy of the module, the program's `main`, the pin; one ordinary circuit ticket | the circuit |

Skeleton:

- Crates, under `crates/`: `synthub` (binary `synthub`, program), a library of the repository (the contract
  `synthub-protocol` and the common logic such as `take`, D4), and nothing for `synth-core`/`synth-io` (they stay in the
  bridge). Language: Rust for each (charter "All the code is Rust").
- Dependencies: `osc-bridge = { git = "https://github.com/roomi-fields/osc-bridge", rev = "<commit of B1>" }` in
  `[workspace.dependencies]`, so every crate of the repository reads one pin; `Cargo.lock` is committed (a binary).
  `publish = false`, as `Cargo.toml:13` of the bridge: the program ships as binaries and npm (R5).
- Framework files: those the frame installs in a project (charter, `package.json` with `grillhouse.flux` and its pinned
  frame version, `.beads`, `deny.toml`, `rust-toolchain.toml`, `.config/nextest.toml`); the CI is the frame's own
  (`ci.yml`, `machine-vierge.yml`, `nuit.yml`) plus the platforms and release workflows copied (§ 1.4). The CI machine needs
  `libasound2-dev` as `scripts/ci_preparer.sh:8-11` does.
- Licence and CLA: `LICENSE` (AGPL-3.0-or-later) and `CLA.md` and `cla.yml` copied, their address changed (`cla.yml:32 :37`).
- First version: `0.25.0` in `[workspace.package]` (decision 2026-11-23, FRONTIER § 9 Q7).
- History: the new repository starts from the program's code; the history up to the split stays in `osc-bridge` (D1). The first
  commit of the new repository says in its message the commit of `osc-bridge` the module was copied from (`git log -1` of
  the bridge at the time of step 2; today `046fb005`). No `git filter-repo`: the module was born in the bridge's history, which is
  where `git log -- crates/osc-bridge/src/synthub/` finds it.

## 4. Decisions

Decided (supervisor, working decisions; the owner gave his go on 2026-11-23 for the remaining plan and left them to the supervisor):

- **S1. How the program reads the library: a git revision, not the registry.** The mature reference is the registry
  (`quinn` over `quinn-proto`; `cargo` and its libraries, not measured in their sources here). Departure, with its reason:
  nothing is published before the breaking version (RENAME-SYNTHUB § 4, 2026-11-23), and `docs/ARCHITECTURE.md:20` already says
  another repository "takes at a git tag" the library. The pin is a `rev` (a commit), as a tag can move; the lock file records
  the commit in any case. The probe of § 7 shows what this costs. The publication of the library on a registry is decided before the
  published version (O1).
- **S2. The new repository starts from the program's code**; history stays in the bridge (D1, owner), a pointer in its first
  commit (§ 3).
- **S3. `synth-core` and `synth-io` stay in the bridge** (§ 1.5); the service reads from the program's library only the
  contract and the common logic (D4). To revisit when code fills them.

To the owner (each with its options; recommendation first):

- **O1. Publish the library on crates.io, and when.** (a) Not before the breaking version; the program lives on the git pin until then
  (recommended). (b) At the birth: lifts `publish = false` (`Cargo.toml:13`) and makes semver of every `pub` item, including
  the 16 of § 2.2. Measured on 2026-11-23: no crate named `osc-bridge` nor `synthub` is on the registry (`cargo search osc-bridge` lists
  `midi-daemon` only; `cargo info synthub`: not found), and its two dependencies `osd` and `osd-formats` 0.6.0 are on it
  (`Cargo.lock:733-746`), so (b) is possible; the cost is the commitment, not a missing piece. Published interface: the owner decides.
- **O2. The repository is created private and made public at the breaking version.** D1 says "public"; the creation of a public
  repository publishes it, against "nothing is published before the breaking version". Recommended: private, with the
  library repository `osc-bridge` public, so the git pin reads without a secret. Sends something outside: owner.
- **O3. What the `synthub` command holds** (the one that changes the size of the program). (a) The commands of the
  service only (`connect`, `privacy`, `detect --learn`, `mcp`) and, for the six commands that name a device by id or slug,
  a body in the program that calls the library. (b) The bridge moves the bodies of `run`, `inspect`, `backup`, `restore`,
  `export`, `orchestrate` from `main.rs` into its library (an addition, ticket B1b), and the program reuses them; both binaries
  hold one code. Recommended: (b), "one translation code, never two" (`RENAME-SYNTHUB.md`, first section: the program "builds on osc-bridge as a library"). It makes the CLI
  of the bridge a published interface that the program shares: owner.

## 5. The order of the moves, step by step

State = main of the bridge / main of `synthub`. "Green" = the repository's own verification.

| Step | Ticket | What | Bridge | synthub |
|---|---|---|---|---|
| 0 | done | the internal split: one module, guarded (OB-ud1.18 → .26) | green, one module | does not exist |
| 1 | B1 (+ B1b if O3 (b)) | the bridge publishes the 16 items of § 2.2: addition only, no behaviour changes; a test of `tests/it/` (an external crate: it sees only `pub`) uses each item | green, the module still inside | does not exist |
| 2 | T1 (writing), T2 (creation), T3 (code) | the repository is born; T3 copies the module (paths `crate::mcp::…` become `osc_bridge::mcp::…`, `crate::synthub::x` becomes `crate::x`), its tests, the program's `main`, pin = commit of B1 | unchanged, green | green on the pin; **two living copies** of the module, none released |
| 3 | R0 (bridge), then T4 (synthub) | R0 removes the module, the commands, the 20 tools, the tests of the link (§ 1.3), the npm package and registry entry; T4, a ticket of `synthub` played as soon as R0 is on main, advances the pin to the commit of R0 | green (R0's own verification, § 6) | before T4: green, still on the pre-R0 pin, which still carries the module the program does not read; after T4: green on R0, the one living copy is the program's |
| 4 | any later removal of a public item | the ticket that stops `synthub` reading the item comes first, then the removal in the bridge, then the bump of the pin | green | green throughout |

The two copies of step 2 are the one transient state, bounded by "nothing is published": no release of either repository
sits between step 2 and step 3, and R0 follows the birth at once. It is the rule "a replacement removes the replaced" played over
two repositories: the removal is the next ticket, not a thing that waits.

### Who moves the pin; where the guards live

- The pin is moved by a ticket of the `synthub` repository, never by the bridge (the bridge knows no program: `docs/ARCHITECTURE.md`
  § 3.2). Who asks for it: the supervisor, after each lot of the bridge that the program must take; each such ticket
  rebuilds and plays the program's tests on the new commit before it enters.
- The guard of the frontier stays in the bridge and keeps one duty: the sources name neither the service nor its host
  (`synthub_frontier_guard.rs`: `HOST` `:40`, `NAMES` `:43`, the test `the_sources_of_the_bridge_name_neither…`). Goes at R0: the
  exemptions of `lib.rs` and `main.rs` (`exempt_lines`), the skip of `src/synthub/` (`problems`), the check that
  `src/synthub/online.rs` exists, the tests of reading the link, and the **inscribed list** `INSCRIBED` (`:21-37`): its seven
  lines are published texts of `main.rs` and `mcp.rs` (the `--help` of `privacy` and `detect --learn`, the schema of
  `share_fingerprint`); R0 removes these lines from the bridge with their commands, and the guard refuses a list entry that matches no
  line (`problems`, the "stale inscribed entry"), so the list must go in the same commit. The texts themselves are in the
  module's copy and in the program's `main` and stay word for word there until R3–R4 rewrite them; the program names itself,
  so no guard of names is written in `synthub` (reading of the supervisor's wording, "the list leaves with the module": the
  texts leave with it, the list, a device of the bridge's guard, ends with the bridge's lines). `docs/ARCHITECTURE.md` § 3.2 loses
  its sentence on the split at R0.
- The program is guarded by its own build: `cargo build --locked` and its tests on the pin, in its CI.

Example: the bridge retires `mcp::hex_bytes`, which the program reads (`synthub/mcp.rs:264`). Its CI is green (no reader left in
the bridge). The CI of `synthub` is green and stays so while the pin is the old commit. The bump ticket moves the pin, the build
of `synthub` fails on `hex_bytes`, and the ticket mends it before entering: the red is a ticket of `synthub`, not a red main. By step 4
the order is the safer one: the program stops reading `hex_bytes` first; the bridge retires it after; the bump follows.

Open, not decided here: no job of the bridge builds the program against the bridge's new main, so a break is seen at the bump, not
at the bridge's commit. That is what a pin gives; a nightly bump probe of the program is a ticket for later.

## 6. Tickets proposed, in order

Each is a single delivery. Components: the supplier first.

| # | Ticket | Components | Route |
|---|---|---|---|
| B1 | The bridge publishes the 16 items the link reads: `pub` on § 2.2; a test of `tests/it/` uses each | `crates/osc-bridge` | full circuit (tests of the library surface go red first) |
| B1b | (if O3 (b)) The library holds the bodies of the six commands that name a device | `crates/osc-bridge` | full circuit, comportement-inchangé tests |
| T1 | Writing of `synthub`: architecture row, `CADRE.md`, `INTERFACE.md`, the library, the moved documents of `synthub-protocol` | `synthub` documents | direct, supervisor and owner |
| T2 | Creation of the repository and its frame (private, O2) | `synthub` | direct, owner and supervisor |
| T3 | The program born: the copy of the module and its tests, the program's `main`, the pin, version 0.25.0, CLA | `synthub` (library, program) | full circuit in the new repository |
| R0 | The bridge removes the link: module, commands, tools, tests of § 1.3, npm and registry files, inscribed list, the sentence of § 3.2; stand-in order for the tests of kind 2 | `crates/osc-bridge`, documents | full circuit |
| T4 | The pin of `synthub` advances to the commit of R0 | `synthub` | circuit, as soon as R0 is on main |
| R3–R6 | already listed in `RENAME-SYNTHUB.md` § 4, after T4 | | |

Order: B1 (B1b), T1, T2, T3, R0, T4, then R3–R6 in the window. R1 (service repository renamed) precedes T2 (RENAME § 4).

## 7. Does a git dependency hinder the publication of the program?

The program leaves by GitHub binaries and npm (`release.yml:45 :60-91`: `cargo build --release`, then `npm publish` of the shim);
`publish = false` (`Cargo.toml:13`); no publication on crates.io is planned for it.

Probe played on 2026-11-23, in the temporary folder: a library crate `sonde-lib` in a git repository (one commit), a program
`sonde-prog` whose only dependency is `sonde-lib = { git = "file://…/sonde/lib", rev = "7de646ad7256cd6e1a56019fb7af150d798a0c7d" }`,
cargo 1.99.1.

```
$ cargo build --manifest-path sonde/prog/Cargo.toml
   Compiling sonde-lib v0.1.0 (file:///…/sonde/lib?rev=7de646ad…#7de646ad)
   Compiling sonde-prog v0.1.0 (…/sonde/prog)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s
$ cargo package --manifest-path sonde/prog/Cargo.toml --allow-dirty
error: failed to verify manifest at `…/sonde/prog/Cargo.toml`
  all dependencies must have a version requirement specified when packaging.
  dependency `sonde-lib` does not specify a version
  Note: The packaged dependency will use the version from crates.io,
  the `git` specification will be removed from the dependency declaration.
$ cargo publish --dry-run --manifest-path sonde/prog/Cargo.toml --allow-dirty
error: failed to verify manifest at `…/sonde/prog/Cargo.toml`
  all dependencies must have a version requirement specified when publishing.
  dependency `sonde-lib` does not specify a version
```

Reading: the git dependency builds and runs; `cargo package` and `cargo publish` refuse it unless the dependency also carries a
`version` (cargo's own rule, stated by the message: the packaged crate uses the registry's version and drops the `git` field). So
the git pin hinders **a publication on crates.io of the program, which is not planned**; it does not touch `cargo build --release`
(the binaries), nor `npm publish` of a shim that holds no Rust. Two things the probe does not tell: that a workspace member of the
bridge is found by name through the git pin (cargo documents it for git dependencies on a workspace; not played on `osc-bridge`),
and the effect on `cargo install --git` of the program (not measured; a `rev` is read by it). `cargo package` of the library itself
needs no probe: its registry dependencies are all on the registry (`Cargo.lock:733-746`).

If the owner chooses O1 (b) later, the pin becomes `{ version = "x", git = … }` first, then the version alone: a one-line change
in `[workspace.dependencies]`, and the program is publishable on crates.io too.

## 8. What this statement leaves open

- The proof that § 2.2 is complete is B1's test (an external crate sees only `pub`) and T3's build; a method or a field read
  through a value that this reading missed shows there.
- The split of each mixed test file (§ 1.3, kind 2 and 3) is R0's.
- O1, O2, O3 are the owner's. Until O3 is answered, B1b is not scheduled.
