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

**Over HTTP**, read-only: `osc-bridge mcp --http 0.0.0.0:8080` serves MCP on
`/mcp` with the three catalogue tools only — `list_devices` (with `query`,
`vendor`, `kind`, `limit`), `list_routes` and `get_device_docs`. The tools
that open MIDI ports are left out, since they would act on the server's
machine, not the caller's. The container image runs it the same way:

```bash
docker build -t osc-bridge .
docker run --rm -p 8080:8080 osc-bridge --http 0.0.0.0:8080
```

```bash
curl -s -X POST http://localhost:8080/mcp -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
       "params":{"name":"list_devices","arguments":{"query":"moog sub"}}}'
```
