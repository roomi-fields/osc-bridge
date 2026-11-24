# osc-bridge — the frontier with the SyntHub program

Statement of the frontier and the owner's answers (ticket OB-ud1.13). Nothing is moved, renamed or published by it. Sections 1 to 8 read the code and give the options; section 9 carries the owner's answer to each question and its date.

**The decision it serves** (owner, 2026-11-22): osc-bridge stays a true OSC bridge, with no link to SyntHub; the program SyntHub (command `synthub`, new repository `roomi-fields/synthub`) builds on it as a library. This statement reads the code as it is at commit `d25a203d` and says, module by module, what keeps the bridge small and what ties it to the service.

## 1. What exists

The workspace lists one member, `crates/osc-bridge` (`Cargo.toml`, `members`). `synth-core`, `synth-io` and `synthhub-protocol` have a `docs/CADRE.md` and an `INTERFACE.md` and no `Cargo.toml`: they are target crates, not code. So the whole engine, the service client and the relay door are today in one crate, a library (`osc_bridge`, `src/lib.rs`) and a binary (`src/main.rs`) with 14 commands. The service client and the relay frames are written by hand in `online.rs` and `relay_protocol.rs`, not typed by `synthhub-protocol` (`crates/synthhub-protocol/docs/INTERFACE.md` says it is the target of the core epic).

| Crate | State | Piece |
|---|---|---|
| `osc-bridge` | code, 12 468 lines in `src/*.rs` (plus 558 in two sub-modules) | `crates/osc-bridge/src/lib.rs:15-36` lists the 22 modules |
| `synth-core` | documents only | `crates/synth-core/docs/` has `CADRE.md`, `INTERFACE.md` |
| `synth-io` | documents only | `crates/synth-io/docs/` |
| `synthhub-protocol` | documents only | `crates/synthhub-protocol/docs/` |

## 2. How each module is classed

The command (run from the root of the repository; it reads every `crate::` and `osc_bridge::` of a module, without the comment lines and without what follows `mod tests {`; `mod tests;` files are tests and are not read; the sub-modules `mcp/roles.rs` and `orchestrator/links.rs` count for their module):

```bash
cd crates/osc-bridge/src
for f in *.rs mcp/roles.rs orchestrator/links.rs; do
  [ "$f" = tests.rs ] && continue
  awk -v F="$f" '/^mod tests \{/{exit} /^[ \t]*\/\//{next}
    {l=$0; while (match(l,/(crate|osc_bridge)::[a-z_0-9]+/)) {s=substr(l,RSTART,RLENGTH); sub(/.*::/,"",s); print F, s, NR; l=substr(l,RSTART+RLENGTH)}}' "$f"
done | awk '!seen[$1" "$2]++'
```

Each output line is `file module line`: the first line where the file reads that module. A module is **net** when its graph of imports (through the modules it reads, and theirs) reaches none of `online`, `catalogue`, `connect`, `relay_protocol`, `take`, `mcp`, `detect`. The piece of a module that is not net is the first import (lowest line) whose target is one of those seven, or a module that is not net itself.

Measured (the command above, read through the rule; the closure is by hand over about 80 lines):

