# The program takes the name SyntHub — statement

Statement of 2026-11-22 for ticket OB-ud1.12 (exploration: nothing is renamed, published or sent).

Decided by the owner (2026-11-22, repositories and strategy, replacing any earlier wording):

- The program is called SyntHub, its command is `synthub`; no alias and no transition; an installed copy too old gets an
  explicit error asking to install the current version; OSD keeps its name.
- The repository `osc-bridge` is **kept** and comes back to a true OSC bridge (translation between OSC and MIDI or OSC
  devices, reading a device card given locally), with no link to SyntHub.
- The program SyntHub is born in a **new public repository `roomi-fields/synthub`** and builds on osc-bridge as a library:
  one translation code, never two.
- The service's private repository becomes `roomi-fields/synthub-server`.
- The frontier between the bridge and the program is a decision of structure taken by its own ticket (grill with the
  owner). This statement does not decide it: for each published name it says whether it names the **bridge**, the
  **program**, or is **undecided** until the frontier.

Counts are the supervisor's command, `git grep -I -i -c -E "osc[-_ ]?bridge" -- . ':!node_modules' ':!.claude' ':!.beads'`
(tracked files). This repository: 817 lines in 141 files, of which 65 lines are in `CHANGELOG.md` and 1 in `BACKLOG.md`
(`$ git grep -I -i -c -E "osc[-_ ]?bridge" -- CHANGELOG.md BACKLOG.md` → `65`, `1`). The two neighbours' counts are given by
the supervisor and taken as is: synthhub 922 lines in 157 files, open-synth-descriptors 211 lines in 30 files. The per-row
line counts below are indicative (a slightly different pattern).

Class column: **B** = names the bridge (the name `osc-bridge` stays); **P** = names the program (becomes `synthub`, in the new
repository); **U** = undecided until the frontier ticket (F0 below).

## 1. Where the name is published or read

### 1.1 osc-bridge repository (read in the copy)

