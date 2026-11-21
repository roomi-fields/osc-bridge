# Writing a driver in other formats — every format of osd-formats

A driver file can be written in every format the `osd-formats` package offers
(`oscquery`, `midnam`, `deluge`, and the others it lists), which other tools
read. Two are detailed here:

- **OSCQuery** — the OSC addresses of the device, with their types, ranges and
  access, as the JSON namespace of the
  [OSCQuery proposal](https://github.com/Vidvox/OSCQueryProposal). OSC
  controllers and browsers build their controls from it.
- **MIDNAM** — the device's MIDI names as a MIDI Name Document 1.0: its
  controllers and drum notes, by name, for DAWs (Ardour, Logic, Digital
  Performer…).

Every format is written by the `osd-formats` package, the one everyone uses: a format
corrected there is corrected here. What each element of a descriptor becomes in
each format is in its documents (`osd-formats/docs/oscquery.md`,
`osd-formats/docs/midnam.md`).

The same text comes from three places:

| Where | How |
|---|---|
| Command line | `osc-bridge export --device korg/minilogue --format oscquery` (any format of `osd-formats`, for example `midnam` or `deluge`; an unknown name lists them): the document on the standard output, what the format could not carry on the error output, one line each (`not carried: <place>: <reason>`) |
| MCP server | tool `export_device` with `device` (id or OSC prefix) and `format`; the document is the text content, the list `lost` (`place`, `reason`) is beside it in the structured result |
| Running bridge | `osc-bridge run … --oscquery-bind 127.0.0.1:7891`, or `oscquery_bind` in the `[osc]` section of `bridge.toml` |

## Putting an export on a device

An export can be put on a recognised device, in one gesture that is the same
for every target: what the target receives is said in its own descriptor
(`resources`: the format it exports, the path of the file, the steps of a file
written by SysEx), so osc-bridge holds no command per device. A target that
declares nothing it receives is refused.

| Where | How |
|---|---|
| Command line | `osc-bridge export --device korg/minilogue --on synthstrom/deluge [--yes]` (no `--format`: the target gives it) |
| MCP server | tool `install_export` with `device`, `on` and `confirm_overwrite` |

A write may overwrite a file on the device, which is not read first: without
`--yes` (`confirm_overwrite: true`) nothing is sent, and the format, the way,
the path, the size and what the format could not carry are shown. A target that
is not plugged in is refused before any message. A send that fails on the way
says the step reached and the messages already sent; nothing is retried or
undone. The library function is `osc_bridge::install::install`.

The library function is `osc_bridge::export::export(&device, format)`: it
returns the document with the list of what the format could not carry. The
hosted server serves the same tool, so the site and the API give the same
output.

## Credit and licence

Both documents carry the credit and licence of the driver's `_sources`. A
driver with no source is not written (`NoSources`): its credit cannot be.

- OSCQuery: in the `DESCRIPTION` of the device's node.
- MIDNAM: in the `<Author>` element.

## Errors

- A device that speaks OSC only has no MIDNAM: the export fails with
  « <device> speaks OSC, not MIDI: it has no MIDI names ».
- An unknown format name fails with « unknown format '<name>' (one of:
  oscquery, midnam) ».

## The running bridge

The listener stays in osc-bridge: it answers `GET /` (whole namespace),
`GET /<path>` (that node and what is under it), `GET /<path>?ATTRIBUTE` (one
attribute, 204 when the node has none, 400 for an attribute it does not know)
and `?HOST_INFO` (`OSC_IP`, `OSC_PORT`, `OSC_TRANSPORT` `UDP`, and
`WS_IP`/`WS_PORT` when `--ws-bind` is on). The namespace it answers is the
export document; under `orchestrate` the documents of every device are merged
under one root.

Left out: `VALUE` (the bridge keeps no state), `LISTEN` and the other
WebSocket commands, and Zeroconf advertising (`_oscjson._tcp`): give clients
the address by hand.

## What osc-bridge adds to the namespace

`osd-formats` writes the namespace of the device. The addresses that are
osc-bridge's own conventions, not the device's, are added by one module of
osc-bridge (`bridge_routes`), for the export and for the running bridge alike:
`/bridge/*`, and for each MIDI device `/identity`, `/identity/reply`,
`/program/recall`, `/program/dump`, `/raw/syx`, `/sysex/raw`. They keep the
types, ranges, `VALS` of the bank names and `OVERLOADS` they had.

## A driver with no source

No source is made up. The export of a driver with no `_sources` is refused with
the fault of `osd-formats` (`NoSources`). The running bridge names that fault at
start-up and does not serve that driver's OSCQuery namespace; the rest of the
driver works as before, and a link under its prefix is not judged against a
namespace it does not have.

## Differences from the former exporters

Measured by `crates/osc-bridge/tests/it/export_documents.rs` against facts
frozen from the exporters osc-bridge held before (`tests/fixtures/exports/`).
These differences are accepted:

- OSCQuery: no `OVERLOADS` on the routes of the descriptor's controls and
  commands (the descriptor has no field for another argument list), and no
  `EXTENDED_TYPE` for a control's value (the descriptor names no argument of a
  controller);
- OSCQuery: the credit is on the device's node, not on the root;
- OSCQuery: no `/param/get`, no MIDI performance routes (`/note/on`, `/cc/<n>`…),
  no route a script adds, and no address the bridge sends (`midi_in`,
  `replies`): the package writes the routes of the controls, commands and
  triggers (what it does not carry is in the list of what is lost);
- MIDNAM: a driver without banks has no empty `PatchBank`;
- MIDNAM: the named values of a control come from its `values` field; the free
  text `_usage` is no longer read.

What `osd-formats` could not carry is shown to the person who exports.