| Module | Lines | Net | First import that takes it out | What it needs |
|---|---|---|---|---|
| `routing` | 174 | yes | — (reads no module) | nothing but OSC and MIDI |
| `midi_codec` | 129 | yes | — | OSC and MIDI |
| `capture` | 131 | yes | — | a local capture file |
| `ws` | 226 | yes | — | a WebSocket, OSC |
| `relay_protocol` | 133 | yes by its imports, forbidden by name | — (no import: the `connect` it names is a comment, `relay_protocol.rs:3`) | the service's relay frames, nothing of the service's code |
| `scripting` | 737 | no | `scripting.rs:188` → `frame` | Lua, a local device card |
| `frame` | 394 | no | `frame.rs:212` → `device` | a local device card |
| `device` | 330 | no | `device.rs:187` → `detect` (`faults` calls `detect::probe_of`) | a local device card |
| `programs` | 363 | no | `programs.rs:15` → `device` | a local device card |
| `bridge_routes` | 137 | no | `bridge_routes.rs:8` → `device` | a local device card |
| `librarian` | 233 | no | `librarian.rs:139` → `device` | a `.syx` file, MIDI |
| `runtime` | 1969 | no | `runtime.rs:12` → `device` | a local device card, OSC, MIDI |
| `export` | 101 | no | `export.rs:67` → `bridge_routes` | `osd-formats`, a local card |
| `oscquery` | 205 | no | `oscquery.rs:15` → `export` | HTTP listener, OSC |
| `install` | 233 | no | `install.rs:17` → `export` (then `:20` → `take`, `:82` → `detect`) | a plugged device, a local card |
| `orchestrator` (+ `links`, 373) | 720 | no | `orchestrator.rs:56` → `runtime` (then `:365` → `catalogue`) | a `bridge.toml`; the driver by `catalogue::locate` |
| `detect` | 1184 | no (is forbidden) | `detect.rs:15` → `device` (`:586` → `mcp::tier_rank`) | MIDI ports, local cards; with the service: matches, probes |
| `mcp` (+ `roles`, 185) | 2832 | no (is forbidden) | `mcp.rs:69` → `online` (`Link` holds an `Online`) | MCP, OSC; the service, the account, the cache |
| `take` | 336 | no (is forbidden) | `take.rs:80` → `detect` (`:84` → `librarian`) | a plugged device; its `find` reads the service's librarian answer |
| `online` | 790 | no (is forbidden) | `online.rs:667` → `detect` (`:671` → `librarian`, `:711` → `mcp`) | the service, the account, the settings, the cache |
| `catalogue` | 77 | no (is forbidden) | `catalogue.rs:14` → `online` (`:73` → `mcp`) | the cache, the service |
| `connect` | 396 | no (is forbidden) | `connect.rs:25` → `relay_protocol` (`:127` → `detect`, `:204` → `online`) | the relay, the account |
| `main` (CLI) | 561 | no | `main.rs:70` → `connect` | see section 3 |