| # | Place | Class | Piece | Lines/files |
|---|---|---|---|---|
| 1 | Application crate and its binary | binary **P**; crate **U** | `crates/osc-bridge/Cargo.toml:2` `name = "osc-bridge"`, `:34` `[[bin]] name`, `:38` `[lib] name = "osc_bridge"` | 3 |
| 2 | Crate folder, workspace member, npm workspace | **U** | `Cargo.toml:5` `members = ["crates/osc-bridge"]`; `package.json:52`; `crates/osc-bridge/package.json:2` | 3 |
| 3 | Library path in code and tests | **B** (the program builds on the library `osc_bridge`) | `crates/osc-bridge/src/main.rs` 42 lines (`use osc_bridge::…`; clap `name = "osc-bridge"` at `:12` is **P**); tests `use osc_bridge::…` | in 4 and 5 |
| 4 | Source: texts the person sees, MIDI port names, server name | server name, command shown, notice **P**; MIDI client names and test port **U** | 19 files, 143 lines: `src/mcp.rs:43` `SERVER_NAME`; `src/mcp.rs:1784` (`osc-bridge run …`); `src/oscquery.rs:53,86`; `src/detect.rs:751…`, `runtime.rs:81`, `take.rs:198` (MIDI client names `osc-bridge-*`); `src/connect.rs:40` `CLIENT`, `:96` `TEST_SYNTH_PORT` | 143 / 19 |
| 5 | Tests and fixtures | **U** (follow their code) | 65 files, 284 lines (tests); 9 files, 20 lines (fixtures, counted inside) | 284 / 65 |
| 6 | Folders on the person's machine | account and settings **P**; driver cache **P** (the catalogue is the link to SyntHub) | `src/online.rs:74` `<config>/osc-bridge/user`; `:78` `<config>/osc-bridge/settings.toml`; `:162` `<cache>/osc-bridge/drivers` | 3 |
| 7 | Environment variable | **P** (it switches the link to the service) | `src/online.rs:57` `OSC_BRIDGE_OFFLINE` (also `main.rs:214`, `README.md:86`, `PRIVACY.md:70`, `docs/MCP.md:76`) | 5 |
| 8 | What the service reads of the program | **P** | `src/online.rs:369` `user_agent("osc-bridge/<version>")`; `src/online.rs:27-29,173-203` the notice « A newer osc-bridge exists: <url> » | 2 |
| 9 | npm package and command | **P** | `npm/package.json:2` `@roomi-fields/osc-bridge`, `:4` `mcpName`, `:45` `bin`; `npm/bin/osc-bridge.js`; `npm/scripts/postinstall.js:15,19-22,46,56` | 30 / 4 |
| 10 | MCP registry entry | **P** | `server.json:3` `io.github.roomi-fields/osc-bridge`, `:4`, `:7`, `:14` | 4 |
| 11 | MCP bundle | **P** | `mcpb/manifest.json:3,4,14,17,18,48,53` and 6 tool descriptions (`:64,211,266,291,468…`) | 13 |
| 12 | Claude plugin | **P** | `.claude-plugin/plugin.json:3,10,11,31,33` incl. the pin `@roomi-fields/osc-bridge@0.24.0` | 5 |
| 13 | Directory listings and launchers | existing entries **P** (they list the MCP server and its npm package); a bridge-only entry **U** | `smithery.yaml:3,5,16`; `glama.json`; `docs/MCP_DIRECTORIES.md` 6 lines (npm, registry, GitHub, marketplace `roomi-fields/claude-plugins`, awesome lists PR #543 merged, PR #6342 closed, mcp.directory, mcp.so) | 9 |
| 14 | Version guard | **U** (it follows where packaging lives) | `scripts/sync_version.py:42` `_NPX_PIN` `@roomi-fields/osc-bridge@`; mirrors `:44-49` | 1 |
| 15 | Release workflow and trusted publisher | **U** (builds the binary and publishes npm: both **P** today) | `.github/workflows/release.yml:50,52,58,82,88,101`; `:60-65` trusted publisher set on npmjs.com, outside the repository | 6 |
| 16 | Other workflows and templates | **B** (they stay with the repository); flux declaration **U** | `platforms.yml:37,59,86,108`; `cla.yml:32,37,40`; `.github/ISSUE_TEMPLATE/bug_report.yml:9`; `package.json:26,30,33` | 17 / 5 |
| 17 | Repository address | **B** for the bridge's own (`Cargo.toml:14`); **P** for the packaging ones | `Cargo.toml:14`, `npm/package.json:37,40`, `server.json:7`, `mcpb/manifest.json:14,17,18`, `.claude-plugin/plugin.json:10,11`, `postinstall.js:15`, `PRIVACY.md:147`, `cla.yml:32,37` | inside the lines above |
| 18 | Documents | README, `docs/ARCHITECTURE.md`, `CADRE.md`, `INTERFACE.md`, crate docs, `CONTEXT.md`, `CONTRIBUTING.md`, `CLA.md` **B** once rewritten without SyntHub; `PRIVACY.md`, `docs/MCP.md`, `npm/README.md` **P**; the rest **U** | 15 files / 104 lines under `docs/`; `README.md` 52; `PRIVACY.md` 15; `CONTRIBUTING.md` 11; `CLA.md` 7; `CONTEXT.md` 2; `npm/README.md` 9; `docs/ARCHITECTURE.md` 8; `crates/osc-bridge/docs/CADRE.md`, `INTERFACE.md` 6; `docs/agents/index-des-interfaces.md` (generated, 3) | ≈ 200 / 21 |
| 19 | Examples | **B** (SuperCollider speaks OSC) | `examples/sc-quark/OscBridge.sc`, `OscBridge.quark:3,4,6` | 10 / 4 |
| 20 | Windows upload scripts | **B** (to delete: no reader) | `scripts/upload-with-arm.ps1:2`, `upload-preset.ps1:2` (path `D:\Claude\osc-bridge\…`) | 3 |
| 21 | Site pointer page | **P** | `.github/workflows/docs.yml:1,35,40` (GitHub Pages pointer) | 3 |
| 22 | Release-binary asset URL used by installed copies and the npm shim | **P** | `npm/scripts/postinstall.js:19-22`, `npm/bin/osc-bridge.js:14` | in 9 |
| 23 | Past history | **B** (stays in the kept repository) | `CHANGELOG.md` 65 lines; `BACKLOG.md` 1 line | 66 |

Note the two spellings: the service and its address are spelled `synthhub` (two h): `src/online.rs:44`
`https://mcp.synthhub.roomi-fields.com`, `src/connect.rs:34` `wss://mcp.synthhub.roomi-fields.com/relay`,
`crates/osc-bridge/tests/it/newer_version_notice.rs:15` `https://synthhub.roomi-fields.com/osc-bridge/`. 51 files of this
repository name SynthHub. The new name `synthub` has one h; the address change is SH-cgi.6 times 2 and 3, not this ticket.
The `synthhub` spelling is itself a link to the service: where the frontier leaves it in the bridge, the bridge's «no link
to SyntHub» is not held (this is a reason for the frontier ticket to read, not a decision here).

The workspace crates `synth-core`, `synth-io`, `synthhub-protocol` have only their `docs/` here (no `Cargo.toml`); they are
named in `docs/ARCHITECTURE.md:23,46-48`. Class: `synth-core`, `synth-io` **U**; `synthhub-protocol` (the wire to the
service) **P**.

### 1.2 open-synth-descriptors (public)

211 lines in 30 files (count given by the supervisor; the families below were read in its public tarball). Everything it
says names the **bridge** as a consumer of OSD (it is what reads a device card), so class **B** for all, except where noted.

| Place | Class | Piece | Lines |
|---|---|---|---|
| Published schemas 0.1 to 0.6 | **B**, frozen | `schemas/0.6/device.schema.json:404` `"x-osd-extension": "osc-bridge"` and its descriptions; 30 lines each in 0.1–0.4, 18 in 0.5 and 0.6 | 156 |
| Parity tool, outside the workspace | **B** (it depends on the library by path) | `tools/osc-bridge-parity/Cargo.toml:2,7,17` (`path = "../../../osc-bridge"`), `tests/parity.rs` 12, `Cargo.toml:4`, `.gitignore:3`, `CLAUDE.md:38` | 21 |
| Code comments | **B** | `osd/src/device.rs` 9, `osd/src/lib.rs:109`, `osd/tests/it/capabilities.rs` 2 | 12 |
| Documents | **B** | `README.md:65,368`; `docs/INTERFACE.md:39,333,591`; `docs/ARCHITECTURE.md:15,36`; `osd/docs/INTERFACE.md:82,88`; `osd-formats/docs/INTERFACE.md:59`; `osd-cli/docs/CADRE.md:27` | 14 |
| Examples and conformance | **B** | `examples/devices/test_synth.json:7,12`, `examples/0.5/synth.device.json:8,13` (vendor `osc-bridge`) | 6 |
| Group configuration | **U** (the group lists repositories; the new one joins it) | `package.json:34` `../osc-bridge`; `.beads/config.yaml:68` | 2 |

### 1.3 synthhub (private)

922 lines in 157 files (count given by the supervisor; most touched: `crates/service/src/server.rs` 49,
`crates/service/src/server/tests.rs` 33, the OSD schema copies under `site/docs/osd/` 30 each, `ops/server/compose.yml` 29,
`README.md` 27, `ops/server/deploy-osc-bridge` 18, `Dockerfile` 17). Families: the service reads the program's name (user
agent, **P**), serves the page `…/osc-bridge/` (**P**), deploys the program's binary (`deploy-osc-bridge`, **P**), copies
OSD schemas (**B**, frozen). File-by-file statement belongs to its own realisation ticket (R6).

