---
title: Catalogue API for AI agents — JSON and MCP
description: >-
  Read the MIDI implementation of 963 synths, drum machines and DAWs from a
  script or an AI agent: a static JSON API on this site, and the osc-bridge
  MCP server over stdio or HTTP.
schema_type: TechArticle
---

# Catalogue API for AI agents

The device catalogue is open to programs, not only to people. An agent that
needs the CC, NRPN or SysEx map of a synth can read it in two ways: plain
JSON files on this site, or the osc-bridge MCP server.

## JSON files

No key, no rate limit, any origin (CORS is open). The data changes when the
site is rebuilt, at each change to the catalogue.

| File | Content |
|------|---------|
| [`/api/v1/index.json`](https://osc-bridge.roomi-fields.com/api/v1/index.json) | Every driver: vendor, name, slug, source, firmware, coverage, links |
| `/api/v1/devices/<vendor>/<driver>.json` | The driver file itself, as osc-bridge loads it |

One entry of `index.json`:

```json
{
  "id": "korg/minilogue",
  "vendor": "KORG",
  "name": "minilogue",
  "slug": "minilogue",
  "kind": "hardware",
  "source_tier": "pencilresearch",
  "firmware": null,
  "license": "CC-BY-SA-4.0",
  "manufacturer_id": [],
  "coverage": { "cc": 40, "sysex_params": 0, "commands": 0, "cc14": 1, "nrpn": 0, "replies": 0, "triggers": 0 },
  "url": "https://osc-bridge.roomi-fields.com/api/v1/devices/korg/minilogue.json",
  "page": "https://osc-bridge.roomi-fields.com/devices/korg/minilogue/"
}
```

- `source_tier` says how far to trust the driver: `hardware-verified` or
  `software-verified` (tested), `vendor-doc` (manufacturer documentation),
  `vendor-osc-api`, `electra-preset` (Electra One community preset),
  `pencilresearch` ([MIDI Guide](https://midi.guide) community data).
- `firmware` is the firmware version the source documents, or `null` when the
  source does not say. Several drivers can exist for one device, one per
  source and firmware.
- `coverage` counts what the driver maps: CC parameters (`cc14` of them
  14-bit, `nrpn` of them NRPN), SysEx parameters, SysEx commands, reply
  patterns and named drum sounds (`triggers`).
- The driver format is described in the [device schema](DEVICE_JSON_SCHEMA.md).
- Licence: drivers are GPL-3.0; those derived from MIDI Guide are also
  CC BY-SA 4.0 (`license` field), so keep the attribution when you reuse them.

```bash
curl -s https://osc-bridge.roomi-fields.com/api/v1/index.json \
  | jq '.devices[] | select(.vendor == "Moog") | .name'
```

## MCP server

The same catalogue as MCP tools, for Claude, Cursor and other MCP clients.

**On your machine (stdio)**, with all eight tools including hardware
detection and sending to a running bridge: see the [MCP guide](MCP.md).
Such a copy is linked to this server unless `--offline`: it mirrors the
catalogue (`GET /v1/catalogue` lists every driver file with its SHA-256;
`POST /v1/catalogue/files` with `{"paths": [...]}`, 100 at most, returns
their contents) and sends its reports and contributions here.

**Hosted, nothing to install**: add `https://mcp.osc-bridge.roomi-fields.com/mcp`
as a remote MCP server (a "custom connector" in Claude). It offers, at most
120 requests per minute per client: the catalogue tools `list_devices` (with
`query`, `vendor`, `kind`, `limit`), `list_routes` and `get_device_docs`;
`report_result` and `get_trust` (below); and the relay tools, which reach
the hardware on the user's own machine (below).

No sign-up. The server gives each client an account when it connects: a
random id, returned in the `Mcp-Session-Id` header and in the server's
instructions, and kept by the server. To stay the same account from one
session to the next, keep it: connect to your personal endpoint
`https://mcp.osc-bridge.roomi-fields.com/u/<id>/mcp` (`GET /u/new` hands out
a fresh one), or pass `user_id` in tool calls. Only ids the server issued
are accepted; the id is tied to nothing else.

What the hosted server records: the accounts it issued; for each tool
call, the time, the account, the tool and its catalogue arguments (search words, vendor,
device slug); and the reports sent with `report_result`. Both are kept to
improve the catalogue. It does not record your
address. Like any site, the web server in front of it keeps a short access
log (address, URL, time) for a few days for traffic statistics; the search
words are not in it.

### Relay: the user's hardware, from anywhere

The user opens [the connect page](connect.md) in Chrome or Edge (or runs
`osc-bridge connect`), which shows a pairing code, and gives the code to the
agent. With it:

| Tool | Does |
|------|------|
| `relay_status` | Lists the MIDI ports of the user's machine |
| `relay_detect` | Sends the MIDI Identity Request to every output, matches the answers (manufacturer, model, firmware) against the catalogue |
| `relay_send` | Sends one driver address with its arguments (`/filter/cutoff`, `[64]`), turned into the right CC, NRPN, note or SysEx by the server — or raw MIDI bytes |
| `relay_listen` | Collects what the hardware sends for a few seconds, decoded; with a driver, each CC or NRPN that moved is matched to its addresses |

#### Contributing drivers

Agents can add to the catalogue. `submit_driver` takes a whole driver file
(the [device schema](DEVICE_JSON_SCHEMA.md)) for a device the catalogue lacks;
`submit_fix` builds a corrected copy of a driver from the mappings learned for
one firmware (below) where they differ from it. Either way the file is its
own variant, `<device>.agent-contributed.fw-<firmware>.json`, marked 🤖 in the
catalogue. Every half hour a merge job checks each pending contribution —
it must parse, lint and leave the library tests passing, and touch nothing
else — and merges it as a pull request; `get_contribution` tells where it
stands. There is no human review: trust comes from usage reports afterwards,
and contributions of a suspect account are held.

#### Learning what controls really send

Drivers can be wrong for a given device or firmware. To check one control,
the agent asks the user to move it while `relay_listen` runs, then records
what it saw with `report_mapping`: the driver address (or, for a control the
driver lacks, its panel label) and the CC, NRPN or note it sent, with the
firmware and the device's Identity Reply. When 3 accounts or more observe the
same message for a control on one firmware, more than any other, the mapping
is learned. `get_trust` lists a driver's learned mappings with what the
driver says and whether they differ; all of them are public at
[`/v1/learned`](https://mcp.osc-bridge.roomi-fields.com/v1/learned).
Observations of suspect accounts are not counted.

Any program can be a relay door: it opens a WebSocket to
`wss://mcp.osc-bridge.roomi-fields.com/relay` (add `?user=<id>` to keep its
account), receives `{"type":"hello","code","user"}`, then exchanges JSON text
frames — `{"type":"ports","outputs":[…],"inputs":[…]}` and
`{"type":"midi_in","port","data":[bytes]}` towards the server,
`{"type":"midi_out","port","data":[bytes]}` from it, and optionally
`{"type":"share","enabled":bool,"private":[port names]}`: `enabled: false`
keeps every device identity out of the records, `private` only those seen on
the ports named (sharing is on, no port private, until the door says
otherwise). `relay_detect` marks each private device with `private: true`.

Messages that may overwrite the device's memory — every SysEx message except
the Identity Request — never come as `midi_out`. They come as one request per
batch, `{"type":"confirm","id","port","messages":[[bytes]],"bytes","gap_ms"}`:
the door shows it to the person, sends the messages only if they accept
(waiting `gap_ms` between two), and answers
`{"type":"confirm_reply","id","accepted":true|false}`. The agent's
`relay_send` waits up to two minutes for that answer and reports it; without
an answer the server withdraws the request with
`{"type":"confirm_cancel","id"}`. A door must not send a SysEx message other
than the Identity Request that came as `midi_out`.

### Reports and trust

After testing a driver on the real device, an agent calls `report_result`:
`works`, `fails` or `partial`, for the whole driver or one OSC address, with
the firmware version and the device's reply to the MIDI Identity Request
(`F0 7E 7F 06 01 F7`) as `identity_hex`. That reply proves the device was
connected and carries the firmware as the device reports it; it must come
from the driver's manufacturer.

Trust is counted per firmware, from each account's latest accepted report:

| Status | Meaning |
|--------|---------|
| `unreported` | No report yet |
| `unconfirmed` | Reported working, by fewer than 3 accounts on one firmware or none with an identified device |
| `usage-verified` | 3 accounts or more agree it works on one firmware, one of them with an identified device, and none reports otherwise there |
| `firmware-dependent` | Works on some firmware, fails on another |
| `failing` | Only failures reported |
| `disputed` | Accounts disagree on the same firmware, or a contrary report gives no firmware |

Each report has a status: `accepted` (counts), `pending` (waits for
approval), `rolled-back` or `rejected`. A report that at least 3 other
accounts contradict on the same target and firmware, more than agree with
it, is rolled back automatically: the driver's trust returns to what it was
without it. Its account becomes suspect: its other reports, and its new ones,
are pending until an administrator approves them or clears the account.
`get_trust` shows how many reports of a driver are pending or rolled back.

When a disagreement involves a report without firmware, `get_trust` sets
`needs_firmware` and the server asks the next reporters to identify their
device. The identities seen for each firmware are kept, so the catalogue
learns how each device reports its versions. The trust of every reported
driver is public at
[`/v1/trust`](https://mcp.osc-bridge.roomi-fields.com/v1/trust).

Reports waiting for approval and suspect accounts are reviewed by the
server's administrator. The tools that open MIDI ports are not offered by the
hosted server, since they would act on its machine, not the caller's; the
relay tools reach the caller's own hardware instead.

```bash
curl -s -X POST https://mcp.osc-bridge.roomi-fields.com/mcp -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
       "params":{"name":"list_devices","arguments":{"query":"moog sub"}}}'
```