What the table shows: **four modules are net** (`routing`, `midi_codec`, `capture`, `ws`). Everything else leaves net through one edge: `device.rs:187`, where `faults` calls `detect::probe_of` (a pure function that builds a SysEx request from a card's `identity.request`). That edge alone takes `device`, `frame`, `programs`, `bridge_routes`, `librarian`, `runtime`, `scripting`, `export`, `oscquery`, `install` and `orchestrator` out of net. So "net" is a strict test; the pure part of those modules is large and the service part small. The reading in section 4 uses the pieces, not the count of net modules.

## 3. The path of each command

A command is classed by what it calls, apart from its module. Six commands go through `catalogue::locate`, which reads the service state `online::linked()`, `online::cache_dir()` and `online::SERVER` (`catalogue.rs:25-26`), and so pass through `online`:

| Command | Piece | Reads the service when the argument is not a file |
|---|---|---|
| `restore` | `main.rs:273` | yes (an id or slug is fetched, or read from the cache) |
| `export` (`--on`, then the target) | `main.rs:301`, `:302` | yes, for both the driver and the target |
| `export --format` | `main.rs:319` | yes |
| `backup` | `main.rs:350` | yes |
| `inspect` | `main.rs:429` | yes |
| `run` | `main.rs:462` | yes |
| `orchestrate` | `orchestrator.rs:365` (reached from `main.rs:331`) | yes, for each device of the file |

The ticket's criterion 8 also names `lint`. Measured: `lint` does not: `cmd_lint` loads a file only (`main.rs:365`, `device::load(&path)`). So six commands, not seven. With a file argument `locate` returns the path at `catalogue.rs:20-21` and touches nothing; the service is touched only by an id or a slug.

The other commands:

| Command | Path | Net |
|---|---|---|
| `list` | `cmd_list` (MIDI ports) | yes |
| `osc-send`, `osc-listen` | `cmd_osc_send`, `cmd_osc_listen` (`main.rs:490`, `:512`) | yes |
| `lint` | `main.rs:365` | a file only |
| `export --on` | `install::install_command` with `take::open_hardware` (`main.rs:301`) | no: `take` is forbidden |
| `detect` (no `--learn`) | `mcp::print_detection` (`main.rs:296`); without `--devices-dir` it reads the cache and, linked, the service (`mcp.rs:2337`, `:2346`) | no |
| `detect --learn` | `synthub::learn::print_learning` (`main.rs:292`); asks the service for matches; `--share-fingerprint` sends one | no |
| `mcp` | `mcp::run` with `Settings` (`main.rs:335-341`) | no |
| `connect` | `connect::run` (`main.rs:280`) | no: the relay, the account |
| `privacy` | `online::settings_file`, `mcp::privacy` (`main.rs:282-288`) | no: the service's settings |

## 4. The side of each module

The side of each module, the bridge or the program; where it was not obvious, the row names the question of section 9 that decides it, and what no answer names stays marked **open**.

| Module | Proposed side | Why |
|---|---|---|
| `routing`, `midi_codec`, `capture`, `ws`, `scripting`, `frame`, `programs`, `bridge_routes`, `runtime`, `orchestrator`, `oscquery`, `export`, `librarian`, `device` | bridge | translation and transport; they need OSC, MIDI and a local card |
| `main` (the commands of section 3 that are not in the next lines) | bridge | `list`, `osc-send`, `osc-listen`, `lint`, `run`, `inspect`, `backup`, `restore`, `export --format`, `orchestrate` |
| `relay_protocol` | program | the frames of the door; its only reader is `connect` (**open**: the frames are typed by `synthhub-protocol` in the target) |
| `connect` | program | the relay door and the account |
| `online` | program | the service client, the account, the settings, the cache |
| `catalogue` | bridge for a file, a folder or a web address; program for an id or a slug (question 1, decided) | `locate` mixes a file (bridge) and an id or slug (program) |
| `detect` | bridge for the local recognition; program for learn and the sharing of a fingerprint (question 3 (a), decided) | one module: the pure reading (probes, identity reply, ports) and the service (matches, share) |
| `mcp` | bridge for the local tools; program for the tools of the service (questions 2 (a) and 3 (a), decided) | one module: tools that need the bridge only, and tools of the service |
| `take`, `install` | `take`: a library of the repository `synthub` (question 4, decided); `install`: **open**, no answer names it | `take` is the service's librarian; `Hardware` and `install` are local MIDI |
| `Settings`, `privacy` | program | in `online`; read by the MCP server and by detection to decide what is shared |

**The modules where the side is not obvious, with their options and costs (as put to the owner; his answers are in section 9):**

1. **`catalogue::locate`**. (a) The bridge accepts a file or a folder of files only; an id or a slug is the program's, and the program passes a path to the library. Cost: `run`, `inspect`, `backup`, `restore`, `export`, `orchestrate` lose "name a device by slug"; `catalogue.rs:73` and the cache go to the program. (b) The bridge keeps the cache and the id/slug lookup, fed by whoever fills it. Cost: `driver_path`, `is_driver_id` and the cache's layout stay in the bridge, a contract with the program, and `locate` still needs `online::cache_dir` for the path (`catalogue.rs:25`).
2. **`detect` (the recognition of the plugged device)**. (a) The bridge keeps the recognition over a local folder of cards (`--devices-dir`, already there, `main.rs:296`) and the program adds the service's candidates; learn and fingerprint sharing are the program's. Cost: `detect.rs:586` (`mcp::tier_rank`) and `online.rs:667` (`detect::Probe`) have to move; `device.rs:187` stays the pure part. (b) The whole `detect` is the program's. Cost: `device::faults` and `programs.rs:299` read `detect`, so the bridge loses the load-time fault and the identity address. (c) The whole `detect` stays in the bridge and its service calls (`mcp::learn_service_matches`, `online::detect`) go. Cost: the bridge keeps 1184 lines the "no link" decision does not need, and recognition by the service's candidates disappears from it.
3. **The learn mode** (`detect --learn`, `detect_hardware` with `learn`, `share_fingerprint`). (a) The program owns it (the listening, the fingerprint, the share). Cost: the bridge no longer listens to a device to describe it; the measure (`measured`) goes with it. (b) The bridge keeps the listening and writes the fingerprint to a file; the program shares it. Cost: the fingerprint's form (an OSD 0.6 fingerprint, `docs/INTERFACE.md` of the application) is a contract between the products.
4. **The MCP server without the service**. (a) The bridge keeps the server with its 16 local tools; the program adds the service's tools by wrapping it. Cost: `mcp.rs` carries `Online` in its state (`Link`, `mcp.rs:69`; `FORWARDED_TOOLS` at `online.rs:47`; `tool_names` at `mcp.rs:1309`) and has to be cut at `build_index`, `call_tool` and `tool_names`. (b) The program owns the whole server; the bridge has none. Cost: an agent no longer drives a device with the bridge alone. (c) The bridge keeps the whole server and a hook for extra tools. Cost: a published interface (the MCP tools) gets a second form.
5. **`take`, `install`**. `take` reads a service answer (`find(midi, lib: &Value, …)`, `take.rs:89`); `install` reads `take` (`install.rs:20`) and `detect` (`install.rs:82`). (a) `take` goes to the program, `install` stays and its use of `take::Hardware` moves to a small MIDI type in the bridge. Cost: a split of `take.rs`. (b) Both go to the program. Cost: `export --on` leaves the bridge (`main.rs:303`).

## 5. The dependencies that cross the frontier in the wrong direction

The bridge reads something of the program or of the service (the proposed frontier of section 4):

| From (bridge) | To (program or service) | Piece |
|---|---|---|
| `device` | `detect::probe_of` | `device.rs:187` |
| `catalogue::locate` and the CLI | `online::linked`, `cache_dir`, `SERVER`, `Online`, `driver_path` | `catalogue.rs:14`, `:25-26` |
| `catalogue::cached` | `mcp::DeviceIndex` | `catalogue.rs:73` |
| `orchestrator` | `catalogue::locate` | `orchestrator.rs:365` |
| `main` | `catalogue::locate` (six commands) | `main.rs:273`, `:301`, `:302`, `:319`, `:350`, `:429`, `:462` |
| `detect` | `mcp::tier_rank` | `detect.rs:586` |
| `install` | `take`, `detect` | `install.rs:20`, `:82` |
| `take` | `detect`, `librarian` | `take.rs:80`, `:84` |
| `mcp` | `online` (`Link`, `FORWARDED_TOOLS`, the notice of a newer version) | `mcp.rs:69`, `:258`, `:324`, `:1315` |
| `mcp` | `detect`, `take`, `install`, `catalogue` | `mcp.rs:311`, `:1430`, `:2126`, `:2128` |

And the reverse, the service's client reading the engine: `online.rs:667` (`detect::Probe`), `:671` (`librarian::parse_hex`), `:711` (`mcp::device_id`). They are not "wrong" if `online` is the program's; they become the program reading the bridge, which is the right direction, once `detect::Probe` and `librarian::parse_hex` are in the library.

## 6. The folder of drivers the bridge reads: its contract

**Format**: one JSON file per driver, in the OSD format, read through `osd` (`device::read_own`, `device::load`, used by `DeviceIndex::build`, `mcp.rs:149-159`).

**Layout** (`online.rs:158-161`, `driver_path` at `online.rs:312`): `<root>/<maker>/<name>.json`, its notes `<name>.md` beside it, and the server's validator `<file>.etag` beside each (`etag_path`, `online.rs:165-168`). The folder `--devices-dir` given to `mcp` and `detect` has the same walk: `DeviceIndex::build` visits every `.json`; a name that starts with `_` is a maker's profile and is not a driver (`mcp.rs:154`); the id of a driver is its path under the root without the extension (`mcp.rs:178`); the slug is the descriptor's `osc_prefix` (`mcp.rs:161`).

**Emplacement**: `$XDG_CACHE_HOME` or `%LOCALAPPDATA%` or `~/.cache`, then `osc-bridge/drivers` (`online::cache_dir`, `online.rs:161`) when no folder is given.

**Who writes it**: the service client, one driver at a time, by `Online::fetch_driver` (`online.rs:620-654`), through `osd::resolve(id, &files)` which fetches a driver's parents by id; the person, by hand, for `--devices-dir`.

**Is it OSD's alone?** Measured: the contract mixes both.
- OSD's: the JSON file, its resolution by `osd`, `osc_prefix`, the `sources` list (`mcp.rs:162`). Whether `osc_prefix` and `sources[].kind` are fields of the OSD specification and which values `kind` takes is not verified here against the OSD specification; the code reads them through `osd` types.
- Specific to SyntHub: the `<maker>/<name>` identifier and its file layout (`driver_path`, `is_driver_id` at `online.rs:236`), the `.etag` files, the `.md` notes fetched from `/api/v1/devices/<id>.md` (`online.rs:640`), and the ranking `tier_rank` (`mcp.rs:210-221`), whose values (`hardware-verified`, `vendor-doc`, `contributed`, `test-fixture`…) are the catalogue's tiers; a driver with a `kind` not in the list ranks 7.

So the folder that `--devices-dir` reads is nearly OSD's alone; the shape of the cache and the tier order are the service's. This is question 1.

## 7. What an installed copy keeps, and what moves

Measured on the code at `d25a203d`. After the split, "the bridge alone" means: no service, no relay, no account.

**Keeps working in the bridge alone** (needing no service): `list`, `osc-send`, `osc-listen`, `lint`; `run`, `inspect`, `backup`, `restore`, `export`, `orchestrate` with a file; `detect` with `--devices-dir` or `--read`; `mcp --offline --devices-dir`, 16 tools (`mcp.rs:1310-1312`: `list_devices`, `get_device_docs`, `list_routes`, `export_device`, `send`, `get_status`, `add_link`, `remove_link`, `list_links`, `detect_hardware`, `backup_presets`, `restore_presets`, `install_export`, `list_roles`, `set_by_role`, `get_by_role`); the OSC addresses (`docs/INTERFACE.md`).

**Stops working in the bridge, or moves to the program** (counted from the code):

| What | Count | Piece |
|---|---|---|
| the relay command `connect` | 1 command, 4 options (`server`, `no_share`, `test_synth`, `private_ports`) | `main.rs:68-86`, `:279` |
| `privacy` | 1 command; the settings it writes are read by the MCP server and by detection | `main.rs:87-102`, `online.rs:87-110` |
| `detect --learn` | 1 mode, 6 flags (`--learn`, `--listen-ms`, `--share-fingerprint`, `--vendor`, `--model`, `--firmware`) | `main.rs:292`, `docs/INTERFACE.md` (the `detect` row) |
| the MCP tools forwarded to the service | 18 | `online.rs:47-50` |
| the tools that need the link, beyond the 18 | 2 (`take_sound`, `load_preset`) | `mcp.rs:1313-1314` |
| the learn parameters of an MCP tool | `learn`, `listen_ms`, `share_fingerprint` of `detect_hardware` | `docs/INTERFACE.md` ("The MCP tools") |
| the name of a device by id or slug (`locate`) | 6 commands | section 3 |
| the cache of drivers and its fetch | the folder `…/osc-bridge/drivers` | `online.rs:161` |

Total MCP tools of a linked copy: 16 local + 2 linked + 18 forwarded = 36; offline 16. **20 of the 36 leave the bridge** if the whole link leaves it.

Published interfaces touched, which go up to the owner (charter): the CLI (`connect`, `privacy`, `detect --learn` and its flags go; `run` and 5 others lose the id/slug form, by the answer to question 1); the MCP tools (20 of 36); the relay protocol (`connect` is its door, and its frames are `relay_protocol`); `docs/INTERFACE.md` lists each.

**Where the explicit error for a copy too old is born.** The charter asks it (`docs/ARCHITECTURE.md` §8, "Compatibility"; `docs/CADRE.md` R15: whoever uses the old form gets an explicit error that asks to install the current version). Today:
- there is a notice, not an error, born at the copy from the service's answer: a reply with a `Deprecation` header and a `Link rel="deprecation"` makes the copy print "A newer osc-bridge exists: <url>" once on the standard error and in the next MCP result (`online.rs:179-222`, `mcp.rs:324`). It needs the service to mark the copy, and the copy to still call the service.
- the type that would carry a version, `identity::ClientVersion`, and the error `UnknownVersion` are in the target interface of `synthhub-protocol` (`crates/synthhub-protocol/docs/INTERFACE.md:12`, `:20`) and not in any code.

After the split the error can be born in two places: at the service, which refuses an old copy by its version and says to install the program (an old copy keeps its `connect` and its tools and calls the service); or at the bridge, where a removed command prints an error instead of the clap message "unrecognized subcommand". R15 reads as asking for the second (the person who types `osc-bridge connect` gets the error); question 2 puts both, and the owner's answer (a) takes the first.

## 8. What the references do

- The reference the charter names for a protocol core with its transports beside it is `quinn-proto` and `quinn` (`CLAUDE.md`, "How we arbitrate"; `docs/ARCHITECTURE.md` strategy 1). What they do is not verified here, by execution or in their source: this document repeats what our own architecture says of them.
- The model of a library and applications that depend on it named by the ticket (ripgrep and its `grep` crates, Ollama on llama.cpp) is not verified here either: no execution and no source read in this session. The recommendations below rest on the charter's own arbitration (the first question, "what does the mature reference do", answered from `quinn` as `docs/ARCHITECTURE.md` states it), and a recommendation that rests on ripgrep or Ollama stays marked "not verified" until the grill.

## 9. The questions for the owner and his answers

Each of questions 1 to 3 has its options (sections above), what each costs, a recommendation, and the owner's answer of 2026-11-22. Questions 4 to 8 are the five questions on the program side, with the owner's answers of 2026-11-23.

1. **The contract of the folder of drivers the bridge reads** (architecture; a published contract between the two products). (a) The bridge reads only a file or a folder of OSD files the person gives (`--devices-dir`, already there), with no cache and no id or slug; the program owns and fills its cache. (b) The bridge keeps a cache in today's format (`online::driver_path`), which keeps it tied to the service. Cost of (a): six commands lose the id/slug form (section 3). Cost of (b): the layout, `.etag` and tier order stay a contract the bridge carries. **Recommendation: (a)**, the shape of `quinn-proto`'s core with its transports beside it (`docs/ARCHITECTURE.md` strategy 1), the core without the service. **Decided 2026-11-22**: the bridge reads an OSD card from a file, a folder, or any web address that returns a card (redirections followed); the bridge has no code proper to SyntHub, no cache of the service and no short name. SyntHub may offer a query address that returns a card or redirects to one; an ambiguous query returns an error that names the candidates, never a guessed card. **Amended 2026-11-23**: the bridge keeps a copy of a card read from a web address, so that it starts again without the network; on that point alone the answer changes, and the bridge still has no cache of the service, no id and no short name.
2. **What happens to `connect`, `privacy`, `detect --learn` and the 20 service tools** (published interfaces; the owner decides). (a) Remove them in one commit, without alias (the repository's rule: the replaced goes in the same commit), and the service refuses an old copy by the version of the API of `synthhub-protocol`, with the message asking to install the program. (b) Keep a command that prints only the explicit error "install synthub". (c) Both. Cost of (a): the person who types a removed command gets clap's generic error, which R15 may not accept as "explicit"; cost of (b): the commands stay in the CLI as stubs, a second form of the interface. **Recommendation: (a)**; the reading of R15 is the owner's. **Decided 2026-11-22: (a)**: the relay door, privacy, the learn mode and the tools tied to the service leave for the program at once, without a shell.
3. **The side of `detect`, the MCP server and `take`, and the shape of the crates.** (a) A library of translation with no service that the program consumes (as the `grep` crates of ripgrep, not verified), and a thin application beside it; `detect` reads a folder local to the bridge, and learn and the sharing of a fingerprint go to the program; the MCP server keeps its local tools. (b) Everything stays in one crate and the program depends on the whole. (c) `detect`, the MCP server and `take` go whole to the program. Costs: sections 4 and 5. **Recommendation: (a)**, which also answers section 5: after the move, `device::faults` calls a pure function of the library, and `online::probes` reads `detect::Probe` the right way round. **Decided 2026-11-22: (a)**: a common library of translation; the bridge keeps a local recognition and an MCP server reduced to the local tools; learn and the sharing of a fingerprint go to the program.
4. **What the private service builds on once the program is out.** **Decided 2026-11-23: (a)**: the library of the bridge and a library of the repository `synthub` (the contract `synthub-protocol` and the common logic such as taking a sound); the service never touches the program's commands.
5. **How the bridge alone installs.** **Decided 2026-11-23: (a)**: binaries on the GitHub releases and `cargo install`; neither npm nor the MCP registry. The deprecation message of the old npm package names both paths (`RENAME-SYNTHUB.md` § 3, D6).
6. **The licence of the repository `synthub`.** **Decided 2026-11-23: (a)**: AGPL-3.0-or-later and the same CLA.
7. **The first version of `synthub`.** **Decided 2026-11-23: (a)**: it continues at 0.25.0.
8. **Whether the invisible preparation starts before the signal.** **Decided 2026-11-23**: yes, as soon as possible: the internal split of osc-bridge, the documents and the birth of the repository; nothing is published.

Questions 1 to 3 depend on each other in the order given: the folder's contract fixes what `locate` may read; the published interface fixes what stays; the sides and the crates follow.