## 2. What keeps the old name, and why

| Keeps | Why |
|---|---|
| The repository `osc-bridge`, its crate `osc-bridge` and library `osc_bridge` (what stays in the bridge after the frontier) | Owner's decision: the repository is kept and comes back to a true OSC bridge; the program builds on it as a library. |
| Past `CHANGELOG.md` entries (65 lines), `BACKLOG.md:1` | History of the repository that stays; says what it was called then. |
| The ticket prefix `OB-` | The prefix is the tracker's, fixed by the charter; it stays with the kept repository. |
| Published OSD schemas 0.1–0.6 (`x-osd-extension: "osc-bridge"`, 156 lines), conformance cases, the vendor `osc-bridge` of the examples | `docs/ARCHITECTURE.md:36` of OSD: « A published version never changes »; and the tag names the bridge as OSD's reader, which stays true. |
| npm `@roomi-fields/osc-bridge` versions up to 0.24.0, GitHub releases up to 0.24.0 | A published version never changes; the old package is deprecated with a message (D6). |
| `x-osd` readers in the program | None found: `grep -rn "x-osd" crates/osc-bridge/src` returns nothing. |
| Private path `D:\Claude\osc-bridge` in the two `.ps1` scripts | A past machine's path; the scripts are candidates for deletion (no reader in the workflows), not renaming. |
| Past directory-listing entries (npm 0.16, registry 0.16 …) | They remain what was published; handled by D6. |

## 3. The owner's decisions

Each with options, a recommendation by the mature model (one brand for the hub and its local program; npm deprecate and a final release pointing to the new name), and the owner's decision with its date. The questions and answers of the frontier and of the program side are in `FRONTIER.md` § 9.

| # | Question | Options | Recommendation | Decision |
|---|---|---|---|---|
| D0 | Frontier between bridge and program (decided by its own ticket, F0; listed here because every **U** waits on it) | (a) the bridge keeps translation (OSC/MIDI/OSC devices), reading a device card given locally, and the MIDI/OSC detection of local devices; the program owns the service link (account, catalogue, driver download, relay, sharing, MCP server, privacy). (b) the bridge keeps also detection and take/recall. | Grill with the owner. Evidence for it: this table's **U** rows; the user agent, folders and `OSC_BRIDGE_OFFLINE` all sit in `src/online.rs`, the relay in `src/connect.rs`, the MCP server in `src/mcp.rs`, so (a) cuts along files that already exist. | Decided 2026-11-22 by the three answers of `FRONTIER.md` § 9: the bridge reads a card from a file, a folder or a web address; the service's commands and tools leave for the program at once; a common library of translation, with a local recognition and a reduced MCP server in the bridge. |
| D1 | Repository names | Decided 2026-11-22: `osc-bridge` kept (true bridge, no link to SyntHub); new public `roomi-fields/synthub` for the program, depending on osc-bridge as a library; service `roomi-fields/synthub-server`. The user account named « Synthub » exists but does not block a repository under `roomi-fields`. | As decided. Order: free the name by renaming the service's repository to `synthub-server` first (GitHub redirects the old address), then create `synthub`. The program's history up to the frontier stays in `osc-bridge`; the new repository starts from the program's code. No alias in code. | Decided 2026-11-22: `osc-bridge` kept, `roomi-fields/synthub` for the program, `roomi-fields/synthub-server` for the service. Decided 2026-11-23: the repository `synthub` is under AGPL-3.0-or-later with the same CLA, and its first version continues at 0.25.0. |
| D2 | npm name (the program's, **P**) | (a) `@roomi-fields/synthub`, scoped, as today. (b) `synthub`, free on npm (HTTP 404 on 2026-11-22), unscoped, as `ollama` or `tailscale`. | (b): « installs it by its name » (`npx -y synthub mcp`). The scope buys nothing since the name is free. Trusted publisher set again on npmjs.com for the new package, bound to the new repository's release workflow. | Decided 2026-11-22: (b), `synthub` without a scope. |
| D3 | MCP registry name (**P**) | (a) `io.github.roomi-fields/synthub` (GitHub owner, checked in the release workflow). (b) a domain namespace, needs a DNS proof. | (a): the workflow already publishes by GitHub OIDC. `mcpName` in the npm `package.json` must equal `server.json` `name`. The old entry `io.github.roomi-fields/osc-bridge` cannot be renamed: the new is published and the old marked deprecated. | Decided 2026-11-22: (a), `io.github.roomi-fields/synthub`; the old entry is marked deprecated. |
| D4 | Names of the workspace crates | (a) the library `osc-bridge` stays (the bridge) and holds what the frontier leaves in it; the program's crate and binary are `synthub` in the new repository; `synthhub-protocol` → `synthub-protocol`, moving with the program; `synth-core`, `synth-io` keep their names. (b) the library is renamed too. | (a) as to names; what is in the library follows the frontier (F0). `synth-core`/`synth-io` describe synthesis, not a program: they keep unless F0 moves them. None published on crates.io (`Cargo.toml:12` `publish = false`): no final crates.io release, and `synthub` stays free there. | Decided 2026-11-22: (a). What the library holds follows `FRONTIER.md` § 9. Decided 2026-11-23: the service builds on the library of the bridge and on a library of the repository `synthub` (the contract `synthub-protocol` and the common logic such as taking a sound), never on the program's commands. |
| D5 | Folders and settings of an installed copy (**P**) | (a) new folders `<config>/synthub/`, `<cache>/synthub/drivers`, nothing read from the old ones: the person links again and sets the privacy settings again; the old folder stays on disk, unread. (b) the new version moves the old folders once. | (a): (b) is a transition, which the rule forbids. Cost stated: the account issued by the service and `settings.toml` start empty; the first run says where the old folder is and that it is not read. The variable becomes `SYNTHUB_OFFLINE`; `OSC_BRIDGE_OFFLINE` is no longer read. The bridge, with no link, keeps no such folder. | Decided 2026-11-22: (a), new folders; the old ones are not read, and the first run says so. |
| D6 | Old npm package, old listings | (a) `npm deprecate "@roomi-fields/osc-bridge@*" "Renamed SyntHub: npx -y synthub mcp"`, a last 0.x release of the old package printing the same. (b) unpublish (npm forbids it after 72 h in most cases). Listings: Smithery, registry, marketplace `claude-plugins`, mcp.so, mcp.directory, awesome lists, Glama. | (a). Listings: publish the new entry, mark the old one deprecated where allowed, a pull request or form elsewhere; `docs/MCP_DIRECTORIES.md` follows the program to the new repository. All outside the repository: the owner decides and sends. Whether the bridge gets its own listing: after F0. | Decided 2026-11-22: (a), the old npm package is marked renamed and the listings are updated, both by the owner. Amended 2026-11-23: the deprecation message names both paths: the hub is `synthub`; the bridge alone installs from the GitHub releases. The bridge alone has binaries on the GitHub releases and `cargo install`, and neither an npm package nor an entry in the MCP registry; its listing in the other directories is **open**. |
| D7 | The explicit error for a copy too old | The service marks a copy deprecated by `Deprecation` and the program prints « A newer osc-bridge exists: <url> » (`src/online.rs:173-203`); an installed copy sends `osc-bridge/<ver>` (`:369`). | The service (SH-cgi.6 time 2) answers every `osc-bridge/*` user agent with the error « SyntHub replaces osc-bridge; install the current version: `npx -y synthub mcp` ». The old copy prints it with its existing notice: no program code for it. | Decided 2026-11-22: as recommended, the service answers an old copy « SyntHub replaces osc-bridge; install the current version ». |
| D8 | The tag `x-osd-extension: "osc-bridge"` in OSD | (a) keep it in all versions. (b) a new schema version says `synthub`. | (a): with the bridge kept as OSD's reader, the tag stays true. OSD's decision; raised only to be seen. | Decided 2026-11-22: (a), the mark `osc-bridge` stays in OSD. |
| D9 | SuperCollider quark, Windows scripts, MIDI port names | Quark `OscBridge` stays (B); MIDI client names `osc-bridge-*` follow the frontier (U); the two `.ps1`: delete. | Quark keeps (it speaks OSC to the bridge). MIDI client names follow whichever side owns the code after F0; a saved patch naming the old port is lost for the program: stated, no alias. Delete the scripts. | Decided 2026-11-22: as recommended; the quark is kept and the two Windows scripts are deleted. |

## 4. Realisation tickets, in order

Each is a single delivery, in a single commit. **W** = enters in the same window as the lots of SH-cgi.6 (its new address, times 2 and 3). The owner decided on 2026-11-23: the invisible preparation (the internal split of osc-bridge, the documents and the birth of the repository `synthub`) starts as soon as possible, before the signal, and nothing of it is published.

| # | Ticket | Components, supplier first | Window |
|---|---|---|---|
| F0 | The frontier between bridge and program, decided on 2026-11-22 (`FRONTIER.md` § 9): writes the cadre and interface of both (D0, D4) | architecture, `crates/osc-bridge` | first; nothing below starts before |
| R0 | The bridge comes back to a true bridge: remove the link to SyntHub from the kept repository (what F0 gives to the program), documents without SyntHub (rows 16, 18 B, 19, 20, 23) | `crates/osc-bridge`, documents | after F0 |
| R1 | Service repository renamed `synthub-server` (frees the name) (D1) | outside this repository, owner | before R2 |
| R2 | The new repository `roomi-fields/synthub`: skeleton, depending on osc-bridge as a library (a package born in four times), AGPL-3.0-or-later with the same CLA, first version 0.25.0 | new repository | after F0, R1 |
| R3 | The wire in the program: user agent, first-run message, notice for an old copy, `SYNTHUB_OFFLINE`, folders `synthub/` (rows 6–8; D5, D7) | `synthub-protocol`, the program's crate | **W** |
| R4 | The program's crate, binary, texts shown, tests (rows 1, 4, 5 P parts; D4, D9) | the program | **W** (the binary name is in the release assets) |
| R5 | The packaging: npm `synthub`, shim, asset names, `server.json`, `mcpb/manifest.json`, `plugin.json`, `smithery.yaml`, version guard, release workflow, trusted publisher (rows 9–15, 22; D2, D3) | the program's `npm`, mcpb, workflows | **W**, same version as R3–R4 |
| R6 | The program's documents and site pages: README, PRIVACY, docs, `MCP_DIRECTORIES.md`, a first `CHANGELOG.md` entry (rows 18 P, 21) | documents | **W** |
| R7 | The service's own names: user agent reading, page `/osc-bridge/`, admin token `OSC_BRIDGE_ADMIN_TOKEN`, deploy script, and its count | synthub-server | **W** (SH-cgi.6 times 2 and 3) |
| R8 | OSD: parity tool path to the kept repository, group configuration; schemas untouched (D8) | open-synth-descriptors | after R2 |
| R9 | Old npm package deprecated; directory listings: new entries, old ones deprecated (D6) | outside, owner | after the release |

Order: F0, then R0 and R1, R2; R3–R6 and R7 form one breaking version with SH-cgi.6 times 2 and 3; R8 and R9 after.

## 5. Not done

- The file-by-file statement of synthhub and open-synth-descriptors: their counts are the supervisor's (criterion 1 as amended).
- The frontier itself (D0, F0): decided by its own ticket, so rows marked **U** stay open.
- Nothing was renamed, published or sent; the OSD tarball was read earlier, in the temporary folder only.
